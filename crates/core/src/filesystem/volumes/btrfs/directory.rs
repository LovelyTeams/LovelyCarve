//! Directory traversal and filesystem node projection.
use super::*;

impl<'a> BtrfsVolume<'a> {
    /// List `(subvol_id, optional_name)` for every subvolume the open()
    /// pass discovered. The default subvolume (`FS_TREE_OBJECTID`, 5) is
    /// always present.
    pub fn list_subvolumes(&self) -> Vec<(u64, Option<String>)> {
        self.subvolumes
            .iter()
            .map(|(id, entry)| (*id, entry.name.clone()))
            .collect()
    }

    /// Children shown when the Btrfs partition/LV itself is expanded. Preserve
    /// the historical default-subvolume root listing, then add independent
    /// subvolumes that are not already reachable via a ROOT_REF from that
    /// default tree (for example standalone snapshot roots).
    pub fn partition_children(&self) -> anyhow::Result<Vec<FsNode>> {
        let mut nodes = self.children(self.default_subvol, self.root_dir_inode)?;
        nodes.extend(self.independent_subvol_root_nodes());
        sort_dirs_first_then_name(&mut nodes);
        Ok(nodes)
    }

    /// Root nodes for the subvolumes that are NOT reachable via a ROOT_REF
    /// from the default tree (standalone snapshot roots) — exactly the extra
    /// entries [`Self::partition_children`] appends after the default
    /// subvolume's own listing. Split out so the snapshot fast path can
    /// reproduce the partition root byte-for-byte.
    pub(crate) fn independent_subvol_root_nodes(&self) -> Vec<FsNode> {
        self.subvolumes
            .iter()
            .filter(|&(&subvol, entry)| {
                should_list_subvol_at_partition_root(
                    subvol,
                    self.default_subvol,
                    entry,
                    &self.subvolumes,
                )
            })
            .map(|(&subvol, entry)| self.root_subvolume_node(subvol, entry))
            .collect()
    }

    pub(super) fn subvol_root(&self, subvol: u64) -> anyhow::Result<(u64, u8)> {
        let entry = self
            .subvolumes
            .get(&subvol)
            .ok_or_else(|| anyhow!("btrfs: subvolume {subvol} unknown"))?;
        Ok((entry.bytenr, entry.level))
    }

    pub fn children(&self, subvol: u64, parent: u64) -> anyhow::Result<Vec<FsNode>> {
        let entries = self.dir_entries(subvol, parent)?;
        let mut nodes = Vec::with_capacity(entries.len());
        for entry in entries {
            match entry.target() {
                DirEntryTarget::Inode(inode) => match self.read_inode(subvol, inode) {
                    Ok(item) => nodes.push(self.node_from_inode(
                        subvol,
                        parent,
                        inode,
                        &entry.name,
                        entry.ftype,
                        &item,
                    )),
                    Err(err) => tracing::debug!(
                        subvol,
                        parent,
                        child = inode,
                        name = %entry.name,
                        error = %err,
                        "btrfs: skipping unresolvable dir entry"
                    ),
                },
                DirEntryTarget::Subvol(child_subvol) => {
                    match self.read_inode(child_subvol, FIRST_FREE_OBJECTID) {
                        Ok(item) => nodes.push(self.node_for_subvol(
                            subvol,
                            parent,
                            child_subvol,
                            &entry.name,
                            &item,
                        )),
                        Err(err) => tracing::debug!(
                            parent_subvol = subvol,
                            parent,
                            child_subvol,
                            name = %entry.name,
                            error = %err,
                            "btrfs: subvolume target unreadable; surfacing as placeholder"
                        ),
                    }
                }
                DirEntryTarget::Unsupported(location_key_type) => tracing::debug!(
                    subvol,
                    parent,
                    name = %entry.name,
                    location_key_type,
                    "btrfs: skipping dir entry with unsupported location key type"
                ),
            }
        }
        sort_dirs_first_then_name(&mut nodes);
        Ok(nodes)
    }

    pub(super) fn dir_entries(&self, subvol: u64, parent: u64) -> anyhow::Result<Vec<DirEntry>> {
        let mut entries = Vec::new();
        let (root_bytenr, root_level) = self.subvol_root(subvol)?;
        let walker = self.walker();
        walker.iter_range(
            root_bytenr,
            root_level,
            &Key {
                objectid: parent.into(),
                key_type: KEY_DIR_INDEX,
                offset: 0u64.into(),
            },
            &Key {
                objectid: parent.into(),
                key_type: KEY_DIR_INDEX,
                offset: u64::MAX.into(),
            },
            |key, data| {
                if key.objectid.get() != parent || key.key_type != KEY_DIR_INDEX {
                    return Ok(false);
                }
                if let Some(entry) = parse_dir_item(data) {
                    entries.push(entry);
                }
                Ok(true)
            },
        )?;
        Ok(entries)
    }

