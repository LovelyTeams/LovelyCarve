//! Open-handle contexts, directory lookup and shared caches.
use super::*;
use crate::filesystem::evidence_session::EvidenceSession;

#[derive(Clone)]
pub(super) struct OpenContext {
    pub(super) node: FsNode,
    pub(super) cache: Arc<Mutex<Option<ChunkCache>>>,
}

/// Sliding window of bytes from the open file. Dokan typically reads in 64 KiB
/// chunks; the underlying `stream_file_chunks` always starts at offset 0, so
/// caching `DOKAN_STREAM_CHUNK` (1 MiB) here amortises ~16 reads per re-stream.
/// Random access to multi-GiB files is still suboptimal (we don't have a true
/// positional read across every adapter), but sequential reads — the common
/// case for hashing, exporting, and AV/forensic scans — drop from O(N²) to
/// O(N²/window).
pub(super) struct ChunkCache {
    pub(super) start: u64,
    pub(super) data: Vec<u8>,
}

/// Path → node and dir → children caches for the mount. Both are pure
/// performance caches — a miss simply re-resolves the path from the root
/// (`resolve`) or re-lists the directory (`children_for`), so bounding them
/// with LRU eviction is always correctness-safe. Without the bound, browsing
/// a large NTFS system disk through the mount grew these maps without limit
/// (one entry per path/dir ever touched), leaking gigabytes over a session.
pub(super) struct PathCache {
    pub(super) nodes: LruCache<String, FsNode>,
    pub(super) children: LruCache<String, Vec<FsNode>>,
}

/// Upper bound on cached path → node entries. ~50k single-line FsNodes is a
/// few tens of MiB at most; well above any working set a user browses at once.
pub(super) const PATH_NODE_CACHE_CAP: usize = 50_000;
/// Directory-listing cache holds whole child vectors, so it's capped lower —
/// the hot set is the ancestor chain plus recently opened folders.
pub(super) const PATH_CHILDREN_CACHE_CAP: usize = 4_096;

pub(super) struct LovelyCarveDokanFs {
    pub(super) image: Arc<EvidenceSession>,
    pub(super) root: FsNode,
    pub(super) root_node_id: String,
    pub(super) volume_name: String,
    pub(super) fs_name: String,
    pub(super) fs_kind: Option<PartitionKind>,
    /// The case-sensitivity decision made at mount time, mirroring the
    /// `MountFlags::CASE_SENSITIVE` flag actually passed to Dokan. Carried here
    /// so `resolve` matches paths with the SAME policy the volume advertises —
    /// recomputing it from `fs_kind` alone is wrong for synthetic roots
    /// (docker / host-docker / tar), whose `fs_kind` is `None` but which mount
    /// case-sensitive.
    pub(super) case_sensitive: bool,
    pub(super) cache: Mutex<PathCache>,
}

impl LovelyCarveDokanFs {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        image: Arc<EvidenceSession>,
        root: FsNode,
        root_node_id: String,
        volume_name: String,
        fs_name: String,
        fs_kind: Option<PartitionKind>,
        case_sensitive: bool,
    ) -> Self {
        let mut nodes =
            LruCache::new(NonZeroUsize::new(PATH_NODE_CACHE_CAP).unwrap_or(NonZeroUsize::MIN));
        nodes.put("\\".to_string(), root.clone());
        Self {
            image,
            root,
            root_node_id,
            volume_name,
            fs_name,
            fs_kind,
            case_sensitive,
            cache: Mutex::new(PathCache {
                nodes,
                children: LruCache::new(
                    NonZeroUsize::new(PATH_CHILDREN_CACHE_CAP).unwrap_or(NonZeroUsize::MIN),
                ),
            }),
        }
    }

    pub(super) fn resolve(&self, raw_path: &U16CStr) -> OperationResult<FsNode> {
        let path = normalize_dokan_path(&raw_path.to_string_lossy());
        if let Some(node) = self.cache.lock().nodes.get(&path).cloned() {
            return Ok(node);
        }

        // Match paths with the SAME case policy the volume advertises to Dokan
        // (computed once at mount time), not a re-derivation from `fs_kind` —
        // which is `None` for synthetic docker / tar roots even though those
        // mount CASE_SENSITIVE, so the old recompute resolved them
        // case-insensitively and could serve a case-colliding sibling's bytes.
        let case_sensitive = self.case_sensitive;
        let mut current_path = "\\".to_string();
        let mut current = self.root.clone();
        let mut session = fs_tree::FsSession::new(&self.image);
        for part in path_components(&path) {
            let children = self.children_for_with_session(&mut session, &current_path, &current)?;
            let Some(child) = children.into_iter().find(|node| {
                let display = sanitize_for_windows(&node.name);
                if case_sensitive {
                    display.as_ref() == part
                } else {
                    display.eq_ignore_ascii_case(&part)
                }
            }) else {
                return Err(STATUS_NO_SUCH_FILE);
            };
            current_path =
                join_mount_path(&current_path, sanitize_for_windows(&child.name).as_ref());
            self.cache
                .lock()
                .nodes
                .put(current_path.clone(), child.clone());
            current = child;
        }
        Ok(current)
    }

    pub(super) fn children_for(&self, path: &str, node: &FsNode) -> OperationResult<Vec<FsNode>> {
        let mut session = fs_tree::FsSession::new(&self.image);
        self.children_for_with_session(&mut session, path, node)
    }

    pub(super) fn children_for_with_session(
        &self,
        session: &mut fs_tree::FsSession<'_>,
        path: &str,
        node: &FsNode,
    ) -> OperationResult<Vec<FsNode>> {
        if node.kind != NodeKind::Dir {
            return Err(STATUS_NOT_A_DIRECTORY);
        }
        if let Some(children) = self.cache.lock().children.get(path).cloned() {
            return Ok(children);
        }
        let children = session.children_for_mount(&node.id).map_err(to_status)?;
        {
            let mut cache = self.cache.lock();
            cache.children.put(path.to_string(), children.clone());
            for child in &children {
                cache.nodes.put(
                    join_mount_path(path, sanitize_for_windows(&child.name).as_ref()),
                    child.clone(),
                );
            }
        }
        Ok(children)
    }

    pub(super) fn file_info(&self, node: &FsNode) -> FileInfo {
        file_info_for_node(node)
    }
}
