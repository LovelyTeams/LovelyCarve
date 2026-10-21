//! Partition-table recognition.
//!
//! The dispatcher follows three stages:
//!
//!   1. Wrap any input — RAW mmap, EWF, VMDK, nested partition slice — in a
//!      uniform `SectorSource` block-device abstraction.
//!   2. Read the first 16 sectors (`HEADER_PROBE_BYTES = 0x2000`) once and
//!      bundle the device handle, that probe buffer, and the sector size into
//!      a `DetectCtx`.
//!   3. Walk a priority-ordered table of `PartParser` objects. The table puts
//!      container-style formats (LUKS) and modern partition tables (GPT, APM)
//!      ahead of legacy MBR so that GPT's protective MBR cannot be
//!      mis-recognised as a real DOS partition table.
//!
//! Every concrete parser is a stateless unit struct implementing `PartParser`.
//! Each parser lives in its own `partitions/<fmt>.rs`
//! sibling submodule; adding a new format means adding one trait impl and one
//! entry to `PARSERS`.

pub mod apm;
pub mod bsd;
pub mod btrfs;
pub mod ext4;
pub mod fat;
pub mod gpt;
pub mod hfsplus;
pub mod luks;
pub mod lvm2;
pub mod mbr;
pub mod mdadm;
pub mod optical_disc;
pub mod squashfs;
pub mod ufs;
pub mod volume_probe;
pub mod xfs;
pub mod xiaomi_firmware;
pub mod zfs;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::core::evidence::image::{rebase_bad_regions, BadRegion};

use apm::ApmParser;
use bsd::BsdParser;
use gpt::GptParser;
use luks::LuksParser;
use lvm2::Lvm2Parser;
use mbr::MbrParser;
use mdadm::MdadmParser;
use optical_disc::OpticalDiscParser;
use volume_probe::{detect_filesystem_bytes, probe_volume, WholeVolumeParser};
use xiaomi_firmware::XiaomiFirmwareParser;

pub const HEADER_PROBE_BYTES: usize = 0x2000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Partition {
    pub id: u8,
    pub kind: PartitionKind,
    pub start_lba: u64,
    pub length_lba: u64,
    pub label: Option<String>,
    /// GPT 分区的**唯一**分区 GUID（canonical `Uuid` 字节，非磁盘混合端序）。
    /// 仅 GPT 表的分区为 `Some`；MBR / 容器（LUKS/LVM/MDADM）/ 整盘卷为
    /// `None`。用于把卷关联到 Windows `MountedDevices` 的 `DMIO:ID:` 项以
    /// 还原源系统盘符（见 `core::analysis::identify::win_drive_letters`）。`#[serde(default)]` 保持
    /// `ImageInfo` 线格式向后兼容。
    #[serde(default)]
    pub unique_guid: Option<[u8; 16]>,
}

/// Which partition-table layout claimed the device. `Volume` covers every
/// whole-device filesystem probe (no partition table at all); the container
/// schemes (LUKS / MDADM / LVM2) mean the device is a bare container whose
/// payload only becomes addressable after unwrap.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TableScheme {
    Gpt,
    Mbr,
    /// Xiaomi `HDR1` firmware container. Its filesystem-bearing segments are
    /// byte-addressed payloads rather than sector-aligned disk partitions.
    Firmware,
    Apm,
    Bsd,
    Luks,
    Mdadm,
    Lvm2,
    Volume,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PartitionKind {
    Ntfs,
    Refs,
    Ext4,
    Btrfs,
    Fat32,
    Fat12,
    Fat16,
    ExFat,
    Cdfs,
    Squashfs,
    Lvm,
    Bitlocker,
    BiosBoot,
    ApfsContainer,
    HfsPlus,
    HfsX,
    Luks,
    Filevault,
    Apm,
    Bsd,
    Swap,
    Extended,
    Mdadm,
    Ufs,
    Xfs,
    Zfs,
    Unknown,
    Ubi,
}

/// Block-device abstraction. Uniform read interface so the dispatcher and
/// every parser stay agnostic to whether the bytes come from a RAW mmap, an
/// EWF/VMDK reader, or a nested partition range.
pub trait SectorSource {
    fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>>;
    fn size(&self) -> u64;