    pub fn node_from_inode(
        &self,
        subvol: u64,
        parent: u64,
        inode: u64,
        name: &str,
        ftype: u8,
        item: &BtrfsInode,
    ) -> FsNode {
        let kind = match ftype {
            FT_DIRECTORY => NodeKind::Dir,
            FT_REGULAR => NodeKind::File,
            FT_SYMLINK => NodeKind::Symlink,
            _ => item.kind(),
        };
        FsNode {
            id: self.btrfs_id(subvol, inode),
            name: name.to_string(),
            parent_id: Some(self.parent_id_for(subvol, parent)),
            kind,
            size: item.size,
            deleted: false,
            inode: Some(inode),
            timestamps: Mactb {
                modified: format_ts(item.mtime),
                accessed: format_ts(item.atime),
                created: format_ts(item.ctime),
                mft_changed: None,
                birth: format_ts(item.otime),
            },
            link_target: None,
            has_snapshot: false,
        }
    }

    pub(crate) fn node_for_subvol(
        &self,
        parent_subvol: u64,
        parent: u64,
        child_subvol: u64,
        name: &str,
        item: &BtrfsInode,
    ) -> FsNode {
        FsNode {
            id: self.btrfs_id(child_subvol, FIRST_FREE_OBJECTID),
            name: name.to_string(),
            parent_id: Some(self.parent_id_for(parent_subvol, parent)),
            kind: NodeKind::Dir,
            size: item.size,
            deleted: false,
            inode: Some(FIRST_FREE_OBJECTID),
            timestamps: Mactb {
                modified: format_ts(item.mtime),
                accessed: format_ts(item.atime),
                created: format_ts(item.ctime),
                mft_changed: None,
                birth: format_ts(item.otime),
            },
            link_target: None,
            has_snapshot: false,
        }
    }

    pub(super) fn root_subvolume_node(&self, subvol: u64, entry: &SubvolEntry) -> FsNode {
        match self.read_inode(subvol, FIRST_FREE_OBJECTID) {
            Ok(item) => self.subvolume_node(
                subvol,
                self.root_parent_id.clone(),
                root_subvolume_name(subvol, entry),
                &item,
            ),
            Err(err) => {
                tracing::debug!(
                    subvol,
                    error = %err,
                    "btrfs: independent subvolume root unreadable; surfacing placeholder"
                );
                FsNode {
                    id: self.btrfs_id(subvol, FIRST_FREE_OBJECTID),
                    name: root_subvolume_name(subvol, entry),
                    parent_id: Some(self.root_parent_id.clone()),
                    kind: NodeKind::Dir,
                    size: 0,
                    deleted: false,
                    inode: Some(FIRST_FREE_OBJECTID),
                    timestamps: Mactb::default(),
                    link_target: None,
                    has_snapshot: false,
                }
            }
        }
    }

    pub(super) fn subvolume_node(
        &self,
        subvol: u64,
        parent_id: String,
        name: String,
        item: &BtrfsInode,
    ) -> FsNode {
        FsNode {
            id: self.btrfs_id(subvol, FIRST_FREE_OBJECTID),
            name,
            parent_id: Some(parent_id),
            kind: NodeKind::Dir,
            size: item.size,
            deleted: false,
            inode: Some(FIRST_FREE_OBJECTID),
            timestamps: Mactb {
                modified: format_ts(item.mtime),
                accessed: format_ts(item.atime),
                created: format_ts(item.ctime),
                mft_changed: None,
                birth: format_ts(item.otime),
            },
            link_target: None,
            has_snapshot: false,
        }
    }

    pub(super) fn btrfs_id(&self, subvol: u64, inode: u64) -> String {
        match self.source {
            BtrfsSource::Partition(part) => format!("btrfs:part:{part}:{subvol}:{inode}"),
            BtrfsSource::Lvm { part_id, lv_index } => {
                format!("btrfs:lvm:{part_id}:{lv_index}:{subvol}:{inode}")
            }
        }
    }

    pub(super) fn parent_id_for(&self, subvol: u64, inode: u64) -> String {
        if subvol == self.default_subvol && inode == self.root_dir_inode {
            self.root_parent_id.clone()
        } else {
            self.btrfs_id(subvol, inode)
        }
    }
}
