//! navigation implementation.
use super::*;
use crate::core::filesystem::evidence_session::EvidenceSession;

pub fn children_for_image(
    image: &Arc<EvidenceSession>,
    parent_id: &str,
) -> anyhow::Result<Vec<FsNode>> {
    children_for_image_with_options(
        image,
        parent_id,
        ChildrenOptions {
            include_deleted_ntfs: true,
            include_unallocated_nodes: true,
        },
    )
}

/// Directory listing profile for read-only mount projections. It deliberately
/// skips heavyweight forensic-only augmentations so Windows shell probes do
/// not trip Dokan's per-request timeout before the mount becomes usable.
pub fn children_for_mount(
    image: &Arc<EvidenceSession>,
    parent_id: &str,
) -> anyhow::Result<Vec<FsNode>> {
    children_for_image_with_options(
        image,
        parent_id,
        ChildrenOptions {
            include_deleted_ntfs: false,
            include_unallocated_nodes: false,
        },
    )
}

pub(super) fn children_for_image_with_options(
    image: &Arc<EvidenceSession>,
    parent_id: &str,
    options: ChildrenOptions,
) -> anyhow::Result<Vec<FsNode>> {
    let image = image.filesystem_session(parent_id);
    if let Some(roots) = image.indexed_roots() {
        if parent_id.trim().is_empty() || parent_id == "root" {
            return Ok(roots
                .iter()
                .filter(|node| node.id != "root")
                .cloned()
                .collect());
        }
        anyhow::bail!("此目录未收录在已有索引中，源镜像不可用。请恢复源镜像后重试。");
    }
    // Docker / OCI archives short-circuit the partition list. The whole
    // image surfaces as a single synthetic root; descending into that
    // root walks the merged container filesystem.
    if image.docker.is_some() {
        if parent_id.trim().is_empty() || parent_id == "root" {
            return Ok(docker_volume::root_children(image));
        }
        if let Some(path) = docker_volume::parse_docker_id(parent_id) {
            return docker_volume::children(image, &path);
        }
    }

    // Folder evidence has the same shape as docker: one synthetic root
    // node hides the partition list, descending into it walks the on-disk
    // tree.
    if image.folder.is_some() {
        if parent_id.trim().is_empty() || parent_id == "root" {
            return Ok(folder_volume::root_children(image));
        }
        if let Some(rel) = folder_volume::parse_folder_id(parent_id) {
            return folder_volume::children(image, &rel);
        }
    }

    // Plain tar evidence: same single-synthetic-root shape as docker / folder,
    // descending walks the archive's merged file tree.
    if image.tar.is_some() {
        if parent_id.trim().is_empty() || parent_id == "root" {
            return Ok(tar_volume::root_children(image));
        }
        if let Some(rel) = tar_volume::parse_tar_id(parent_id) {
            return tar_volume::children(image, &rel);
        }
    }

    // Zip evidence: identical single-synthetic-root shape, descending walks the
    // archive's merged file tree (members decompressed on read).
    if image.zip.is_some() {
        if parent_id.trim().is_empty() || parent_id == "root" {
            return Ok(zip_volume::root_children(image));
        }
        if let Some(rel) = zip_volume::parse_zip_id(parent_id) {
            return zip_volume::children(image, &rel);
        }
    }

    if image.sevenz.is_some() {
        if parent_id.trim().is_empty() || parent_id == "root" {
            return Ok(sevenz_volume::root_children(image));
        }
        if let Some(rel) = sevenz_volume::parse_sevenz_id(parent_id) {
            return sevenz_volume::children(image, &rel);
        }
    }

    // AD1 logical image: same single synthetic root over the item tree.
    if image.ad1.is_some() {
        if parent_id.trim().is_empty() || parent_id == "root" {
            return Ok(ad1_volume::root_children(image));
        }
        if let Some(id) = ad1_volume::parse_ad1_id(parent_id) {
            return ad1_volume::children(image, id);
        }
    }

    // Ordinary Linux disk images may contain a Docker Engine data root. Its
    // containers are exposed as a synthetic peer of the physical partitions;
    // descending into one walks the lazily-composed overlay2 filesystem.
    if let Some(id) = host_docker::parse_host_docker_id(parent_id) {
        return host_docker::children(image, &id);
    }

    if parent_id.trim().is_empty() || parent_id == "root" {
        let mut nodes: Vec<FsNode> = image
            .info()
            .partitions
            .iter()
            .map(|partition| partition_node(image, partition))
            .collect();
        // Surface the gaps between/around partitions (and the image tail) as
        // virtual "未使用空间N" files alongside the partitions.
        if options.include_unallocated_nodes {
            nodes.extend(disk_gap_nodes(image));
        }
        if let Some(docker_node) = host_docker::root_node(image) {
            nodes.push(docker_node);
        }
        return Ok(nodes);
    }

    if let Some(part_id) = parent_id.strip_prefix("part:") {
        let partition = find_partition(image, part_id)?;
        let children = match partition.kind {
            PartitionKind::Ext4 => Ext4Volume::open(
                Arc::clone(image),
                Ext4Source::Partition(partition.id),
                partition.start_lba * image.sector_size as u64,
                partition.length_lba * image.sector_size as u64,
                format!("part:{}", partition.id),
            )?
            .children(2, true),
            PartitionKind::Btrfs => {
                let volume = open_btrfs_source(image, BtrfsSource::Partition(partition.id))?;
                volume.partition_children()
            }
            PartitionKind::Fat32 | PartitionKind::Fat12 | PartitionKind::Fat16 => {
                let volume = FatVolume::open(Arc::clone(image), partition)?;
                // Surface root-directory `0xE5` deleted entries inline, the way
                // `ntfs_children` does, so files deleted from the still-allocated
                // root directory remain visible.
                if options.include_deleted_ntfs {
                    volume.children_with_deleted("")
                } else {
                    volume.root_children()
                }
            }
            PartitionKind::ExFat => {
                let volume = ExfatVolume::open(Arc::clone(image), partition)?;
                if options.include_deleted_ntfs {
                    volume.children_with_deleted("")
                } else {
                    volume.root_children()
                }
            }
            PartitionKind::Cdfs => CdfsVolume::open(Arc::clone(image), partition)?.root_children(),
            PartitionKind::Squashfs => {
                SquashfsVolume::open(Arc::clone(image), partition)?.root_children()
            }
            PartitionKind::Ubi => crate::core::filesystem::volumes::ubi::UbiVolume::open(
                Arc::clone(image),
                partition,
            )?
            .root_children(),
            PartitionKind::ApfsContainer => apfs_volume_nodes(Arc::clone(image), partition),
            PartitionKind::HfsPlus | PartitionKind::HfsX => {
                HfsPlusVolume::open(Arc::clone(image), partition)?.root_children()
            }
            PartitionKind::Ufs => UfsVolume::open(Arc::clone(image), partition)?.root_children(),
            PartitionKind::Xfs => {
                XfsVolume::open_partition(Arc::clone(image), partition)?.root_children()
            }
            PartitionKind::Zfs => {
                ZfsVolume::open(Arc::clone(image), partition)?.partition_children()
            }
            PartitionKind::Lvm => {
                // LVM has no filesystem of its own; its unallocated space is
                // the VG-level PE gap, appended here, not the FS-level node.
                let lvm = LvmVolume::open(image, partition)?;
                let mut nodes = lvm.logical_volume_nodes()?;
                nodes.extend(lvm.vg_unalloc_nodes());
                Ok(nodes)
            }
            PartitionKind::Ntfs => ntfs_root_children(image, partition, options),
            PartitionKind::Refs => RefsVolume::open(Arc::clone(image), partition)?.root_children(),
            PartitionKind::Bitlocker => bitlocker_partition_children(image, partition, options),
            PartitionKind::Unknown => match image.external_volume(parent_id) {
                Some(volume) => volume.children(image, parent_id, options),
                None => Ok(Vec::new()),
            },
            _ => Ok(Vec::new()),
        };
        // Append a "$未分配簇" virtual file for the partition's unallocated
        // space (a filesystem-level free-cluster node, sibling to the real
        // files). `fs_unalloc_nodes` returns empty for kinds with no physical
        // unallocated concept (LVM/Bitlocker/read-only/aggregate), so the
        // exhaustive decision lives in one place.
        let mut children = children?;
        if options.include_unallocated_nodes {
            // FAT on-disk structures ($DBR / $FAT1 / $FAT2) as readable virtual
            // files. Empty for non-FAT kinds.
            children.extend(fat_system_nodes(image, partition));
            children.extend(fs_unalloc_nodes(image, partition));
        }
        return Ok(children);
    }

    // An LVM logical-volume node (`lvm:N:M`): routing only — expand to the
    // nested filesystem's root. Kept explicit (not a registry adapter) because
    // the nested FS is chosen per `lv.kind`.
    if let Some((part_id, lv_index)) = parse_lvm_id(parent_id) {
        let partition = find_partition(image, part_id)?;
        let lvm = LvmVolume::open(image, partition)?;
        let lv = lvm
            .logical_volumes
            .get(lv_index)
            .ok_or_else(|| anyhow::anyhow!("logical volume not found: {parent_id}"))?;
        match lv.kind {
            PartitionKind::Ext4 => {
                let children = Ext4Volume::open(
                    Arc::clone(image),
                    Ext4Source::Lvm {
                        part_id: partition.id,
                        lv_index,
                    },
                    lv.absolute_offset,
                    lv.size_bytes,
                    format!("lvm:{}:{lv_index}", partition.id),
                )?
                .children(2, true)?;
                return Ok(children);
            }
            PartitionKind::Xfs => {
                return open_xfs_source(
                    image,
                    XfsSource::Lvm {
                        part_id: partition.id,
                        lv_index,
                    },
                )?
                .root_children();
            }
            PartitionKind::Btrfs => {
                let volume = open_btrfs_source(
                    image,
                    BtrfsSource::Lvm {
                        part_id: partition.id,
                        lv_index,
                    },
                )?;
                return volume.partition_children();
            }
            PartitionKind::Ntfs => {
                return ntfs_lvm_root_children(image, partition, lv_index, options);
            }
            _ => return Ok(Vec::new()),
        }
    }

    // Every concrete filesystem subtree (ntfs / fat / exfat / refs / cdfs /
    // squashfs / apfs / hfsplus / ufs / xfs / zfs / btrfs / ext4) is served by
    // its registry adapter. The NTFS deleted-records snapshot, fat/exfat
    // deleted-aware listing, and zfs DSL/ZPL split all live inside the adapters.
    if let Some(volume) = crate::core::filesystem::fs_dispatch::dispatch(image, parent_id)? {
        return volume.children(image, parent_id, options);
    }

    Ok(Vec::new())
}