    /// [`Self::read`] plus the spans of the returned bytes that a tolerant
    /// backend synthesized as zeros for unreadable source sectors, as absolute
    /// offsets of this source within the bytes returned. The caller owns the
    /// spans: a composite device maps them into its own address space, and
    /// nothing on this path reports them to a read-fault collection scope. A
    /// source that wraps others forwards the call; one that never synthesizes
    /// bytes keeps this default.
    fn read_traced(&self, offset: u64, len: usize) -> anyhow::Result<(Vec<u8>, Vec<BadRegion>)> {
        Ok((self.read(offset, len)?, Vec::new()))
    }
}

/// Bundle handed to each `PartParser`: the underlying block device, a cached
/// 16-sector probe buffer, the sector size, and the device size.
pub struct DetectCtx<'a> {
    pub source: &'a dyn SectorSource,
    pub head: &'a [u8],
    pub sector_size: usize,
    pub size: u64,
}

impl<'a> DetectCtx<'a> {
    fn for_source(source: &'a dyn SectorSource, head: &'a [u8], sector_size: usize) -> Self {
        Self {
            source,
            head,
            sector_size,
            size: source.size(),
        }
    }
}

/// A stateless partition-format recognizer.
/// Each implementation either claims the device by returning `Some(parts)` —
/// possibly an empty `Vec` for a recognised-but-empty table — or returns
/// `None` so the dispatcher tries the next parser.
pub trait PartParser: Sync {
    fn name(&self) -> &'static str;
    /// Table layout this parser recognises. Defaults to [`TableScheme::Volume`]
    /// because the long tail of parsers are whole-device filesystem probes;
    /// only the real table / container parsers override it.
    fn scheme(&self) -> TableScheme {
        TableScheme::Volume
    }
    fn try_parse(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<Vec<Partition>>>;

    /// Return table identity from the same parse as the partitions. Parsers
    /// with multiple recoverable copies must override this so metadata cannot
    /// come from a different copy. Ordinary probes need no disk identifier.
    fn try_parse_with_meta(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<DetectedTable>> {
        Ok(self.try_parse(ctx)?.map(|partitions| DetectedTable {
            partitions,
            scheme: Some(self.scheme()),
            disk_id: if self.scheme() == TableScheme::Mbr {
                mbr::read_disk_signature(ctx.head)
            } else {
                None
            },
        }))
    }
}

/// Priority-ordered parser table. Containers and modern partition layouts
/// are checked before legacy MBR to avoid misreading protective MBR entries.
///
/// LUKS sits at the top because its 6-byte fixed magic at offset 0 is
/// unambiguous: anything that matches LUKS is a LUKS container, and the
/// device must be unwrapped before further partition recognition. MDADM
/// follows because its 4-byte superblock magic at offset 0/8 cannot collide
/// with GPT/MBR signatures, and a RAID member must be assembled before any
/// partition table on it makes sense. GPT runs before MBR so a GPT disk's
/// protective MBR (type `0xEE`, offset 1) cannot be mistaken for a real DOS
/// partition table. LVM2 sits before Btrfs / Ext4 so a normally-partitioned
/// disk that happens to embed an LVM PV label gets recognised by its real
/// partition table first. FAT / Btrfs / Ext4 / Squashfs / UFS / XFS / ZFS / HFS+
/// sit last so they only fire on bare, whole-device filesystems (no MBR /
/// GPT / LUKS / etc.) — FAT's BPB lives in the first sector with a common
/// FAT32 backup at sector 6, Btrfs's 8-byte `_BHRfS_M` magic at fixed offset
/// `0x10040`, Ext4's 2-byte `0xEF53` magic at fixed offset `0x438`,
/// SquashFS v4's 4-byte `hsqs` / `sqsh` magic at offset 0, ZFS's three
/// consecutive 8-byte uberblock magics at offset `0x20000`, and HFS+'s
/// 2-byte `H+` / `HX` magic at fixed offset `0x400` cannot collide with
/// anything the earlier parsers would have claimed.
static PARSERS: &[&dyn PartParser] = &[
    &LuksParser,
    &MdadmParser,
    &XiaomiFirmwareParser,
    &OpticalDiscParser,
    &GptParser,
    &ApmParser,
    &BsdParser,
    &MbrParser,
    &Lvm2Parser,
    &WholeVolumeParser,
];

/// Result of [`detect_partitions_with_meta`]: the flat partition list plus
/// the table identity that produced it. `scheme` is `None` when no parser
/// claimed the device; `disk_id` carries the GPT disk GUID or the MBR disk
/// signature when the matching scheme defines one.
#[derive(Debug, Clone)]
pub struct DetectedTable {
    pub partitions: Vec<Partition>,
    pub scheme: Option<TableScheme>,
    pub disk_id: Option<String>,
}

/// Walk `PARSERS` in priority order and return the first match.
pub fn detect_partitions(
    source: &dyn SectorSource,
    sector_size: usize,
) -> anyhow::Result<Vec<Partition>> {
    Ok(detect_partitions_with_meta(source, sector_size)?.partitions)
}

/// Like [`detect_partitions`], but also reports which table scheme matched
/// and the on-disk disk identifier (GPT disk GUID / MBR signature).
pub fn detect_partitions_with_meta(
    source: &dyn SectorSource,
    sector_size: usize,
) -> anyhow::Result<DetectedTable> {
    if sector_size == 0 {
        return Ok(DetectedTable {
            partitions: Vec::new(),
            scheme: None,
            disk_id: None,
        });
    }
    let head = source
        .read(0, HEADER_PROBE_BYTES)
        .context("reading partition header probe")?;
    let ctx = DetectCtx::for_source(source, &head, sector_size);
    for parser in PARSERS {
        if let Some(table) = parser.try_parse_with_meta(&ctx)? {
            tracing::debug!(
                parser = parser.name(),
                count = table.partitions.len(),
                "partition parser matched"
            );
            return Ok(table);
        }
    }
    Ok(DetectedTable {
        partitions: Vec::new(),
        scheme: None,
        disk_id: None,
    })
}

/// Container-style kind that represents an opaque payload — re-running
/// parsers on its raw bytes produces noise until the container is unwrapped
/// (LUKS decrypted, MDADM array assembled, LVM metadata parsed, BitLocker
/// unlocked) or the slice is already covered by another walker (Extended is
/// expanded by `MbrParser::follow_ebr_chain`). Recursion stops at these.
fn is_container_leaf(kind: PartitionKind) -> bool {
    match kind {
        PartitionKind::Luks
        | PartitionKind::Filevault
        | PartitionKind::Mdadm
        | PartitionKind::Lvm
        | PartitionKind::Bitlocker
        | PartitionKind::Ubi
        | PartitionKind::Extended => true,
        PartitionKind::Ntfs
        | PartitionKind::Refs
        | PartitionKind::Ext4
        | PartitionKind::Btrfs
        | PartitionKind::Fat32
        | PartitionKind::Fat12
        | PartitionKind::Fat16
        | PartitionKind::ExFat
        | PartitionKind::Cdfs
        | PartitionKind::Squashfs
        | PartitionKind::BiosBoot
        | PartitionKind::ApfsContainer
        | PartitionKind::HfsPlus
        | PartitionKind::HfsX
        | PartitionKind::Apm
        | PartitionKind::Bsd
        | PartitionKind::Swap
        | PartitionKind::Ufs
        | PartitionKind::Xfs
        | PartitionKind::Zfs
        | PartitionKind::Unknown => false,
    }
}

fn should_recurse(kind: PartitionKind, byte_off: u64, byte_len: u64, parent_size: u64) -> bool {
    if is_container_leaf(kind) {
        return false;
    }
    if byte_len < HEADER_PROBE_BYTES as u64 {
        return false;
    }
    if byte_off == 0 && byte_len == parent_size {
        return false;
    }
    true
}

/// One node of the recursive partition tree. `partition` holds the
/// dispatcher's flat result for this level; `children` holds whatever the
/// recursive scan found inside the partition's byte range. A container-leaf
/// kind always has empty children.
#[derive(Debug, Clone)]
pub struct PartitionNode {
    pub partition: Partition,
    pub children: Vec<PartitionNode>,
}

/// Depth-limited recursive partition scan. Each discovered
/// partition becomes a sub-IO range, and `detect_partitions` is invoked
/// against that range with `max_depth` decremented.
///
/// Recursion stops when `max_depth` hits 0, when the child kind is a
/// container leaf (LUKS/MDADM/LVM/BitLocker/Extended), when the child is too
/// small to host a partition table (`< HEADER_PROBE_BYTES`), when the child
/// reports the parent's whole-device range, or when the child's absolute
/// byte range has already been visited at a previous level.
///
/// The function is intentionally not wired through the IPC layer — its
/// output is Rust-internal so consumers can opt in once container unwrap
/// (LUKS decryption, LVM metadata parsing, mdadm assembly) lands.
pub fn detect_partitions_tree(
    source: &dyn SectorSource,
    sector_size: usize,
    max_depth: u8,
) -> anyhow::Result<Vec<PartitionNode>> {
    let mut visited: HashSet<(u64, u64)> = HashSet::new();
    detect_recursive(source, sector_size, max_depth, 0, &mut visited)
}

fn detect_recursive(
    source: &dyn SectorSource,
    sector_size: usize,
    max_depth: u8,
    abs_base: u64,
    visited: &mut HashSet<(u64, u64)>,
) -> anyhow::Result<Vec<PartitionNode>> {
    if max_depth == 0 {
        return Ok(Vec::new());
    }
    let partitions = detect_partitions(source, sector_size)?;
    let parent_size = source.size();
    let mut nodes = Vec::with_capacity(partitions.len());
    for partition in partitions {
        let byte_off = partition.start_lba.saturating_mul(sector_size as u64);
        let byte_len = partition.length_lba.saturating_mul(sector_size as u64);
        let abs_off = abs_base.saturating_add(byte_off);
        let abs_key = (abs_off, byte_len);
        let children = if should_recurse(partition.kind, byte_off, byte_len, parent_size)
            && visited.insert(abs_key)
        {
            let slice = PartitionSlice::new(source, byte_off, byte_len);
            detect_recursive(&slice, sector_size, max_depth - 1, abs_off, visited)?
        } else {
            Vec::new()
        };
        nodes.push(PartitionNode {
            partition,
            children,
        });
    }
    Ok(nodes)
}

/// Promote partition kinds by probing each partition's boot region. Preserves
/// table-supplied kinds for formats we cannot positively identify from the
/// boot region alone.
pub fn refine_partition_kinds(
    source: &dyn SectorSource,
    partitions: &mut [Partition],
    sector_size: usize,
) -> anyhow::Result<()> {
    for partition in partitions {
        let offset = partition.start_lba * sector_size as u64;
        let byte_len = partition.length_lba.saturating_mul(sector_size as u64);
        let slice = PartitionSlice::new(source, offset, byte_len);
        // Cache the 4 KiB boot region the first time we read it so the
        // filesystem-detection fallback and the BIOS-boot zero check below
        // share a single read instead of hitting the (often compressed
        // EWF/VMDK) backing store twice for the same offset. probe_volume
        // does its own reads, so the cache stays lazy — partitions it
        // positively identifies never pay for the 4 KiB read at all.
        let mut header: Option<Vec<u8>> = None;
        partition.kind = if let Some(hit) = probe_volume(&slice, sector_size)? {
            hit.kind
        } else {
            let bytes = header.insert(source.read(offset, 4096)?);
            detect_filesystem(bytes, partition.kind)
        };
        if partition.kind == PartitionKind::Unknown && partition.length_lba <= 4096 {
            let bytes = match header {
                Some(bytes) => bytes,
                None => source.read(offset, 4096)?,
            };
            if bytes.iter().any(|&byte| byte != 0) {
                partition.kind = PartitionKind::BiosBoot;
            }
        }
    }
    Ok(())
}

pub(crate) const BTRFS_PRIMARY_SUPERBLOCK: u64 = 0x1_0000;
pub(crate) const BTRFS_PROBE_END_OFFSET: u64 = BTRFS_PRIMARY_SUPERBLOCK + 0x48;
pub(crate) const BTRFS_MAGIC: &[u8; 8] = b"_BHRfS_M";

pub fn detect_filesystem(bytes: &[u8], fallback: PartitionKind) -> PartitionKind {
    detect_filesystem_bytes(bytes, fallback)
}

/// Adapter that exposes a sub-range of a parent `SectorSource` as its own
/// independent block device. Each discovered partition gets its own IO range
/// so it can be recursively
/// re-fed into the dispatcher (LUKS-decrypted bytes, nested partitions,
/// RAID members, etc.).
pub struct PartitionSlice<'a> {
    parent: &'a dyn SectorSource,
    base: u64,
    length: u64,
}

impl<'a> PartitionSlice<'a> {
    pub fn new(parent: &'a dyn SectorSource, base: u64, length: u64) -> Self {
        Self {
            parent,
            base,
            length,
        }
    }

