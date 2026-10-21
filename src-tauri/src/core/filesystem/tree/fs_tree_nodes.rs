use std::sync::Arc;

use crate::core::evidence::image::{Partition, PartitionKind};
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::tree::fs_tree::{FsNode, Mactb, NodeKind};
use crate::core::filesystem::unalloc::{self, VolumeFree};
use crate::core::filesystem::volumes::ext4::{Ext4Source, Ext4Volume};
use crate::core::filesystem::volumes::fat::FatVolume;
use crate::core::filesystem::volumes::fat_deleted::open_geom;
use crate::core::filesystem::volumes::hfsplus::HfsPlusVolume;
use crate::core::filesystem::volumes::ufs::UfsVolume;

/// FAT on-disk metadata structures surfaced as readable virtual files at the
/// partition root: `$DBR` (the
/// boot sector / DOS Boot Record at sector 0), `$FAT1`, and `$FAT2` (the FAT
/// tables). Backed by the `sysfile:` byte-range scheme so reads / hex / stat
/// land straight on the image. Empty for non-FAT partitions or when the
/// geometry cannot be parsed.
pub(crate) fn fat_system_nodes(image: &Arc<EvidenceSession>, partition: &Partition) -> Vec<FsNode> {
    if !matches!(
        partition.kind,
        PartitionKind::Fat32 | PartitionKind::Fat12 | PartitionKind::Fat16
    ) {
        return Vec::new();
    }
    let base = partition.start_lba * image.sector_size as u64;
    let Ok(geom) = open_geom(image, base) else {
        return Vec::new();
    };
    let bps = u64::from(geom.bytes_per_sector);
    let fat_len = u64::from(geom.sectors_per_fat) * bps;
    let fat1_off = base + u64::from(geom.reserved_sectors) * bps;
    let part_id = partition.id;
    let make = |name: &str, offset: u64, len: u64| FsNode {
        id: format!("sysfile:part:{part_id}:0x{offset:x}:0x{len:x}"),
        name: name.to_string(),
        parent_id: Some(format!("part:{part_id}")),
        kind: NodeKind::File,
        size: len,
        deleted: false,
        inode: None,
        timestamps: Mactb::default(),
        link_target: None,
        has_snapshot: false,
    };
    let mut nodes = vec![make("$DBR", base, bps), make("$FAT1", fat1_off, fat_len)];
    if geom.num_fats >= 2 {
        nodes.push(make("$FAT2", fat1_off + fat_len, fat_len));
    }
    nodes
}

