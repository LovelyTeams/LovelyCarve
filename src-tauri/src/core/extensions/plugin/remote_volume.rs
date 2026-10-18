use crate::core::filesystem::external_volume::{decode_remote_node_id, REMOTE_NODE_PREFIX};
use std::{sync::Arc, time::Duration};

use anyhow::{anyhow, Context as _, Result};
use rmpv::Value;

use crate::core::{
    filesystem::evidence_session::EvidenceSession,
    filesystem::fs_dispatch::{Volume, VolumeChildrenPage},
    filesystem::tree::fs_tree::{ChildrenOptions, FsNode, FsStat},
};

use super::{
    wire::{LppFsNodeV1, LppFsStatV1},
    PluginConnection, PluginError,
};

const REMOTE_READ_CHUNK: usize = 4 * 1024 * 1024;
const METADATA_TIMEOUT: Duration = Duration::from_secs(15);
const DIRECTORY_TIMEOUT: Duration = Duration::from_secs(60);
const READ_TIMEOUT: Duration = Duration::from_secs(120);
const DIRECTORY_PAGE_SIZE: u64 = 1_000;
const MAX_DIRECTORY_ENTRIES: usize = 1_000_000;

impl crate::core::filesystem::external_volume::ExternalVolume for RemoteVolume {
    fn host_root(&self) -> &str {
        self.host_root()
    }
    fn owns_node(&self, node_id: &str) -> bool {
        self.owns_node(node_id)
    }
    fn is_alive(&self) -> bool {
        self.is_alive()
    }
    fn source_id(&self) -> &str {
        self.source_id()
    }
    fn close(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(async move { RemoteVolume::close(self).await.map_err(Into::into) })
    }
}

#[derive(serde::Deserialize)]
struct RemoteNodePage {
    items: Vec<LppFsNodeV1>,
    #[serde(default)]
    next_cursor: Option<String>,
    #[serde(default)]
    total: Option<usize>,
}

#[derive(Clone)]
pub(crate) struct RemoteVolume {
    client: PluginConnection,
    plugin_id: String,
    session_id: String,
    root_node: String,
    host_root: String,
    source_id: String,
}

impl std::fmt::Debug for RemoteVolume {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteVolume")
            .field("plugin_id", &self.plugin_id)
            .field("session_id", &self.session_id)
            .field("host_root", &self.host_root)
            .finish_non_exhaustive()
    }
}

impl RemoteVolume {
    pub(crate) fn new(
        client: PluginConnection,
        session_id: String,
        root_node: String,
        host_root: String,
        source_id: String,
    ) -> Self {
        Self {
            plugin_id: client.identity().id.clone(),
            client,
            session_id,
            root_node,
            host_root,
            source_id,
        }
    }

    pub(crate) fn host_root(&self) -> &str {
        &self.host_root
    }

    pub(crate) fn owns_node(&self, host_id: &str) -> bool {
        if host_id == self.host_root {
            return true;
        }
        decode_remote_node_id(host_id)
            .map(|(plugin, session, _)| plugin == self.plugin_id && session == self.session_id)
            .unwrap_or(false)
    }

    pub(crate) fn is_alive(&self) -> bool {
        self.client.is_alive()
    }

    pub(crate) fn source_id(&self) -> &str {
        &self.source_id
    }

    pub(crate) async fn close(&self) -> super::Result<()> {
        self.client
            .call_with_timeout(
                "fs.close",
                Value::Map(vec![(
                    Value::from("session_id"),
                    Value::from(self.session_id.clone()),
                )]),
                Duration::from_secs(5),
            )
            .await
            .map(|_| ())
    }

    fn opaque_node(&self, host_id: &str) -> Result<String> {
        if host_id == self.host_root {
            return Ok(self.root_node.clone());
        }
        let (plugin_id, session_id, node_id) = decode_remote_node_id(host_id)?;
        if plugin_id != self.plugin_id || session_id != self.session_id {
            anyhow::bail!("remote node belongs to another plugin session");
        }
        Ok(node_id)
    }

    fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.call_plugin(method, params).map_err(Into::into)
    }

    fn call_plugin(&self, method: &str, params: Value) -> super::Result<Value> {
        let timeout = match method {
            "fs.stat" => METADATA_TIMEOUT,
            "fs.children" | "fs.children_page" => DIRECTORY_TIMEOUT,
            "fs.read_range" => READ_TIMEOUT,
            _ => DIRECTORY_TIMEOUT,
        };
        self.client
            .call_blocking_with_timeout(method, params, timeout)
    }

    fn params(&self, node_id: &str, extra: &[(&str, Value)]) -> Value {
        let mut entries = vec![
            (
                Value::from("session_id"),
                Value::from(self.session_id.clone()),
            ),
            (Value::from("node_id"), Value::from(node_id.to_owned())),
        ];
        entries.extend(
            extra
                .iter()
                .map(|(key, value)| (Value::from(*key), value.clone())),
        );
        Value::Map(entries)
    }

    fn encode_node(&self, opaque_id: &str) -> String {
        format!(
            "{REMOTE_NODE_PREFIX}{}:{}:{}",
            hex::encode(self.plugin_id.as_bytes()),
            hex::encode(self.session_id.as_bytes()),
            hex::encode(opaque_id.as_bytes())
        )
    }

    fn read_bytes(&self, id: &str, offset: u64, length: usize) -> Result<Vec<u8>> {
        let opaque_id = self.opaque_node(id)?;
        let size = self.file_stat(id, id)?.size;
        let length = (size.saturating_sub(offset)).min(length as u64) as usize;
        let mut bytes = Vec::with_capacity(length);
        while bytes.len() < length {
            let requested = (length - bytes.len()).min(REMOTE_READ_CHUNK);
            let position = offset
                .checked_add(bytes.len() as u64)
                .context("remote range offset overflow")?;
            let result = self.call(
                "fs.read_range",
                self.params(
                    &opaque_id,
                    &[
                        ("offset", Value::from(position)),
                        ("length", Value::from(requested as u64)),
                    ],
                ),
            )?;
            let chunk = binary_result_with_limit(result, "fs.read_range", requested as u64)?;
            if chunk.is_empty() {
                anyhow::bail!("short remote file read at {position}, expected {requested} bytes");
            }
            bytes.extend(chunk);
        }
        Ok(bytes)
    }

    fn children_page_remote(
        &self,
        id: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Option<VolumeChildrenPage>> {
        let opaque_id = self.opaque_node(id)?;
        let limit = limit.clamp(1, DIRECTORY_PAGE_SIZE as usize);
        let params = self.params(
            &opaque_id,
            &[
                ("limit", Value::from(limit as u64)),
                ("cursor", cursor.map(Value::from).unwrap_or(Value::Nil)),
            ],
        );
        let result = match self.call_plugin("fs.children_page", params) {
            Ok(result) => result,
            Err(PluginError::Rpc { code, .. })
                if code == "METHOD_NOT_FOUND" && cursor.is_none() =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        let page: RemoteNodePage = rmpv::ext::from_value(result)
            .map_err(|error| anyhow!("invalid fs.children_page result: {error}"))?;
        if page.items.len() > limit {
            anyhow::bail!(
                "fs.children_page returned {} entries for a limit of {limit}",
                page.items.len()
            );
        }
        if page.next_cursor.is_some() && page.next_cursor.as_deref() == cursor {
            anyhow::bail!("plugin repeated fs.children_page cursor");
        }
        if page.total.is_some_and(|total| total < page.items.len()) {
            anyhow::bail!("fs.children_page total is smaller than the returned page");
        }
        let mut items = page
            .items
            .into_iter()
            .map(|node| node.into_host(id))
            .collect::<Result<Vec<_>>>()?;
        for node in &mut items {
            node.id = self.encode_node(&node.id);
        }
        Ok(Some(VolumeChildrenPage {
            items,
            next_cursor: page.next_cursor,
            total: page.total,
        }))
    }
}

impl Volume for RemoteVolume {
    fn children(
        &self,
        _image: &Arc<EvidenceSession>,
        id: &str,
        _opts: ChildrenOptions,
    ) -> Result<Vec<FsNode>> {
        let mut nodes = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            match self.children_page_remote(id, cursor.as_deref(), DIRECTORY_PAGE_SIZE as usize)? {
                Some(page) => {
                    nodes.extend(page.items);
                    if nodes.len() > MAX_DIRECTORY_ENTRIES {
                        anyhow::bail!(
                            "remote directory exceeds {MAX_DIRECTORY_ENTRIES} entry safety limit"
                        );
                    }
                    match page.next_cursor {
                        Some(next_cursor) => cursor = Some(next_cursor),
                        None => break,
                    }
                }
                None if cursor.is_none() => {
                    let opaque_id = self.opaque_node(id)?;
                    let result = self.call("fs.children", self.params(&opaque_id, &[]))?;
                    let wire_nodes: Vec<LppFsNodeV1> = rmpv::ext::from_value(result)
                        .map_err(|error| anyhow!("invalid fs.children result: {error}"))?;
                    nodes = wire_nodes
                        .into_iter()
                        .map(|node| node.into_host(id))
                        .collect::<Result<Vec<_>>>()?;
                    for node in &mut nodes {
                        node.id = self.encode_node(&node.id);
                    }
                    break;
                }
                None => anyhow::bail!("plugin pagination became unavailable after the first page"),
            }
        }
        Ok(nodes)
    }

    fn children_page(
        &self,
        _image: &Arc<EvidenceSession>,
        id: &str,
        _opts: ChildrenOptions,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Option<VolumeChildrenPage>> {
        self.children_page_remote(id, cursor, limit)
    }

    fn read_file(&self, id: &str, max_bytes: usize) -> Result<Vec<u8>> {
        self.read_bytes(id, 0, max_bytes)
    }

    fn file_stat(&self, id: &str, node_id: &str) -> Result<FsStat> {
        let opaque_id = self.opaque_node(id)?;
        let result = self.call("fs.stat", self.params(&opaque_id, &[]))?;
        let stat: LppFsStatV1 = rmpv::ext::from_value(result)
            .map_err(|error| anyhow!("invalid fs.stat result: {error}"))?;
        stat.into_host(node_id)
    }

    fn read_range(&self, id: &str, offset: u64, length: usize) -> Result<Option<Vec<u8>>> {
        self.read_bytes(id, offset, length).map(Some)
    }

    fn stream_file(
        &self,
        id: &str,
        chunk_size: usize,
        visitor: &mut dyn FnMut(&[u8]) -> Result<()>,
    ) -> Result<u64> {
        let opaque_id = self.opaque_node(id)?;
        let size = self.file_stat(id, id)?.size;
        let request_size = chunk_size.clamp(1, REMOTE_READ_CHUNK);
        let mut offset = 0_u64;
        while offset < size {
            let length = (size - offset).min(request_size as u64);
            let result = self.call(
                "fs.read_range",
                self.params(
                    &opaque_id,
                    &[
                        ("offset", Value::from(offset)),
                        ("length", Value::from(length)),
                    ],
                ),
            )?;
            let bytes = binary_result_with_limit(result, "fs.read_range", length)?;
            if bytes.is_empty() {
                anyhow::bail!("short remote file stream at {offset}, expected {size} bytes");
            }
            visitor(&bytes)?;
            offset = offset
                .checked_add(bytes.len() as u64)
                .context("remote file stream offset overflow")?;
        }
        Ok(offset)
    }
}

fn binary_result(value: Value, method: &str) -> Result<Vec<u8>> {
    match value {
        Value::Binary(bytes) => Ok(bytes),
        _ => Err(anyhow!("{method} result must be MessagePack binary")),
    }
}

fn binary_result_with_limit(value: Value, method: &str, maximum: u64) -> Result<Vec<u8>> {
    let bytes = binary_result(value, method)?;
    if bytes.len() as u64 > maximum {
        anyhow::bail!(
            "{method} returned {} bytes for a request limited to {maximum}",
            bytes.len()
        );
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::io::{duplex, split};

    use crate::core::extensions::plugin::{
        codec::{read_frame, write_frame},
        LppMessage, SourceRegistry,
    };

    use super::*;

    #[test]
    fn binary_result_rejects_more_than_requested() {
        assert!(binary_result_with_limit(Value::Binary(vec![0; 3]), "read", 2).is_err());
    }

    #[test]
    fn remote_node_ids_round_trip_opaque_unicode() {
        let id = format!(
            "plugin:{}:{}:{}",
            hex::encode("org.test"),
            hex::encode("会话:1"),
            hex::encode("inode:文件:5")
        );
        assert_eq!(
            decode_remote_node_id(&id).expect("decode node"),
            (
                "org.test".to_owned(),
                "会话:1".to_owned(),
                "inode:文件:5".to_owned()
            )
        );
    }

    #[test]
    fn remote_volume_follows_plugin_page_cursors() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let (host_stream, plugin_stream) = duplex(64 * 1024);
        let (host_reader, host_writer) = split(host_stream);
        let (mut plugin_reader, mut plugin_writer) = split(plugin_stream);
        runtime.spawn(async move {
            let hello = Value::Map(vec![
                (Value::from("protocol_min"), Value::from(1)),
                (Value::from("protocol_max"), Value::from(1)),
                (
                    Value::from("plugin"),
                    Value::Map(vec![
                        (Value::from("id"), Value::from("org.test.paged")),
                        (Value::from("name"), Value::from("Paged FS")),
                        (Value::from("version"), Value::from("1")),
                    ]),
                ),
                (
                    Value::from("capabilities"),
                    Value::Array(vec![Value::from("filesystem")]),
                ),
            ]);
            write_frame(
                &mut plugin_writer,
                &LppMessage::request(1, "plugin.hello", hello),
            )
            .await
            .expect("hello");
            read_frame(&mut plugin_reader).await.expect("host hello");

            for page_index in 0..2 {
                let LppMessage::Request {
                    id, method, params, ..
                } = read_frame(&mut plugin_reader).await.expect("page request")
                else {
                    panic!("expected request");
                };
                assert_eq!(method, "fs.children_page");
                let entries = params.as_map().expect("params map");
                let cursor = entries
                    .iter()
                    .find_map(|(key, value)| (key.as_str() == Some("cursor")).then_some(value))
                    .expect("cursor param");
                if page_index == 0 {
                    assert!(cursor.is_nil());
                } else {
                    assert_eq!(cursor.as_str(), Some("page:2"));
                }
                let node_number = page_index + 1;
                let result = Value::Map(vec![
                    (
                        Value::from("items"),
                        Value::Array(vec![Value::Map(vec![
                            (
                                Value::from("id"),
                                Value::from(format!("file:{node_number}")),
                            ),
                            (
                                Value::from("name"),
                                Value::from(format!("{node_number}.txt")),
                            ),
                            (Value::from("kind"), Value::from("file")),
                            (Value::from("size"), Value::from(1)),
                        ])]),
                    ),
                    (
                        Value::from("next_cursor"),
                        if page_index == 0 {
                            Value::from("page:2")
                        } else {
                            Value::Nil
                        },
                    ),
                    (Value::from("total"), Value::from(2)),
                ]);
                write_frame(&mut plugin_writer, &LppMessage::success(id, result))
                    .await
                    .expect("page response");
            }
        });
        let client = runtime
            .block_on(super::super::client::PluginClient::connect(
                Box::new(host_reader),
                Box::new(host_writer),
                super::super::client::HostRpcServices::new(
                    SourceRegistry::default(),
                    super::super::FileRegistry::default(),
                    super::super::EvidenceRegistry::default(),
                    [],
                ),
                None,
                None,
                None,
            ))
            .expect("client");
        let volume = RemoteVolume::new(
            PluginConnection::Process(client),
            "session:1".to_owned(),
            "root".to_owned(),
            "part:1".to_owned(),
            "source:1".to_owned(),
        );
        let temporary = tempfile::tempdir().expect("tempdir");
        std::fs::write(temporary.path().join("image.raw"), [0_u8; 1]).expect("image");
        let image = Arc::new(
            EvidenceSession::open(temporary.path().join("image.raw")).expect("open image"),
        );

        let children = volume
            .children(
                &image,
                "part:1",
                ChildrenOptions {
                    include_deleted_ntfs: true,
                    include_unallocated_nodes: true,
                },
            )
            .expect("paged children");
        assert_eq!(
            children
                .iter()
                .map(|node| node.name.as_str())
                .collect::<Vec<_>>(),
            vec!["1.txt", "2.txt"]
        );
    }

    #[test]
    fn remote_volume_lists_stats_reads_and_streams() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let (host_stream, plugin_stream) = duplex(64 * 1024);
        let (host_reader, host_writer) = split(host_stream);
        let (mut plugin_reader, mut plugin_writer) = split(plugin_stream);
        runtime.spawn(async move {
            let hello = Value::Map(vec![
                (Value::from("protocol_min"), Value::from(1)),
                (Value::from("protocol_max"), Value::from(1)),
                (
                    Value::from("plugin"),
                    Value::Map(vec![
                        (Value::from("id"), Value::from("org.test.fs")),
                        (Value::from("name"), Value::from("Test FS")),
                        (Value::from("version"), Value::from("1")),
                    ]),
                ),
                (
                    Value::from("capabilities"),
                    Value::Array(vec![Value::from("filesystem")]),
                ),
            ]);
            write_frame(
                &mut plugin_writer,
                &LppMessage::request(1, "plugin.hello", hello),
            )
            .await
            .expect("hello");
            read_frame(&mut plugin_reader).await.expect("host hello");
            for _ in 0..11 {
                let LppMessage::Request {
                    id, method, params, ..
                } = read_frame(&mut plugin_reader).await.expect("request")
                else {
                    panic!("expected request");
                };
                let result = match method.as_str() {
                    "fs.children_page" => Value::Map(vec![
                        (
                            Value::from("items"),
                            Value::Array(vec![Value::Map(vec![
                                (Value::from("id"), Value::from("file:1")),
                                (Value::from("name"), Value::from("hello.txt")),
                                (Value::from("parent_id"), Value::from("root")),
                                (Value::from("kind"), Value::from("file")),
                                (Value::from("size"), Value::from(5)),
                                (Value::from("inode"), Value::from(1)),
                            ])]),
                        ),
                        (Value::from("next_cursor"), Value::Nil),
                        (Value::from("total"), Value::from(1)),
                    ]),
                    "fs.stat" => Value::Map(vec![
                        (Value::from("node_id"), Value::from("file:1")),
                        (Value::from("size"), Value::from(5)),
                        (Value::from("inode"), Value::from(1)),
                        (Value::from("mime"), Value::from("text/plain")),
                    ]),
                    "fs.read_range" => {
                        let entries = params.as_map().expect("params map");
                        let number = |name: &str| {
                            entries
                                .iter()
                                .find_map(|(key, value)| {
                                    (key.as_str() == Some(name)).then(|| value.as_u64())
                                })
                                .flatten()
                                .expect("numeric param")
                        };
                        let data = b"hello";
                        let start = number("offset") as usize;
                        let end = start
                            .saturating_add(number("length") as usize)
                            .min(data.len());
                        Value::Binary(data[start.min(data.len())..end].to_vec())
                    }
                    _ => panic!("unexpected method {method}"),
                };
                write_frame(&mut plugin_writer, &LppMessage::success(id, result))
                    .await
                    .expect("response");
            }
        });
        let client = runtime
            .block_on(super::super::client::PluginClient::connect(
                Box::new(host_reader),
                Box::new(host_writer),
                super::super::client::HostRpcServices::new(
                    SourceRegistry::default(),
                    super::super::FileRegistry::default(),
                    super::super::EvidenceRegistry::default(),
                    [],
                ),
                None,
                None,
                None,
            ))
            .expect("client");
        let volume = RemoteVolume::new(
            PluginConnection::Process(client),
            "session:1".to_owned(),
            "root".to_owned(),
            "part:1".to_owned(),
            "source:1".to_owned(),
        );
        let temporary = tempfile::tempdir().expect("tempdir");
        std::fs::write(temporary.path().join("image.raw"), vec![0_u8; 4096]).expect("image");
        let image = Arc::new(
            EvidenceSession::open(temporary.path().join("image.raw")).expect("open image"),
        );

        let page = volume
            .children_page(
                &image,
                "part:1",
                ChildrenOptions {
                    include_deleted_ntfs: true,
                    include_unallocated_nodes: true,
                },
                None,
                1,
            )
            .expect("children page")
            .expect("native page");
        assert_eq!(page.items.len(), 1);
        assert!(page.next_cursor.is_none());

        let children = volume
            .children(
                &image,
                "part:1",
                ChildrenOptions {
                    include_deleted_ntfs: true,
                    include_unallocated_nodes: true,
                },
            )
            .expect("children");
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].parent_id.as_deref(), Some("part:1"));
        let node_id = children[0].id.clone();
        assert_eq!(volume.file_stat(&node_id, &node_id).expect("stat").size, 5);
        assert_eq!(volume.read_file(&node_id, 16).expect("read"), b"hello");
        assert_eq!(
            volume.read_range(&node_id, 3, 16).expect("range"),
            Some(b"lo".to_vec())
        );
        let mut streamed = Vec::new();
        assert_eq!(
            volume
                .stream_file(&node_id, 2, &mut |chunk| {
                    streamed.extend_from_slice(chunk);
                    Ok(())
                })
                .expect("stream"),
            5
        );
        assert_eq!(streamed, b"hello");
    }
}