    pub fn for_partition(
        parent: &'a dyn SectorSource,
        partition: &Partition,
        sector_size: usize,
    ) -> Self {
        Self {
            parent,
            base: partition.start_lba * sector_size as u64,
            length: partition.length_lba * sector_size as u64,
        }
    }

    /// Parent offset and length of a slice read; `None` when nothing is left.
    fn parent_span(&self, offset: u64, len: usize) -> Option<(u64, usize)> {
        let clamped_offset = offset.min(self.length);
        let available = self.length.saturating_sub(clamped_offset);
        let read_len = (len as u64).min(available) as usize;
        (read_len != 0).then_some((self.base + clamped_offset, read_len))
    }
}

impl<'a> SectorSource for PartitionSlice<'a> {
    fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        match self.parent_span(offset, len) {
            Some((parent_offset, read_len)) => self.parent.read(parent_offset, read_len),
            None => Ok(Vec::new()),
        }
    }

    fn size(&self) -> u64 {
        self.length
    }

    fn read_traced(&self, offset: u64, len: usize) -> anyhow::Result<(Vec<u8>, Vec<BadRegion>)> {
        let Some((parent_offset, read_len)) = self.parent_span(offset, len) else {
            return Ok((Vec::new(), Vec::new()));
        };
        let (bytes, faults) = self.parent.read_traced(parent_offset, read_len)?;
        let faults = rebase_bad_regions(
            &faults,
            parent_offset,
            bytes.len() as u64,
            parent_offset - self.base,
        );
        Ok((bytes, faults))
    }
}

