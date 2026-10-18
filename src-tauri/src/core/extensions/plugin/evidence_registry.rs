//! 检材级只读文件视图,支持按名称、路径和扩展名定位文件。
//!
//! 检材级 function 拿到任务后自行定位根分区并按路径读取文件。
//! 本注册表为它们提供 LPP 通道:宿主在
//! `analyzer.analyze_evidence` 期间登记一条证据会话,插件通过
//! `evidence.roots/stat/list/read/readv` 以 POSIX 路径访问该检材的全部
//! 可浏览卷。访问总是 owner-scoped(按插件 id 校验),读取限制与
//! `file.read` 一致(单次 4 MiB)。路径逐组件解析,拒绝 `..` 越界,根
//! 只能是文件系统数组里的卷键(`part:N` / `lvm:N:M` / `root`)。

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use parking_lot::Mutex;
use serde::Serialize;

use crate::core::{
    filesystem::evidence_session::EvidenceSession,
    filesystem::fs_all::FsAll,
    filesystem::tree::fs_tree::{self, FsNode, FsSession, Mactb, NodeKind},
};

use super::evidence_directory_cache::DirectoryCache;
use super::{PluginError, Result};

pub const MAX_EVIDENCE_READ: usize = 4 * 1024 * 1024;
pub const MAX_EVIDENCE_READV_RANGES: usize = 64;
pub const MAX_EVIDENCE_READV_TOTAL: usize = super::source_registry::MAX_SOURCE_READV_BYTES;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceHandle {
    pub evidence_id: String,
    pub image_id: String,
}

/// 一个可浏览根(卷)的描述。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceRoot {
    pub root_id: String,
    pub fs_prefix: String,
    pub kind: String,
    pub size: u64,
}

/// 路径解析结果节点的稳定 DTO(与 LPP `FsNode` 字段口径一致)。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceNode {
    pub node_id: String,
    pub name: String,
    pub kind: &'static str,
    pub size: u64,
    pub deleted: bool,
    pub link_target: Option<String>,
    pub timestamps: Mactb,
}

/// A bounded directory reply for LPP's 8 MiB frame limit.
pub struct EvidencePage {
    pub nodes: Vec<EvidenceNode>,
    pub total: usize,
}

struct EvidenceEntry {
    owner_plugin_id: String,
    image_id: String,
    image: Arc<EvidenceSession>,
    volumes: FsAll,
    root_keys: HashSet<String>,
    directories: Arc<Mutex<DirectoryCache>>,
    name_scans: Arc<Mutex<HashMap<String, super::evidence_name_scan::NameScan>>>,
}

#[derive(Clone, Default)]
pub struct EvidenceRegistry {
    entries: Arc<Mutex<HashMap<String, EvidenceEntry>>>,
}

impl EvidenceRegistry {
    pub fn register(
        &self,
        owner_plugin_id: &str,
        image_id: String,
        image: Arc<EvidenceSession>,
    ) -> EvidenceHandle {
        let volumes = FsAll::for_image(&image);
        let root_keys = volumes
            .entries()
            .iter()
            .map(|entry| entry.volume_key.clone())
            .collect();
        let evidence_id = format!("evidence_{}", uuid::Uuid::new_v4().simple());
        self.entries.lock().insert(
            evidence_id.clone(),
            EvidenceEntry {
                owner_plugin_id: owner_plugin_id.to_owned(),
                image_id: image_id.clone(),
                image,
                volumes,
                root_keys,
                directories: Arc::new(Mutex::new(DirectoryCache::default())),
                name_scans: Arc::default(),
            },
        );
        EvidenceHandle {
            evidence_id,
            image_id,
        }
    }

    pub fn roots(&self, owner_plugin_id: &str, evidence_id: &str) -> Result<Vec<EvidenceRoot>> {
        let entry = self.entry(owner_plugin_id, evidence_id)?;
        Ok(entry
            .volumes
            .entries()
            .iter()
            .map(|volume| EvidenceRoot {
                root_id: volume.volume_key.clone(),
                fs_prefix: volume.fs_prefix.to_owned(),
                kind: format!("{:?}", volume.kind).to_lowercase(),
                size: volume.size,
            })
            .collect())
    }

