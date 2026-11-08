//! Lifecycle contract for volumes provided outside the native FS readers.

use super::fs_dispatch::Volume;
use super::{
    evidence_session::EvidenceSession,
    fs_dispatch::VolumeChildrenPage,
    tree::fs_tree::{ChildrenOptions, FsNode, FsStat},
};
use anyhow::{anyhow, Context as _, Result};
use std::sync::Arc;
use std::{fmt::Debug, future::Future, pin::Pin};

/// A read-only provider-owned volume installed in an evidence session.
/// Implementers preserve their backing connection while calls hold a handle,
/// recognize only their own node namespace, and close asynchronously during
/// image unload. The registry drains handles before awaiting close; closing
/// must not unload native code while an outstanding call is still running.
pub trait ExternalVolume: Volume + Debug + Send + Sync {
    fn host_root(&self) -> &str;
    fn owns_node(&self, node_id: &str) -> bool;
    fn is_alive(&self) -> bool;
    fn source_id(&self) -> &str;
    fn close(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>>;
}

impl<T: Volume + ?Sized> Volume for Arc<T> {
    fn children(
        &self,
        image: &Arc<EvidenceSession>,
        id: &str,
        options: ChildrenOptions,
    ) -> anyhow::Result<Vec<FsNode>> {
        (**self).children(image, id, options)
    }

    fn children_page(
        &self,
        image: &Arc<EvidenceSession>,
        id: &str,
        options: ChildrenOptions,
        cursor: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<Option<VolumeChildrenPage>> {
        (**self).children_page(image, id, options, cursor, limit)
    }

    fn read_file(&self, id: &str, maximum: usize) -> anyhow::Result<Vec<u8>> {
        (**self).read_file(id, maximum)
    }

    fn read_range(&self, id: &str, offset: u64, length: usize) -> anyhow::Result<Option<Vec<u8>>> {
        (**self).read_range(id, offset, length)
    }

    fn file_stat(&self, id: &str, node_id: &str) -> anyhow::Result<FsStat> {
        (**self).file_stat(id, node_id)
    }

    fn stream_file(
        &self,
        id: &str,
        chunk_size: usize,
        visitor: &mut dyn FnMut(&[u8]) -> anyhow::Result<()>,
    ) -> anyhow::Result<u64> {
        (**self).stream_file(id, chunk_size, visitor)
    }
}

pub(crate) const REMOTE_NODE_PREFIX: &str = "plugin:";

pub(crate) fn decode_remote_node_id(id: &str) -> Result<(String, String, String)> {
    let encoded = id
        .strip_prefix(REMOTE_NODE_PREFIX)
        .ok_or_else(|| anyhow!("not a remote plugin node"))?;
    let mut parts = encoded.split(':');
    let decode = |part: Option<&str>, field: &str| -> Result<String> {
        let bytes = hex::decode(part.ok_or_else(|| anyhow!("missing remote {field}"))?)
            .with_context(|| format!("invalid remote {field} encoding"))?;
        String::from_utf8(bytes).with_context(|| format!("remote {field} is not UTF-8"))
    };
    let plugin_id = decode(parts.next(), "plugin id")?;
    let session_id = decode(parts.next(), "session id")?;
    let node_id = decode(parts.next(), "node id")?;
    if parts.next().is_some() {
        anyhow::bail!("remote node id has trailing fields");
    }
    Ok((plugin_id, session_id, node_id))
}