/// Test-only `SectorSource` impl over an in-memory buffer. Exposed at
/// `pub(crate)` so each `core/part_*.rs` sibling can share the same fixture
/// without duplicating the trivial `read`/`size` shim.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub struct BufSource(pub Vec<u8>);

    impl SectorSource for BufSource {
        fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
            let start = (offset as usize).min(self.0.len());
            let end = (start + len).min(self.0.len());
            Ok(self.0[start..end].to_vec())
        }
        fn size(&self) -> u64 {
            self.0.len() as u64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_support::BufSource;

    #[test]
    fn parser_table_priority_is_documented_order() {
        let names: Vec<_> = PARSERS.iter().map(|p| p.name()).collect();
        assert_eq!(
            names,
            vec![
                "luks",
                "mdadm",
                "xiaomi_firmware",
                "optical_disc",
                "gpt",
                "apm",
                "bsd",
                "mbr",
                "lvm2",
                "volume"
            ]
        );
    }

    #[test]
    fn partition_slice_clamps_to_range() {
        let src = BufSource((0..32u8).collect());
        let slice = PartitionSlice::new(&src, 8, 12);
        assert_eq!(slice.size(), 12);
        let bytes = slice.read(0, 32).expect("read");
        assert_eq!(bytes, (8u8..20u8).collect::<Vec<_>>());
        let beyond = slice.read(20, 4).expect("read past end");
        assert!(beyond.is_empty());
    }

    #[test]
    fn partition_slice_reports_parent_faults_in_slice_offsets() {
        /// Zero bytes with a synthesized span at 1000..1100.
        struct Faulty;
        impl SectorSource for Faulty {
            fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
                Ok(vec![
                    0;
                    (len as u64).min(4096u64.saturating_sub(offset))
                        as usize
                ])
            }
            fn size(&self) -> u64 {
                4096
            }
            fn read_traced(
                &self,
                offset: u64,
                len: usize,
            ) -> anyhow::Result<(Vec<u8>, Vec<BadRegion>)> {
                let bytes = self.read(offset, len)?;
                let bad = [BadRegion {
                    offset: 1000,
                    len: 100,
                }];
                let faults = rebase_bad_regions(&bad, offset, bytes.len() as u64, offset);
                Ok((bytes, faults))
            }
        }
        let slice = PartitionSlice::new(&Faulty, 512, 1024);
        let (bytes, faults) = slice.read_traced(400, 400).expect("read");
        assert_eq!(bytes.len(), 400);
        assert_eq!(
            faults,
            vec![BadRegion {
                offset: 488,
                len: 100,
            }]
        );
        let (bytes, faults) = slice.read_traced(1000, 100).expect("tail");
        assert_eq!(bytes.len(), 24);
        assert!(faults.is_empty());
    }

    #[test]
    fn partition_slice_for_partition_uses_lba() {
        let src = BufSource((0..64u8).collect());
        let partition = Partition {
            id: 0,
            kind: PartitionKind::Unknown,
            start_lba: 1,
            length_lba: 1,
            label: None,
            unique_guid: None,
        };
        let slice = PartitionSlice::for_partition(&src, &partition, 16);
        let bytes = slice.read(0, 16).expect("read");
        assert_eq!(bytes, (16u8..32u8).collect::<Vec<_>>());
    }

    /// Build a buffer that looks like a GPT disk holding one partition whose
    /// payload begins with an LVM2 PV label. Top-level scan must find the GPT
    /// entry; recursion must descend one level and identify the LVM2
    /// container; LVM2 is a container leaf so recursion terminates there
    /// rather than re-scanning encrypted/raw container bytes.
    #[test]
    fn detect_partitions_tree_descends_into_gpt_then_stops_at_lvm() {
        let sector = 512usize;
        let mut buf = vec![0u8; sector * 400];

        buf[sector..sector + 8].copy_from_slice(b"EFI PART");
        buf[sector + 72..sector + 80].copy_from_slice(&2u64.to_le_bytes());
        buf[sector + 80..sector + 84].copy_from_slice(&1u32.to_le_bytes());
        buf[sector + 84..sector + 88].copy_from_slice(&128u32.to_le_bytes());

        let entries = sector * 2;
        buf[entries..entries + 16].copy_from_slice(&[1u8; 16]);
        buf[entries + 32..entries + 40].copy_from_slice(&100u64.to_le_bytes());
        buf[entries + 40..entries + 48].copy_from_slice(&399u64.to_le_bytes());

        let pv_off = 100 * sector;
        lvm2::test_support::put_label(&mut buf[pv_off..], 0, &[]);

        let src = BufSource(buf);
        let tree = detect_partitions_tree(&src, sector, 3).expect("tree");
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].partition.start_lba, 100);
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(tree[0].children[0].partition.kind, PartitionKind::Lvm);
        assert!(tree[0].children[0].children.is_empty());
    }

    #[test]
    fn detect_partitions_tree_respects_max_depth() {
        let sector = 512usize;
        let mut buf = vec![0u8; sector * 200];
        buf[sector..sector + 8].copy_from_slice(b"EFI PART");
        buf[sector + 72..sector + 80].copy_from_slice(&2u64.to_le_bytes());
        buf[sector + 80..sector + 84].copy_from_slice(&1u32.to_le_bytes());
        buf[sector + 84..sector + 88].copy_from_slice(&128u32.to_le_bytes());
        let entries = sector * 2;
        buf[entries..entries + 16].copy_from_slice(&[1u8; 16]);
        buf[entries + 32..entries + 40].copy_from_slice(&100u64.to_le_bytes());
        buf[entries + 40..entries + 48].copy_from_slice(&199u64.to_le_bytes());
        let pv_off = 100 * sector;
        buf[pv_off..pv_off + 8].copy_from_slice(b"LABELONE");
        buf[pv_off + 24..pv_off + 32].copy_from_slice(b"LVM2 001");

        let src = BufSource(buf);
        let tree = detect_partitions_tree(&src, sector, 1).expect("tree");
        assert_eq!(tree.len(), 1);
        assert!(
            tree[0].children.is_empty(),
            "max_depth=1 must surface only top-level partitions"
        );
    }
}