    /// 路径定位;不存在返回 `None`(对应插件 SDK 的空查询结果)。
    pub fn stat(
        &self,
        owner_plugin_id: &str,
        evidence_id: &str,
        root_id: &str,
        path: &str,
    ) -> Result<Option<EvidenceNode>> {
        let entry = self.entry(owner_plugin_id, evidence_id)?;
        let mut session = FsSession::new(&entry.image);
        Ok(resolve_path(&mut session, &entry, root_id, path, true)?.map(evidence_node))
    }

    pub fn list(
        &self,
        owner_plugin_id: &str,
        evidence_id: &str,
        root_id: &str,
        path: &str,
    ) -> Result<Vec<EvidenceNode>> {
        Ok(self
            .directory_nodes(owner_plugin_id, evidence_id, root_id, path)?
            .iter()
            .cloned()
            .map(evidence_node)
            .collect())
    }

    pub fn list_page(
        &self,
        owner_plugin_id: &str,
        evidence_id: &str,
        root_id: &str,
        path: &str,
        skip: usize,
        limit: usize,
    ) -> Result<EvidencePage> {
        if limit == 0 || limit > 512 {
            return Err(PluginError::ResourceLimit(
                "evidence.list_page limit must be 1..=512".into(),
            ));
        }
        let children = self.directory_nodes(owner_plugin_id, evidence_id, root_id, path)?;
        if skip > children.len() {
            return Err(PluginError::Rpc {
                code: "INVALID_PARAMS".into(),
                message: "directory cursor exceeds total".into(),
            });
        }
        Ok(EvidencePage {
            total: children.len(),
            nodes: children
                .iter()
                .skip(skip)
                .take(limit)
                .cloned()
                .map(evidence_node)
                .collect(),
        })
    }

    fn directory_nodes(
        &self,
        owner_plugin_id: &str,
        evidence_id: &str,
        root_id: &str,
        path: &str,
    ) -> Result<Arc<Vec<FsNode>>> {
        let entry = self.entry(owner_plugin_id, evidence_id)?;
        let mut session = FsSession::new(&entry.image);
        let node = resolve_path(&mut session, &entry, root_id, path, true)?
            .ok_or_else(|| path_not_found(root_id, path))?;
        if node.kind != NodeKind::Dir {
            return Err(not_a_directory(root_id, path));
        }
        let children = directory_children(&mut session, &entry, &node.id).map_err(|error| {
            tracing::debug!(%evidence_id, %root_id, %path, "plugin evidence list failed: {error}");
            PluginError::Rpc {
                code: "EVIDENCE_LIST_FAILED".to_owned(),
                message: "logical evidence directory listing failed".to_owned(),
            }
        })?;
        Ok(children)
    }

