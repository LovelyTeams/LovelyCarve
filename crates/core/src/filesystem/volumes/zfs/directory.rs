//! Directory traversal and filesystem node projection.
use super::*;

impl ZfsVolume {
    /// Children of the DSL directory at `dataset_path`. Returns the DSL
    /// sub-datasets first, then (when the directory's head dataset is a ZPL
    /// filesystem) the ZPL root entries. Errors at the ZPL stage are
    /// downgraded to `tracing::warn!` so a single broken filesystem dataset
    /// doesn't blank the whole DSL listing.
    pub fn dsl_children(&self, dataset_path: &str) -> Result<Vec<FsNode>> {
        let parent_id = self.dsl_id(dataset_path);
        let drive = self.open_drive()?;
        let mos = drive
            .get_most_recent_mos()
            .map_err(|err| anyhow!("ZFS get_most_recent_mos: {err}"))?;
        let dsl_dir = walk_dsl_dir(&mos, dataset_path)
            .map_err(|err| anyhow!("ZFS dsl walk {dataset_path:?}: {err}"))?;

        let mut out: Vec<FsNode> = Vec::new();

        match dsl_dir.children() {
            Ok(child_list) => {
                for (zap_name, _child_dir) in child_list.into_iter().take(MAX_CHILDREN_PER_DIR) {
                    let name_str = zap_string_to_string(&zap_name);
                    let child_dataset = join_dataset(dataset_path, &name_str);
                    out.push(FsNode {
                        id: self.dsl_id(&child_dataset),
                        name: name_str,
                        parent_id: Some(parent_id.clone()),
                        kind: NodeKind::Dir,
                        size: 0,
                        deleted: false,
                        inode: None,
                        timestamps: Mactb::default(),
                        link_target: None,
                        has_snapshot: false,
                    });
                }
            }
            Err(err) => {
                // Pool-root or pure-container DSL dirs sometimes have no
                // child_dir_zapobj; treat as empty rather than failing.
                tracing::warn!("ZFS dsl children {dataset_path:?}: {err}");
            }
        }

        // Try to surface the ZPL root entries if this dataset is a
        // filesystem. Encryption / unsupported-feature errors land here.
        if let Some(entries) = self.try_zpl_root_entries(&mos, dataset_path, &parent_id) {
            out.extend(entries);
        }

        sort_dirs_first_then_name(&mut out);
        Ok(out)
    }

    /// Children of a ZPL directory at `file_path` inside the filesystem
    /// dataset at `dataset_path`. `file_path` of `""` denotes the ZPL root.
    pub fn zpl_children(&self, dataset_path: &str, file_path: &str) -> Result<Vec<FsNode>> {
        let parent_id = self.zpl_id(dataset_path, file_path);
        let drive = self.open_drive()?;
        let mos = drive
            .get_most_recent_mos()
            .map_err(|err| anyhow!("ZFS get_most_recent_mos: {err}"))?;
        let dsl_dir = walk_dsl_dir(&mos, dataset_path)
            .map_err(|err| anyhow!("ZFS dsl walk {dataset_path:?}: {err}"))?;
        let dataset = dsl_dir
            .get_head_dataset()
            .map_err(|err| anyhow!("ZFS head dataset {dataset_path:?}: {err}"))?;
        let os = dataset
            .get_object_set()
            .map_err(|err| anyhow!("ZFS object_set {dataset_path:?}: {err}"))?;
        if os.os.os_type != OsType::ZFS {
            bail!("ZFS dataset {dataset_path} is not a ZPL filesystem");
        }
        let zpl = os
            .as_zpl()
            .ok_or_else(|| anyhow!("ZFS dataset {dataset_path} is not a ZPL filesystem"))?;
        let dir = walk_zpl_dir(&zpl, file_path)?;
        let entries = dir
            .children()
            .map_err(|err| anyhow!("ZFS zpl children {file_path:?}: {err}"))?;

        let mut out = Vec::with_capacity(entries.len().min(MAX_CHILDREN_PER_DIR));
        for (zap_name, entry) in entries.into_iter().take(MAX_CHILDREN_PER_DIR) {
            let name_str = zap_string_to_string(&zap_name);
            let child_path = join_unix(file_path, &name_str);
            let (kind, size, timestamps) = match &entry {
                ZfsDirEntry::Dir(_) => (NodeKind::Dir, 0u64, Mactb::default()),
                ZfsDirEntry::File(file) => (
                    NodeKind::File,
                    zpl_file_size(file).unwrap_or(0),
                    zpl_file_timestamps(file),
                ),
            };
            out.push(FsNode {
                id: self.zpl_id(dataset_path, &child_path),
                name: name_str,
                parent_id: Some(parent_id.clone()),
                kind,
                size,
                deleted: false,
                inode: None,
                timestamps,
                link_target: None,
                has_snapshot: false,
            });
        }
        sort_dirs_first_then_name(&mut out);
        Ok(out)
    }