/// Filesystem-level "$未分配簇" virtual files for `partition`: the clusters
/// the filesystem does not allocate to any file, surfaced as sibling nodes to
/// the real files.
///
/// The per-kind decision is exhaustive so a new [`PartitionKind`] forces a
/// choice here: produce nodes (and how precisely) or skip. Kinds with no
/// physical unallocated concept (LVM — handled at VG level; BitLocker — locked
/// container; read-only SquashFS/CDFS; logical Extended/aggregate; the
/// non-filesystem markers) return an empty list.
pub(crate) fn fs_unalloc_nodes(image: &Arc<EvidenceSession>, partition: &Partition) -> Vec<FsNode> {
    let sector_size = image.sector_size as u64;
    let part_offset = partition.start_lba * sector_size;
    let part_len = partition.length_lba * sector_size;

    // Per-kind precision: filesystems whose free count / allocation map we can
    // read cheaply report it exactly; the rest fall back to `WholeVolume` (one
    // approximate node spanning the partition). Kinds that should never carry
    // an FS-level unallocated node return `None`. A volume that fails to open
    // or report degrades to `WholeVolume` rather than vanishing.
    let free: Option<VolumeFree> = match partition.kind {
        PartitionKind::HfsPlus | PartitionKind::HfsX => Some(
            HfsPlusVolume::open(Arc::clone(image), partition)
                .and_then(|vol| vol.free_bytes())
                .map(VolumeFree::FreeBytes)
                .unwrap_or(VolumeFree::WholeVolume),
        ),
        PartitionKind::Ufs => Some(
            UfsVolume::open(Arc::clone(image), partition)
                .and_then(|vol| vol.free_bytes())
                .map(VolumeFree::FreeBytes)
                .unwrap_or(VolumeFree::WholeVolume),
        ),
        PartitionKind::Fat32 | PartitionKind::Fat12 | PartitionKind::Fat16 => Some(
            FatVolume::open(Arc::clone(image), partition)
                .and_then(|vol| vol.free_bytes())
                .map(VolumeFree::FreeBytes)
                .unwrap_or(VolumeFree::WholeVolume),
        ),
        PartitionKind::Ext4 => Some(
            Ext4Volume::open(
                Arc::clone(image),
                Ext4Source::Partition(partition.id),
                part_offset,
                part_len,
                format!("part:{}", partition.id),
            )
            .and_then(|vol| vol.free_bytes())
            .map(VolumeFree::FreeBytes)
            .unwrap_or(VolumeFree::WholeVolume),
        ),
        // Still approximate (no cheap free map exposed yet — phase 3 continues):
        PartitionKind::Btrfs
        | PartitionKind::ExFat
        | PartitionKind::ApfsContainer
        | PartitionKind::Xfs
        | PartitionKind::Zfs
        | PartitionKind::Ntfs
        | PartitionKind::Refs => Some(VolumeFree::WholeVolume),
        // No FS-level unallocated node:
        // - Lvm: free PE space is surfaced at the VG level (`vg_unalloc_nodes`)
        // - Bitlocker: locked container; its inner NTFS handles its own once unlocked
        // - Cdfs / Squashfs: read-only packed images, no allocation pool
        // - Extended / Apm / Bsd / Mdadm / Luks / Swap / BiosBoot: containers /
        //   non-filesystem regions with no per-file allocation to complement
        // - Unknown: unrecognised, nothing reliable to compute
        PartitionKind::Lvm
        | PartitionKind::Bitlocker
        | PartitionKind::Cdfs
        | PartitionKind::Squashfs
        | PartitionKind::Ubi
        | PartitionKind::Extended
        | PartitionKind::Apm
        | PartitionKind::Bsd
        | PartitionKind::Mdadm
        | PartitionKind::Filevault
        | PartitionKind::Luks
        | PartitionKind::Swap
        | PartitionKind::BiosBoot
        | PartitionKind::Unknown => None,
    };

    let Some(free) = free else {
        return Vec::new();
    };
    fs_unalloc_nodes_from_free(partition.id, part_offset, part_len, free)
}

/// Turn a [`VolumeFree`] report into one or more "$未分配簇" nodes. Exact
/// (`Allocated`) reports yield one node per free segment; coarse reports yield
/// a single node spanning the partition with the best `size` available.
fn fs_unalloc_nodes_from_free(
    part_id: u8,
    part_offset: u64,
    part_len: u64,
    free: VolumeFree,
) -> Vec<FsNode> {
    let make = |offset: u64, len: u64, size: u64, multi: Option<usize>| {
        let name = match multi {
            Some(idx) => format!("$未分配簇{}", idx + 1),
            None => "$未分配簇".to_string(),
        };
        FsNode {
            id: format!("unalloc:part:{part_id}:0x{offset:x}:0x{len:x}"),
            name,
            parent_id: Some(format!("part:{part_id}")),
            kind: NodeKind::File,
            size,
            deleted: false,
            inode: None,
            timestamps: Mactb::default(),
            link_target: None,
            has_snapshot: false,
        }
    };

    match free {
        VolumeFree::Allocated(occupied) => {
            let segments =
                unalloc::free_intervals(part_offset, part_len, &occupied, unalloc::MIN_DISK_GAP);
            let multi = segments.len() > 1;
            segments
                .into_iter()
                .enumerate()
                .map(|(idx, (offset, len))| make(offset, len, len, multi.then_some(idx)))
                .collect()
        }
        // Exact free total, unknown layout: one node over the whole partition,
        // but `size` reports the true free byte count.
        VolumeFree::FreeBytes(free_bytes) => {
            vec![make(part_offset, part_len, free_bytes, None)]
        }
        // No bitmap: one node over the whole partition; `size` is the whole
        // volume (an upper bound — the name is left plain, the size is honest
        // about being the partition span rather than a measured free count).
        VolumeFree::WholeVolume => vec![make(part_offset, part_len, part_len, None)],
    }
}