    pub fn read(
        &self,
        owner_plugin_id: &str,
        evidence_id: &str,
        root_id: &str,
        path: &str,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>> {
        if length > MAX_EVIDENCE_READ {
            return Err(PluginError::ResourceLimit(format!(
                "evidence.read length {length} exceeds {MAX_EVIDENCE_READ}"
            )));
        }
        let entry = self.entry(owner_plugin_id, evidence_id)?;
        let mut session = FsSession::new(&entry.image);
        let node = resolve_path(&mut session, &entry, root_id, path, true)?
            .ok_or_else(|| path_not_found(root_id, path))?;
        if node.kind != NodeKind::File {
            return Err(not_a_file(root_id, path));
        }
        if offset >= node.size || length == 0 {
            return Ok(Vec::new());
        }
        let available = node.size - offset;
        let length = length.min(usize::try_from(available).unwrap_or(usize::MAX));
        let mut bytes = fs_tree::read_file_range(&entry.image, &node.id, offset, length)
            .map_err(|error| {
                tracing::debug!(%evidence_id, %root_id, %path, "plugin evidence read failed: {error}");
                PluginError::Rpc {
                    code: "EVIDENCE_READ_FAILED".to_owned(),
                    message: "logical evidence file read failed".to_owned(),
                }
            })?;
        bytes.truncate(length);
        Ok(bytes)
    }

    pub fn readv(
        &self,
        owner_plugin_id: &str,
        evidence_id: &str,
        root_id: &str,
        path: &str,
        ranges: &[(u64, usize)],
    ) -> Result<Vec<Vec<u8>>> {
        if ranges.len() > MAX_EVIDENCE_READV_RANGES {
            return Err(PluginError::ResourceLimit(format!(
                "evidence.readv has more than {MAX_EVIDENCE_READV_RANGES} ranges"
            )));
        }
        let total = ranges.iter().try_fold(0_usize, |total, (_, length)| {
            total.checked_add(*length).ok_or_else(|| {
                PluginError::ResourceLimit("evidence.readv total length overflowed".to_owned())
            })
        })?;
        if total > MAX_EVIDENCE_READV_TOTAL {
            return Err(PluginError::ResourceLimit(format!(
                "evidence.readv total length {total} exceeds {MAX_EVIDENCE_READV_TOTAL}"
            )));
        }
        ranges
            .iter()
            .map(|(offset, length)| {
                self.read(
                    owner_plugin_id,
                    evidence_id,
                    root_id,
                    path,
                    *offset,
                    *length,
                )
            })
            .collect()
    }

    pub(super) fn name_scan_page(
        &self,
        owner: &str,
        evidence: &str,
        root: &str,
        cursor: Option<&str>,
        filters: super::evidence_name_scan::Filters,
        cancellation: &tokio_util::sync::CancellationToken,
    ) -> Result<(super::evidence_name_scan::Page, Option<String>)> {
        let entry = self.entry(owner, evidence)?;
        if !entry.root_keys.contains(root) {
            return Err(root_unknown(root));
        }
        let mut scan = if let Some(cursor) = cursor {
            entry
                .name_scans
                .lock()
                .remove(cursor)
                .ok_or_else(|| PluginError::Rpc {
                    code: "EVIDENCE_SCAN_CURSOR_INVALID".into(),
                    message:
                        "scan cursor is absent, already consumed or belongs to another evidence"
                            .into(),
                })?
        } else {
            if entry.name_scans.lock().len() >= 8 {
                return Err(PluginError::ResourceLimit(
                    "evidence has 8 outstanding name scans".into(),
                ));
            }
            super::evidence_name_scan::NameScan::new(root.to_owned(), filters)
        };
        if scan.root != root {
            return Err(root_unknown(root));
        }
        let page = scan.page(&entry.image, cancellation)?;
        let next = if page.complete {
            None
        } else {
            let cursor = uuid::Uuid::new_v4().simple().to_string();
            let mut scans = entry.name_scans.lock();
            if scans.len() >= 8 {
                return Err(PluginError::ResourceLimit(
                    "evidence has 8 outstanding name scans".into(),
                ));
            }
            scans.insert(cursor.clone(), scan);
            Some(cursor)
        };
        Ok((page, next))
    }

    pub fn remove_evidence(&self, owner_plugin_id: &str, evidence_id: &str) {
        self.entries
            .lock()
            .retain(|key, entry| !(key == evidence_id && entry.owner_plugin_id == owner_plugin_id));
    }

    pub fn cleanup_plugin(&self, owner_plugin_id: &str) {
        self.entries
            .lock()
            .retain(|_, entry| entry.owner_plugin_id != owner_plugin_id);
    }

    pub fn cleanup_image(&self, image_id: &str) {
        self.entries
            .lock()
            .retain(|_, entry| entry.image_id != image_id);
    }

    fn entry(&self, owner_plugin_id: &str, evidence_id: &str) -> Result<EvidenceEntry> {
        self.entries
            .lock()
            .get(evidence_id)
            .filter(|entry| entry.owner_plugin_id == owner_plugin_id)
            .map(|entry| EvidenceEntry {
                owner_plugin_id: entry.owner_plugin_id.clone(),
                image_id: entry.image_id.clone(),
                image: Arc::clone(&entry.image),
                volumes: entry.volumes.clone(),
                root_keys: entry.root_keys.clone(),
                directories: Arc::clone(&entry.directories),
                name_scans: Arc::clone(&entry.name_scans),
            })
            .ok_or_else(invalid_evidence_handle)
    }
}

/// 符号链接跟随跳数上限(防环)。Linux 根分区里 `/etc/os-release`、
/// `/etc/localtime` 等关键文件都是链接(常见相对目标 `../usr/lib/...`)。
const MAX_LINK_HOPS: usize = 16;

fn resolve_path(
    session: &mut FsSession<'_>,
    entry: &EvidenceEntry,
    root_id: &str,
    path: &str,
    follow_links: bool,
) -> Result<Option<FsNode>> {
    if !entry.root_keys.contains(root_id) {
        return Err(root_unknown(root_id));
    }
    let components: Vec<&str> = path
        .split('/')
        .filter(|component| !component.is_empty() && *component != ".")
        .collect();
    if components.is_empty() {
        // 空路径 = 卷根本身(便于 `evidence.list` 直接列根)。
        return Ok(Some(FsNode {
            id: root_id.to_owned(),
            name: root_id.to_owned(),
            parent_id: None,
            kind: NodeKind::Dir,
            size: 0,
            deleted: false,
            inode: None,
            timestamps: Default::default(),
            link_target: None,
            has_snapshot: false,
        }));
    }
    let mut queue: std::collections::VecDeque<String> = components
        .iter()
        .map(|component| (*component).to_owned())
        .collect();
    let mut stack: Vec<FsNode> = Vec::new();
    let mut link_hops = 0_usize;
    while let Some(component) = queue.pop_front() {
        if component == ".." {
            if stack.pop().is_none() {
                return Err(PluginError::Rpc {
                    code: "EVIDENCE_PATH_INVALID".to_owned(),
                    message: "evidence paths must stay inside the selected root".to_owned(),
                });
            }
            continue;
        }
        let dir_id = stack
            .last()
            .map(|node| node.id.clone())
            .unwrap_or_else(|| root_id.to_owned());
        let children = directory_children(session, entry, &dir_id).map_err(|error| {
            tracing::debug!(%root_id, %path, "plugin evidence path resolve failed: {error}");
            PluginError::Rpc {
                code: "EVIDENCE_LIST_FAILED".to_owned(),
                message: "logical evidence directory listing failed".to_owned(),
            }
        })?;
        let Some(found) = children
            .iter()
            .find(|node| node.name == component)
            .or_else(|| {
                children
                    .iter()
                    .find(|node| node.name.eq_ignore_ascii_case(component.as_str()))
            })
            .cloned()
        else {
            return Ok(None);
        };
        if follow_links
            && found.kind == NodeKind::Symlink
            && link_hops < MAX_LINK_HOPS
            && found.link_target.is_some()
        {
            let target = found.link_target.clone().unwrap_or_default();
            link_hops += 1;
            let mut parts: std::collections::VecDeque<String> = target
                .split('/')
                .filter(|part| !part.is_empty())
                .map(|part| part.to_owned())
                .collect();
            if target.starts_with('/') {
                stack.clear();
            }
            parts.extend(queue);
            queue = parts;
            continue;
        }
        stack.push(found);
    }
    Ok(stack.pop())
}

fn evidence_node(node: FsNode) -> EvidenceNode {
    EvidenceNode {
        node_id: node.id,
        name: node.name,
        kind: node_kind_name(node.kind),
        size: node.size,
        deleted: node.deleted,
        link_target: node.link_target,
        timestamps: node.timestamps,
    }
}

fn directory_children(
    session: &mut FsSession<'_>,
    entry: &EvidenceEntry,
    directory: &str,
) -> Result<Arc<Vec<FsNode>>> {
    if let Some(nodes) = entry.directories.lock().get(directory) {
        return Ok(nodes);
    }
    let nodes = Arc::new(
        session
            .children(directory)
            .map_err(|error| PluginError::Rpc {
                code: "EVIDENCE_LIST_FAILED".to_owned(),
                message: error.to_string(),
            })?,
    );
    entry
        .directories
        .lock()
        .insert(directory.to_owned(), Arc::clone(&nodes));
    Ok(nodes)
}

fn node_kind_name(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Dir => "dir",
        NodeKind::File => "file",
        NodeKind::Symlink => "link",
        NodeKind::Unallocated => "unallocated",
    }
}