    /// Look up the ZPL root of the head dataset at `dataset_path` and emit
    /// its top-level entries as `zpl:` `FsNode`s. Returns `None` if any step
    /// of the chain fails (no head dataset, not a ZPL filesystem, encryption
    /// enabled, etc.) — those errors are non-fatal because the DSL children
    /// were already collected.
    pub(super) fn try_zpl_root_entries(
        &self,
        mos: &ZfsMetaObjectSet<'_, PartitionDrive>,
        dataset_path: &str,
        parent_id: &str,
    ) -> Option<Vec<FsNode>> {
        // Pool root has no head dataset of its own.
        if dataset_path.is_empty() {
            return None;
        }
        let dsl_dir = match walk_dsl_dir(mos, dataset_path) {
            Ok(d) => d,
            Err(err) => {
                tracing::warn!("ZFS try_zpl walk_dsl {dataset_path:?}: {err}");
                return None;
            }
        };
        let dataset = match dsl_dir.get_head_dataset() {
            Ok(d) => d,
            Err(err) => {
                tracing::warn!("ZFS try_zpl head_dataset {dataset_path:?}: {err}");
                return None;
            }
        };
        let os = match dataset.get_object_set() {
            Ok(o) => o,
            Err(err) => {
                tracing::warn!("ZFS try_zpl object_set {dataset_path:?}: {err}");
                return None;
            }
        };
        if os.os.os_type != OsType::ZFS {
            return None;
        }
        let zpl = os.as_zpl()?;
        let zpl_root = match zpl.get_root() {
            Ok(r) => r,
            Err(err) => {
                tracing::warn!("ZFS try_zpl get_root {dataset_path:?}: {err}");
                return None;
            }
        };
        let entries = match zpl_root.children() {
            Ok(c) => c,
            Err(err) => {
                tracing::warn!("ZFS try_zpl children {dataset_path:?}: {err}");
                return None;
            }
        };

        let mut out = Vec::with_capacity(entries.len().min(MAX_CHILDREN_PER_DIR));
        for (zap_name, entry) in entries.into_iter().take(MAX_CHILDREN_PER_DIR) {
            let name_str = zap_string_to_string(&zap_name);
            let file_path = format!("/{name_str}");
            let (kind, size, timestamps) = match &entry {
                ZfsDirEntry::Dir(_) => (NodeKind::Dir, 0u64, Mactb::default()),
                ZfsDirEntry::File(file) => (
                    NodeKind::File,
                    zpl_file_size(file).unwrap_or(0),
                    zpl_file_timestamps(file),
                ),
            };
            out.push(FsNode {
                id: self.zpl_id(dataset_path, &file_path),
                name: name_str,
                parent_id: Some(parent_id.to_string()),
                kind,
                size,
                deleted: false,
                inode: None,
                timestamps,
                link_target: None,
                has_snapshot: false,
            });
        }
        Some(out)
    }
}

/// Walk a DSL path inside the MOS by chasing `child_dir_zapobj`. Returns
/// the leaf `ZfsDslDir` — borrows from `mos`, so the caller must keep both
/// the drive and MOS alive.
pub(super) fn walk_dsl_dir<'drive, 'mos>(
    mos: &'mos ZfsMetaObjectSet<'drive, PartitionDrive>,
    dataset_path: &str,
) -> Result<ZfsDslDir<'drive, 'mos, PartitionDrive>> {
    let mut current = mos
        .get_root_dir()
        .map_err(|err| anyhow!("ZFS get_root_dir: {err}"))?;
    for component in dataset_path.split('/').filter(|s| !s.is_empty()) {
        let key = ZapString::from_byte_slice(component.as_bytes())
            .ok_or_else(|| anyhow!("dataset component {component:?} contains a NUL byte"))?;
        let next = current
            .get_child(&key)
            .map_err(|err| anyhow!("ZFS dsl get_child {component}: {err}"))?
            .ok_or_else(|| anyhow!("ZFS dataset component not found: {component}"))?;
        current = next;
    }
    Ok(current)
}

/// Walk a `/`-rooted UTF-8 path inside a ZPL filesystem. Empty `file_path`
/// returns the filesystem root. The returned dir borrows from `zpl`.
pub(super) fn walk_zpl_dir<'drive, 'fs>(
    zpl: &'fs ZfsZPLObjectSet<'drive, PartitionDrive>,
    file_path: &str,
) -> Result<ZfsZPLDir<'drive, 'fs, PartitionDrive>> {
    let mut current = zpl
        .get_root()
        .map_err(|err| anyhow!("ZFS zpl get_root: {err}"))?;
    for component in file_path.split('/').filter(|s| !s.is_empty()) {
        let key = ZapString::from_byte_slice(component.as_bytes())
            .ok_or_else(|| anyhow!("path component {component:?} contains a NUL byte"))?;
        let entry = current
            .get_child(&key)
            .map_err(|err| anyhow!("ZFS zpl get_child {component}: {err}"))?
            .ok_or_else(|| anyhow!("ZFS path component not found: {component}"))?;
        current = match entry {
            ZfsDirEntry::Dir(d) => d,
            ZfsDirEntry::File(_) => {
                bail!("ZFS path component {component} is a regular file, expected directory")
            }
        };
    }
    Ok(current)
}

pub(super) fn join_dataset(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

pub(super) fn join_unix(parent: &str, name: &str) -> String {
    if parent.is_empty() || parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

pub(super) fn sort_dirs_first_then_name(out: &mut [FsNode]) {
    out.sort_by(|a, b| {
        let ak = u8::from(a.kind != NodeKind::Dir);
        let bk = u8::from(b.kind != NodeKind::Dir);
        ak.cmp(&bk)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}