/// Build virtual nodes for gaps between detected partitions (and the image
/// tail). Pure interval arithmetic over the partition table — no filesystem
/// parsing — so it works on every image format. Gaps shorter than
/// [`unalloc::MIN_DISK_GAP`] are dropped so partition-alignment slivers and the
/// GPT backup header don't add noise nodes.
pub(crate) fn disk_gap_nodes(image: &Arc<EvidenceSession>) -> Vec<FsNode> {
    let sector_size = image.sector_size as u64;
    let occupied: Vec<(u64, u64)> = image
        .partitions
        .iter()
        .map(|partition| {
            (
                partition.start_lba * sector_size,
                partition.length_lba * sector_size,
            )
        })
        .collect();
    unalloc::free_intervals(0, image.len(), &occupied, unalloc::MIN_DISK_GAP)
        .into_iter()
        .enumerate()
        .map(|(idx, (offset, len))| FsNode {
            id: format!("diskgap:0x{offset:x}:0x{len:x}"),
            name: format!("未使用空间{}", idx + 1),
            parent_id: None,
            kind: NodeKind::File,
            size: len,
            deleted: false,
            inode: None,
            timestamps: Mactb::default(),
            link_target: None,
            has_snapshot: false,
        })
        .collect()
}

pub(crate) fn partition_node(image: &Arc<EvidenceSession>, partition: &Partition) -> FsNode {
    let supported = matches!(
        partition.kind,
        PartitionKind::Ext4
            | PartitionKind::Btrfs
            | PartitionKind::Fat32
            | PartitionKind::Fat12
            | PartitionKind::Fat16
            | PartitionKind::ExFat
            | PartitionKind::Cdfs
            | PartitionKind::Squashfs
            | PartitionKind::Ubi
            | PartitionKind::ApfsContainer
            | PartitionKind::HfsPlus
            | PartitionKind::HfsX
            | PartitionKind::Lvm
            | PartitionKind::Ntfs
            | PartitionKind::Refs
            | PartitionKind::Ufs
            | PartitionKind::Xfs
            | PartitionKind::Zfs
            | PartitionKind::Bitlocker
            // Unknown volumes remain expandable so the LPP manager can lazily
            // probe external filesystem parsers when the user opens them.
            | PartitionKind::Unknown
    );
    FsNode {
        id: if supported {
            format!("part:{}", partition.id)
        } else {
            format!("unsupported:{}", partition.id)
        },
        name: crate::core::common::navigation_name::volume_name(
            &partition
                .label
                .clone()
                .unwrap_or_else(|| partition_name(partition)),
        ),
        parent_id: None,
        kind: if supported {
            NodeKind::Dir
        } else {
            NodeKind::Unallocated
        },
        size: partition.length_lba * image.sector_size as u64,
        deleted: false,
        // Recovered (signature-scanned) volumes carry no partition-table
        // entry; flag them so the UI distinguishes them from table-backed
        // partitions (P1-13).
        inode: None,
        timestamps: Mactb::default(),
        link_target: None,
        has_snapshot: false,
    }
}

fn partition_name(partition: &Partition) -> String {
    // 导航名只留序号：文件系统类型和几何（起始 LBA 等）在分区列表卡片与
    // 详情面板里都有，放进名字只会拉长面包屑和全路径。GPT 卷标存在时
    // 优先用卷标（见 partition_node）。
    format!("分区{}", partition.id)
}
