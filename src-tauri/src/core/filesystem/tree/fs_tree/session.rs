//! Filesystem session session operations.
use super::*;
use crate::core::filesystem::evidence_session::EvidenceSession;

impl<'a> FsSession<'a> {
    pub fn new(image: &'a Arc<EvidenceSession>) -> Self {
        Self {
            image,
            volumes: HashMap::new(),
        }
    }

    /// The image this session reads from.
    pub(crate) fn image(&self) -> &'a Arc<EvidenceSession> {
        self.image
    }

    /// The cached [`Volume`] owning `id`, opened once per volume-root for the
    /// whole walk. `Ok(None)` for ids whose filesystem keeps no per-walk cache
    /// (the caller then falls back to the per-call open path).
    pub(crate) fn volume(
        &mut self,
        id: &str,
    ) -> anyhow::Result<Option<&(dyn crate::core::filesystem::fs_dispatch::Volume + 'a)>> {
        let Some(key) = crate::core::filesystem::fs_dispatch::volume_root_key(id) else {
            return Ok(None);
        };
        use std::collections::hash_map::Entry;
        match self.volumes.entry(key) {
            Entry::Occupied(slot) => Ok(Some(&**slot.into_mut())),
            Entry::Vacant(slot) => {
                match crate::core::filesystem::fs_dispatch::dispatch(self.image, id)? {
                    Some(volume) => Ok(Some(&**slot.insert(volume))),
                    None => Ok(None),
                }
            }
        }
    }

    /// Children of `parent_id`, identical to [`children_for_image`] but serving
    /// every subtree listing from a cached volume handle. Root listings
    /// (`part:N`, `lvm:N:M`) keep their explicit per-kind augmentation
    /// (`$未分配簇`) and open their volume once — only the deep
    /// subtree listings, the O(files) hot path, reuse the cache.
    pub fn children(&mut self, parent_id: &str) -> anyhow::Result<Vec<FsNode>> {
        self.children_with_options(
            parent_id,
            ChildrenOptions {
                include_deleted_ntfs: true,
                include_unallocated_nodes: true,
            },
        )
    }

    /// Directory listing profile for Dokan mount projections. It reuses this
    /// session's cached volume handles like [`Self::children`], but mirrors
    /// [`children_for_mount`] by skipping heavyweight forensic-only
    /// augmentations so Windows shell probes do not trigger deleted-record,
    /// or unallocated-space scans while resolving paths.
    pub fn children_for_mount(&mut self, parent_id: &str) -> anyhow::Result<Vec<FsNode>> {
        self.children_with_options(
            parent_id,
            ChildrenOptions {
                include_deleted_ntfs: false,
                include_unallocated_nodes: false,
            },
        )
    }

    pub(super) fn children_with_options(
        &mut self,
        parent_id: &str,
        options: ChildrenOptions,
    ) -> anyhow::Result<Vec<FsNode>> {
        if self.image.is_index_only() {
            return children_for_image_with_options(self.image, parent_id, options);
        }
        // Listing options for cached subtree dispatch: deleted NTFS / exFAT
        // entries follow the caller's profile; unallocated nodes are
        // appended by the explicit root branches below, never by the per-FS
        // `children`.
        let subtree_options = ChildrenOptions {
            include_deleted_ntfs: options.include_deleted_ntfs,
            include_unallocated_nodes: false,
        };

        // A subtree node inside a cached volume ("ntfs:…", "ext4:…", "refs:…",
        // "ufs:…", "xfs:…", "cdfs:…", "squashfs:…", "exfat:…", "apfs:…",
        // "btrfs:…"). Each of these re-parses expensive on-disk metadata per
        // open in the generic dispatcher (the MFT + $UpCase, the ReFS block
        // map, UFS cylinder groups, the SquashFS tables, the exFAT FAT, APFS
        // OMAP B-trees, the XFS superblock, the btrfs chunk + root trees);
        // serve them from one cached open for the whole walk. The adapter
        // carries the NTFS deleted-records snapshot and the exFAT
        // deleted-aware listing. `image` is copied out first so calling the
        // borrowed volume doesn't conflict with the `&mut self` cache borrow.
        let image = self.image;
        if let Some(volume) = self.volume(parent_id)? {
            return volume.children(image, parent_id, subtree_options);
        }

        // An LVM logical-volume node whose filesystem is ext4: list its root
        // through the cached-session fast path.
        if let Some((part_id, lv_index)) = parse_lvm_id(parent_id) {
            if let Ok(partition) = find_partition(self.image, part_id) {
                if let Ok(lvm) = LvmVolume::open(self.image, partition) {
                    let lv_kind = lvm.logical_volumes.get(lv_index).map(|lv| lv.kind);
                    if lv_kind == Some(PartitionKind::Ext4) {
                        let source = Ext4Source::Lvm {
                            part_id: partition.id,
                            lv_index,
                        };
                        let children = open_ext4_source(self.image, source)?
                            .children(EXT4_ROOT_INODE, true)?;
                        return Ok(children);
                    }
                }
            }
        }

        // A plain ext4 partition root: list its root inode, then append the same
        // FS-level "$未分配簇" node the generic dispatcher does.
        if let Some(part_str) = parent_id.strip_prefix("part:") {
            let is_ext4 = find_partition(self.image, part_str)
                .map(|p| p.kind == PartitionKind::Ext4)
                .unwrap_or(false);
            if is_ext4 {
                let pid: u8 = part_str.parse()?;
                let source = Ext4Source::Partition(pid);
                let mut children =
                    open_ext4_source(self.image, source)?.children(EXT4_ROOT_INODE, true)?;
                if options.include_unallocated_nodes {
                    let partition = find_partition(self.image, part_str)?;
                    children.extend(fs_unalloc_nodes(self.image, partition));
                }
                return Ok(children);
            }
        }

        // A partition root whose filesystem is one of the cached path-addressed
        // kinds: list its root, then append "$未分配簇" exactly like the
        // generic dispatcher.
        if let Some(part_str) = parent_id.strip_prefix("part:") {
            let kind = find_partition(self.image, part_str).map(|p| p.kind).ok();
            let pid: Option<u8> = part_str.parse().ok();
            if let (Some(kind), Some(pid)) = (kind, pid) {
                let root = match kind {
                    PartitionKind::Ubifs => Some(
                        crate::core::filesystem::volumes::ubifs::open_ubifs_source(
                            self.image,
                            crate::core::filesystem::volumes::ubifs::UbifsSource::Partition(pid),
                        )?
                        .root_children(),
                    ),
                    PartitionKind::Cramfs => Some(
                        crate::core::filesystem::volumes::rom_images::open_cramfs_source(
                            self.image, pid,
                        )?
                        .root_children(),
                    ),
                    PartitionKind::Romfs => Some(
                        crate::core::filesystem::volumes::rom_images::open_romfs_source(
                            self.image, pid,
                        )?
                        .root_children(),
                    ),
                    PartitionKind::Refs => Some(
                        open_refs_source(self.image, RefsSource::Partition(pid))?.root_children(),
                    ),
                    PartitionKind::Ufs => Some(
                        open_ufs_source(self.image, UfsSource::Partition(pid))?.root_children(),
                    ),
                    PartitionKind::Xfs => Some(
                        open_xfs_source(self.image, XfsSource::Partition(pid))?.root_children(),
                    ),
                    PartitionKind::Cdfs => Some(
                        open_cdfs_source(self.image, CdfsSource::Partition(pid))?.root_children(),
                    ),
                    PartitionKind::Squashfs => Some(
                        open_squashfs_source(self.image, SquashfsSource::Partition(pid))?
                            .root_children(),
                    ),
                    PartitionKind::ExFat => Some(
                        open_exfat_source(self.image, ExfatSource::Partition(pid))?
                            .children_with_deleted(""),
                    ),
                    PartitionKind::ApfsContainer => Some(apfs_volume_nodes(
                        Arc::clone(self.image),
                        find_partition(self.image, part_str)?,
                    )),
                    _ => None,
                };
                if let Some(root) = root {
                    let mut children = root?;
                    if options.include_unallocated_nodes {
                        let partition = find_partition(self.image, part_str)?;
                        children.extend(fs_unalloc_nodes(self.image, partition));
                    }
                    return Ok(children);
                }
            }
        }
        children_for_image_with_options(self.image, parent_id, options)
    }
}