fn invalid_evidence_handle() -> PluginError {
    PluginError::Rpc {
        code: "INVALID_EVIDENCE_HANDLE".to_owned(),
        message: "evidence handle is not available to this plugin".to_owned(),
    }
}

fn root_unknown(root_id: &str) -> PluginError {
    PluginError::Rpc {
        code: "EVIDENCE_ROOT_UNKNOWN".to_owned(),
        message: format!("root {root_id} is not a browsable volume of this evidence"),
    }
}

fn path_not_found(root_id: &str, path: &str) -> PluginError {
    PluginError::Rpc {
        code: "EVIDENCE_PATH_NOT_FOUND".to_owned(),
        message: format!("{path} does not exist under {root_id}"),
    }
}

fn not_a_directory(root_id: &str, path: &str) -> PluginError {
    PluginError::Rpc {
        code: "EVIDENCE_NOT_A_DIRECTORY".to_owned(),
        message: format!("{path} under {root_id} is not a directory"),
    }
}

fn not_a_file(root_id: &str, path: &str) -> PluginError {
    PluginError::Rpc {
        code: "EVIDENCE_NOT_A_FILE".to_owned(),
        message: format!("{path} under {root_id} is not a regular file"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::filesystem::fs_all;
    use std::fs;

    fn folder_evidence() -> (tempfile::TempDir, Arc<EvidenceSession>) {
        let temporary = tempfile::tempdir().expect("temp directory");
        let root = temporary.path().join("evidence-root");
        fs::create_dir_all(root.join("etc")).expect("create etc");
        fs::write(
            root.join("etc/passwd"),
            b"root:x:0:0:root:/root:/bin/bash\n",
        )
        .expect("passwd");
        fs::write(root.join("etc/hostname"), b"forensic-box\n").expect("hostname");
        let image = Arc::new(EvidenceSession::open(&root).expect("open folder image"));
        (temporary, image)
    }

    #[test]
    fn native_name_scan_cursors_preserve_paths_owners_and_cleanup() {
        use super::super::evidence_name_scan::Filters;
        use lovelycarve_plugin::{value, Value};
        let (temporary, _initial_image) = folder_evidence();
        for index in 0..140 {
            let directory = temporary
                .path()
                .join("evidence-root")
                .join(format!("dir-{index:03}"));
            fs::create_dir(&directory).unwrap();
            fs::write(directory.join("SYSTEM"), b"test").unwrap();
        }
        let params = value::map([
            ("names", Value::Array(vec!["system".into()])),
            ("prefixes", Value::Array(vec![])),
            ("suffixes", Value::Array(vec![])),
        ]);
        let cancellation = tokio_util::sync::CancellationToken::new();
        let filters = || Filters::from_value(&params).unwrap();
        let image =
            Arc::new(EvidenceSession::open(temporary.path().join("evidence-root")).unwrap());
        let registry = EvidenceRegistry::default();
        let handle = registry.register("org.test", "image-1".into(), image);
        let root = "root";
        assert!(registry
            .name_scan_page(
                "org.other",
                &handle.evidence_id,
                root,
                None,
                filters(),
                &cancellation
            )
            .is_err());
        assert!(registry
            .name_scan_page(
                "org.test",
                &handle.evidence_id,
                "part:0",
                None,
                filters(),
                &cancellation
            )
            .is_err());
        let (first, next) = registry
            .name_scan_page(
                "org.test",
                &handle.evidence_id,
                root,
                None,
                filters(),
                &cancellation,
            )
            .unwrap();
        assert!(!first.complete);
        let cursor = next.unwrap();
        assert!(registry
            .name_scan_page(
                "org.other",
                &handle.evidence_id,
                root,
                Some(&cursor),
                filters(),
                &cancellation
            )
            .is_err());
        let (second, mut next) = registry
            .name_scan_page(
                "org.test",
                &handle.evidence_id,
                root,
                Some(&cursor),
                filters(),
                &cancellation,
            )
            .unwrap();
        assert!(registry
            .name_scan_page(
                "org.test",
                &handle.evidence_id,
                root,
                Some(&cursor),
                filters(),
                &cancellation
            )
            .is_err());
        let mut paths = first.paths;
        paths.extend(second.paths);
        while let Some(cursor) = next {
            let (page, following) = registry
                .name_scan_page(
                    "org.test",
                    &handle.evidence_id,
                    root,
                    Some(&cursor),
                    filters(),
                    &cancellation,
                )
                .unwrap();
            paths.extend(page.paths);
            next = following;
        }
        paths.sort();
        assert_eq!(
            paths,
            (0..140)
                .map(|index| format!("evidence-root/dir-{index:03}/SYSTEM"))
                .collect::<Vec<_>>()
        );
        let cancelled = tokio_util::sync::CancellationToken::new();
        cancelled.cancel();
        assert!(matches!(
            registry.name_scan_page(
                "org.test", &handle.evidence_id, root, None, filters(), &cancelled
            ),
            Err(PluginError::Rpc { code, .. }) if code == "CANCELLED"
        ));
        let (_, next) = registry
            .name_scan_page(
                "org.test",
                &handle.evidence_id,
                root,
                None,
                filters(),
                &cancellation,
            )
            .unwrap();
        registry.cleanup_image("image-1");
        assert!(registry
            .name_scan_page(
                "org.test",
                &handle.evidence_id,
                root,
                next.as_deref(),
                filters(),
                &cancellation
            )
            .is_err());
    }

    #[test]
    fn paged_listing_is_complete_bounded_and_owner_scoped() {
        let (temporary, image) = folder_evidence();
        for index in 0..1027 {
            fs::write(
                temporary
                    .path()
                    .join("evidence-root")
                    .join(format!("item-{index}")),
                b"",
            )
            .unwrap();
        }
        let registry = EvidenceRegistry::default();
        let handle = registry.register("org.test", "image-1".into(), image);
        let root = registry.roots("org.test", &handle.evidence_id).unwrap()[0]
            .root_id
            .clone();
        let expected = registry
            .list("org.test", &handle.evidence_id, &root, "")
            .unwrap();
        let mut actual = Vec::new();
        while actual.len() < expected.len() {
            let page = registry
                .list_page(
                    "org.test",
                    &handle.evidence_id,
                    &root,
                    "",
                    actual.len(),
                    512,
                )
                .unwrap();
            assert_eq!(page.total, expected.len());
            assert!(page.nodes.len() <= 512);
            actual.extend(page.nodes);
        }
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
        assert!(registry
            .list_page("other", &handle.evidence_id, &root, "", 0, 512)
            .is_err());
        assert!(registry
            .list_page("org.test", &handle.evidence_id, &root, "../", 0, 512)
            .is_err());
        assert!(registry
            .list_page("org.test", &handle.evidence_id, &root, "", 0, 513)
            .is_err());
        assert!(registry
            .list_page(
                "org.test",
                &handle.evidence_id,
                &root,
                "",
                expected.len() + 1,
                1
            )
            .is_err());
    }

    #[test]
    fn roots_list_and_read_follow_folder_volume() {
        let (_temporary, image) = folder_evidence();
        let registry = EvidenceRegistry::default();
        let handle = registry.register("org.test", "image-1".to_owned(), image);
        let roots = registry
            .roots("org.test", &handle.evidence_id)
            .expect("roots");
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].root_id, fs_all::ROOT_VOLUME_KEY);

        // folder 证据在卷根下先有一层同名合成根节点,再进入实际目录树。
        let volume_children = registry
            .list("org.test", &handle.evidence_id, "root", "")
            .expect("list volume root");
        let synthetic_root = volume_children.first().expect("synthetic root node");
        assert_eq!(synthetic_root.kind, "dir");
        let base = format!("{}/", synthetic_root.name);
        let node = registry
            .stat(
                "org.test",
                &handle.evidence_id,
                "root",
                &format!("{base}etc/passwd"),
            )
            .expect("stat")
            .expect("found");
        assert_eq!(node.kind, "file");
        assert_eq!(
            registry
                .read(
                    "org.test",
                    &handle.evidence_id,
                    "root",
                    &format!("{base}etc/passwd"),
                    0,
                    4096
                )
                .expect("read"),
            b"root:x:0:0:root:/root:/bin/bash\n"
        );
        let listing = registry
            .list(
                "org.test",
                &handle.evidence_id,
                "root",
                &format!("{base}etc"),
            )
            .expect("list");
        let names: Vec<&str> = listing.iter().map(|node| node.name.as_str()).collect();
        assert!(names.contains(&"passwd"));
        assert!(names.contains(&"hostname"));
    }

    #[test]
    fn evidence_handles_are_owner_scoped_and_paths_validated() {
        let (_temporary, image) = folder_evidence();
        let registry = EvidenceRegistry::default();
        let handle = registry.register("org.test", "image-1".to_owned(), image);
        assert!(registry.roots("org.other", &handle.evidence_id).is_err());
        assert!(registry
            .stat("org.test", &handle.evidence_id, "part:0", "etc/passwd")
            .is_err());
        assert!(registry
            .read(
                "org.test",
                &handle.evidence_id,
                "root",
                "etc/../etc/passwd",
                0,
                4
            )
            .is_err());
        assert!(registry
            .stat("org.test", &handle.evidence_id, "root", "etc/missing")
            .expect("stat")
            .is_none());
        registry.remove_evidence("org.test", &handle.evidence_id);
        assert!(registry.roots("org.test", &handle.evidence_id).is_err());
    }

    /// 真镜像冒烟:`LOVELYCARVE_TEST_LINUX_IMAGE=<Linux 磁盘镜像>` 时运行,
    /// 验证分区根定位、etc/passwd 解析与 os-release 读取。
    #[test]
    #[ignore = "set LOVELYCARVE_TEST_LINUX_IMAGE to a Linux disk image to run"]
    fn resolves_linux_rootfs_on_real_image() {
        let path = std::env::var("LOVELYCARVE_TEST_LINUX_IMAGE").expect("test image env");
        let image =
            Arc::new(EvidenceSession::open(std::path::Path::new(&path)).expect("open image"));
        let registry = EvidenceRegistry::default();
        let handle = registry.register("org.test", "kali".to_owned(), image);
        let roots = registry
            .roots("org.test", &handle.evidence_id)
            .expect("roots");
        let rootfs = roots
            .iter()
            .map(|root| root.root_id.as_str())
            .find(|root_id| {
                registry
                    .stat("org.test", &handle.evidence_id, root_id, "etc/passwd")
                    .expect("stat")
                    .is_some()
            })
            .expect("no linux rootfs found among roots");
        let passwd = registry
            .read(
                "org.test",
                &handle.evidence_id,
                rootfs,
                "etc/passwd",
                0,
                4096,
            )
            .expect("read passwd");
        let text = String::from_utf8_lossy(&passwd);
        println!("passwd:\n{text}");
        if let Some(os_release) = registry
            .stat("org.test", &handle.evidence_id, rootfs, "etc/os-release")
            .expect("stat")
        {
            let bytes = registry
                .read(
                    "org.test",
                    &handle.evidence_id,
                    rootfs,
                    "etc/os-release",
                    0,
                    usize::try_from(os_release.size.min(4096)).unwrap_or(4096),
                )
                .expect("read os-release");
            println!("os-release:\n{}", String::from_utf8_lossy(&bytes));
        }
    }
}
