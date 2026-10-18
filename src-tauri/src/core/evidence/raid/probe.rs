//! Partition and filesystem description of assembled devices.
use super::*;

/// Identity and size of the filesystem an assembled device carries.
#[derive(Debug, Clone)]
pub(super) struct AssembledFilesystem {
    pub(super) kind: PartitionKind,
    pub(super) fsid_hex: Option<String>,
    pub(super) bytes_used: Option<u64>,
    pub(super) total_bytes: Option<u64>,
    pub(super) num_devices: Option<u64>,
    pub(super) sector_size: Option<u32>,
    pub(super) node_size: Option<u32>,
}

impl AssembledFilesystem {
    pub(super) fn record(&self, set_uuid: &str) -> RaidFilesystem {
        RaidFilesystem {
            set_uuid: set_uuid.to_string(),
            kind: kind_label(self.kind),
            fsid: self.fsid_hex.clone(),
            total_bytes: self.total_bytes,
            bytes_used: self.bytes_used,
            num_devices: self.num_devices,
            sector_size: self.sector_size,
            node_size: self.node_size,
        }
    }
}

/// What an assembled device holds: its partition table (or the single
/// whole-device volume), the kind used to label it, and the filesystem
/// summary of its largest recognised volume.
pub(super) struct DeviceDescription {
    pub(super) partitions: Vec<Partition>,
    pub(super) fs_kind: PartitionKind,
    pub(super) filesystem: Option<AssembledFilesystem>,
}

/// Detect the partition table / bare volume on an assembled device. Read
/// errors are reported in `diagnostics`; the device is still registered so it
/// stays reachable for hex view, carving and export.
pub(super) fn describe_device(
    source: &(dyn SectorSource + Send + Sync),
    name: &str,
    diagnostics: &mut Vec<String>,
) -> DeviceDescription {
    let mut partitions = match detect_partitions_with_meta(source, 512) {
        Ok(table) => table.partitions,
        Err(error) => {
            diagnostics.push(format!("{name}: partition scan failed: {error:#}"));
            Vec::new()
        }
    };
    if let Err(error) = refine_partition_kinds(source, &mut partitions, 512) {
        diagnostics.push(format!("{name}: volume detection failed: {error:#}"));
    }
    let primary = partitions
        .iter()
        .filter(|partition| manual::is_browsable_fs(partition.kind))
        .max_by_key(|partition| partition.length_lba)
        .or_else(|| {
            partitions
                .iter()
                .filter(|partition| partition.kind != PartitionKind::Unknown)
                .max_by_key(|partition| partition.length_lba)
        });
    let fs_kind = primary.map_or(PartitionKind::Unknown, |partition| partition.kind);
    let filesystem = primary.and_then(|partition| filesystem_summary(source, partition));
    DeviceDescription {
        partitions,
        fs_kind,
        filesystem,
    }
}

/// Read the superblock of the filesystem on `partition` for its identity and
/// size. Btrfs, ext2/3/4, XFS and NTFS are summarised; other kinds return
/// `None`.
pub(super) fn filesystem_summary(
    source: &(dyn SectorSource + Send + Sync),
    partition: &Partition,
) -> Option<AssembledFilesystem> {
    let base = partition.start_lba.checked_mul(512)?;
    match partition.kind {
        PartitionKind::Btrfs => btrfs_summary(source, base),
        PartitionKind::Ext4 => ext4_summary(source, base),
        PartitionKind::Xfs => xfs_summary(source, base),
        PartitionKind::Ntfs => ntfs_summary(source, base),
        _ => None,
    }
}

fn le_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn le_u64(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn btrfs_summary(
    source: &(dyn SectorSource + Send + Sync),
    base: u64,
) -> Option<AssembledFilesystem> {
    // `struct btrfs_super_block`: fsid 0x20, magic 0x40, total_bytes 0x70,
    // bytes_used 0x78, num_devices 0x88, sectorsize 0x90, nodesize 0x94.
    let buf = source.read(base + BTRFS_PRIMARY_SUPERBLOCK, 0x100).ok()?;
    if buf.get(0x40..0x48)? != BTRFS_MAGIC {
        return None;
    }
    Some(AssembledFilesystem {
        kind: PartitionKind::Btrfs,
        fsid_hex: Some(hex(buf.get(0x20..0x30)?)),
        bytes_used: le_u64(&buf, 0x78),
        total_bytes: le_u64(&buf, 0x70),
        num_devices: le_u64(&buf, 0x88),
        sector_size: le_u32(&buf, 0x90),
        node_size: le_u32(&buf, 0x94),
    })
}

fn ext4_summary(
    source: &(dyn SectorSource + Send + Sync),
    base: u64,
) -> Option<AssembledFilesystem> {
    // `struct ext2_super_block` at +1024: blocks_count_lo 0x04,
    // log_block_size 0x18, magic 0x38, uuid 0x68.
    let sb = source.read(base + 1024, 0x100).ok()?;
    if sb.get(0x38..0x3A)? != [0x53, 0xEF] {
        return None;
    }
    let blocks = u64::from(le_u32(&sb, 0x04)?);
    let block_size = 1024u32 << le_u32(&sb, 0x18)?.min(20);
    Some(AssembledFilesystem {
        kind: PartitionKind::Ext4,
        fsid_hex: Some(hex(sb.get(0x68..0x78)?)),
        bytes_used: None,
        total_bytes: Some(blocks.saturating_mul(u64::from(block_size))),
        num_devices: None,
        sector_size: Some(block_size),
        node_size: None,
    })
}

fn xfs_summary(
    source: &(dyn SectorSource + Send + Sync),
    base: u64,
) -> Option<AssembledFilesystem> {
    // XFS superblock fields are big-endian: blocksize 0x04, dblocks 0x08,
    // uuid 0x20.
    let sb = source.read(base, 512).ok()?;
    if sb.get(0..4)? != b"XFSB" {
        return None;
    }
    let block_size = u32::from_be_bytes(sb.get(0x04..0x08)?.try_into().ok()?);
    let blocks = u64::from_be_bytes(sb.get(0x08..0x10)?.try_into().ok()?);
    Some(AssembledFilesystem {
        kind: PartitionKind::Xfs,
        fsid_hex: Some(hex(sb.get(0x20..0x30)?)),
        bytes_used: None,
        total_bytes: Some(blocks.saturating_mul(u64::from(block_size))),
        num_devices: None,
        sector_size: Some(block_size),
        node_size: None,
    })
}

fn ntfs_summary(
    source: &(dyn SectorSource + Send + Sync),
    base: u64,
) -> Option<AssembledFilesystem> {
    // NTFS boot sector: bytes per sector 0x0B, total sectors 0x28, serial 0x48.
    let boot = source.read(base, 512).ok()?;
    if boot.get(3..11)? != b"NTFS    " {
        return None;
    }
    let bytes_per_sector = u16::from_le_bytes(boot.get(0x0B..0x0D)?.try_into().ok()?);
    let total_sectors = le_u64(&boot, 0x28)?;
    let serial = le_u64(&boot, 0x48)?;
    Some(AssembledFilesystem {
        kind: PartitionKind::Ntfs,
        fsid_hex: Some(format!("{serial:016x}")),
        bytes_used: None,
        total_bytes: Some(total_sectors.saturating_mul(u64::from(bytes_per_sector))),
        num_devices: None,
        sector_size: Some(u32::from(bytes_per_sector)),
        node_size: None,
    })
}
