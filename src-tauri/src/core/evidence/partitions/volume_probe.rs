//! Volume-signature probes shared by partition refinement, whole-device
//! filesystem detection, and lost-partition recovery.
//!
//! Each probe is intentionally small and read-only: it either recognises a
//! volume/container from fixed on-disk evidence or abstains so the next probe
//! can try. The registry keeps boot-sector heuristics in one place instead of
//! scattering NTFS/FAT/ext/XFS/etc. checks across callers.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::core::evidence::crypto::bitlocker;
use crate::core::evidence::partitions::fat::{
    detect_valid_fat_boot_sector, recover_fat32_boot_sector, FAT32_BACKUP_BOOT_SECTOR,
};
use crate::core::evidence::partitions::hfsplus::{
    is_hfs_plus_magic, HFSX_MAGIC, HFS_VOLUME_HEADER_OFFSET,
};
use crate::core::evidence::partitions::luks::LUKS_MAGIC;
use crate::core::evidence::partitions::squashfs::is_squashfs_magic;
use crate::core::evidence::partitions::ufs::{
    is_ufs2_magic_at_superblock, UFS_MAGIC_OFFSET, UFS_SBLOCK_UFS2,
};
use crate::core::evidence::partitions::xfs::is_xfs_magic;
use crate::core::evidence::partitions::{
    DetectCtx, PartParser, Partition, PartitionKind, SectorSource, BTRFS_MAGIC,
    BTRFS_PRIMARY_SUPERBLOCK,
};

const BOOT_PROBE_BYTES: usize = 4096;
const EXT_MAGIC_OFFSET: usize = 0x400 + 0x38;
const REFS_OEM_OFFSET: usize = 3;
const REFS_FSRS_OFFSET: usize = 16;
const NTFS_OEM_OFFSET: usize = 3;
const EXFAT_OEM_OFFSET: usize = 3;
const CDFS_FIRST_VOLUME_DESCRIPTOR: u64 = 16 * 2048;
const CDFS_VOLUME_DESCRIPTOR_SIZE: usize = 2048;
const CDFS_MAX_VOLUME_DESCRIPTORS: u64 = 32;
const LVM2_SCAN_OFFSETS: [usize; 4] = [0, 512, 1024, 1536];
const ZFS_LABEL_SIZE: u64 = 256 * 1024;
const ZFS_UBERBLOCK_ARRAY_OFFSET: u64 = 128 * 1024;
const ZFS_UBERBLOCK_STRIDE: u64 = 1024;
const ZFS_UBERBLOCK_PROBE_COUNT: usize = 3;
const ZFS_UBERBLOCK_MAGIC_LE: [u8; 8] = [0x0c, 0xb1, 0xba, 0x00, 0, 0, 0, 0];
const ZFS_UBERBLOCK_MAGIC_BE: [u8; 8] = [0, 0, 0, 0, 0, 0xba, 0xb1, 0x0c];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeProbeHit {
    pub probe: String,
    pub kind: PartitionKind,
    pub confidence: u8,
    pub evidence: Vec<String>,
    pub relative_offset: u64,
}

impl VolumeProbeHit {
    fn new(
        probe: &'static str,
        kind: PartitionKind,
        confidence: u8,
        relative_offset: u64,
        evidence: impl Into<String>,
    ) -> Self {
        Self {
            probe: probe.into(),
            kind,
            confidence,
            evidence: vec![evidence.into()],
            relative_offset,
        }
    }
}

/// A stateless recogniser for one volume/container signature.
pub trait VolumeProbe: Sync {
    fn name(&self) -> &'static str;
    fn probe(&self, ctx: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>>;
}

pub struct WholeVolumeParser;

impl PartParser for WholeVolumeParser {
    fn name(&self) -> &'static str {
        "volume"
    }

    fn try_parse(&self, ctx: &DetectCtx<'_>) -> Result<Option<Vec<Partition>>> {
        let Some(hit) = probe_volume_with_head(ctx)? else {
            return Ok(None);
        };
        if ctx.sector_size == 0 {
            return Ok(None);
        }
        Ok(Some(vec![Partition {
            id: 0,
            kind: hit.kind,
            start_lba: 0,
            length_lba: ctx.size / ctx.sector_size as u64,
            label: None,
            unique_guid: None,
        }]))
    }
}

struct UbiProbe;

impl VolumeProbe for UbiProbe {
    fn name(&self) -> &'static str {
        "ubi"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
        if ctx.head.get(..4) != Some(b"UBI#") {
            return Ok(None);
        }
        let header = ubidump::ubifs::ec_header::UbiEcHeader::parse(ctx.head)?;
        anyhow::ensure!(
            header.version == 1
                && header.vid_hdr_ofs >= 64
                && header
                    .vid_hdr_ofs
                    .checked_add(64)
                    .is_some_and(|end| end <= header.data_ofs)
                && u64::from(header.data_ofs) < ctx.size,
            "invalid UBI EC geometry"
        );
        Ok(Some(VolumeProbeHit::new(
            "ubi",
            PartitionKind::Ubi,
            100,
            0,
            "UBI EC header with verified CRC",
        )))
    }
}

static VOLUME_PROBES: &[&dyn VolumeProbe] = &[
    &FilevaultProbe,
    &BitlockerProbe,
    &SquashfsProbe,
    &UbiProbe,
    &XfsProbe,
    &ExtProbe,
    &ApfsProbe,
    &HfsProbe,
    &LvmProbe,
    &LuksProbe,
    &NtfsProbe,
    &RefsProbe,
    &ExfatProbe,
    &CdfsProbe,
    &FatProbe,
    &BtrfsProbe,
    &UfsProbe,
    &ZfsProbe,
];

pub fn probe_volume(
    source: &dyn SectorSource,
    sector_size: usize,
) -> Result<Option<VolumeProbeHit>> {
    let head = source.read(0, BOOT_PROBE_BYTES)?;
    let ctx = DetectCtx {
        source,
        head: &head,
        sector_size,
        size: source.size(),
    };
    probe_volume_with_head(&ctx)
}

pub fn probe_volume_with_head(ctx: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
    for probe in VOLUME_PROBES {
        if let Some(hit) = probe.probe(ctx)? {
            tracing::debug!(probe = probe.name(), kind = ?hit.kind, "volume probe matched");
            return Ok(Some(hit));
        }
    }
    Ok(None)
}

pub fn detect_filesystem_bytes(bytes: &[u8], fallback: PartitionKind) -> PartitionKind {
    if bitlocker::is_bitlocker_boot_sector_bytes(bytes) {
        return PartitionKind::Bitlocker;
    }
    if is_squashfs_magic(bytes) {
        return PartitionKind::Squashfs;
    }
    if is_xfs_magic(bytes) {
        return PartitionKind::Xfs;
    }
    if has_ext_magic(bytes) {
        return PartitionKind::Ext4;
    }
    if has_apfs_header(bytes) {
        return PartitionKind::ApfsContainer;
    }
    if filevault::storage::check_volume_signature(bytes) {
        return PartitionKind::Filevault;
    }
    if is_hfs_plus_magic(bytes) {
        let magic = &bytes[HFS_VOLUME_HEADER_OFFSET..HFS_VOLUME_HEADER_OFFSET + 2];
        return if magic == HFSX_MAGIC {
            PartitionKind::HfsX
        } else {
            PartitionKind::HfsPlus
        };
    }
    if has_lvm_label(bytes) {
        return PartitionKind::Lvm;
    }
    if bytes.len() >= LUKS_MAGIC.len() && bytes[0..LUKS_MAGIC.len()] == LUKS_MAGIC {
        return PartitionKind::Luks;
    }
    if has_ntfs_oem(bytes) {
        return PartitionKind::Ntfs;
    }
    if has_refs_signature(bytes) {
        return PartitionKind::Refs;
    }
    if has_exfat_oem(bytes) {
        return PartitionKind::ExFat;
    }
    if has_cdfs_descriptor(bytes) {
        return PartitionKind::Cdfs;
    }
    if let Some(kind) = detect_valid_fat_boot_sector(bytes, u64::MAX) {
        return kind;
    }
    let magic_start = BTRFS_PRIMARY_SUPERBLOCK as usize + 0x40;
    if bytes.get(magic_start..magic_start + BTRFS_MAGIC.len()) == Some(BTRFS_MAGIC.as_slice()) {
        return PartitionKind::Btrfs;
    }
    fallback_after_probe(fallback)
}

pub fn fallback_after_probe(fallback: PartitionKind) -> PartitionKind {
    match fallback {
        PartitionKind::Ext4
        | PartitionKind::Ntfs
        | PartitionKind::Refs
        | PartitionKind::Btrfs
        | PartitionKind::Fat32
        | PartitionKind::Fat12
        | PartitionKind::Fat16
        | PartitionKind::ExFat
        | PartitionKind::Cdfs
        | PartitionKind::Squashfs
        | PartitionKind::Ubi
        | PartitionKind::Lvm
        | PartitionKind::Bitlocker
        | PartitionKind::Luks
        | PartitionKind::HfsPlus
        | PartitionKind::HfsX
        | PartitionKind::Xfs => PartitionKind::Unknown,
        PartitionKind::Filevault
        | PartitionKind::BiosBoot
        | PartitionKind::ApfsContainer
        | PartitionKind::Apm
        | PartitionKind::Bsd
        | PartitionKind::Swap
        | PartitionKind::Extended
        | PartitionKind::Mdadm
        | PartitionKind::Ufs
        | PartitionKind::Zfs
        | PartitionKind::Unknown => fallback,
    }
}

struct FilevaultProbe;
impl VolumeProbe for FilevaultProbe {
    fn name(&self) -> &'static str {
        "filevault"
    }
    fn probe(&self, ctx: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
        if filevault::storage::check_volume_signature(ctx.head) {
            return Ok(Some(VolumeProbeHit::new(
                "filevault",
                PartitionKind::Filevault,
                100,
                0,
                "CoreStorage CS header with verified CRC-32C",
            )));
        }
        if let Some(offset) = ctx.size.checked_sub(512) {
            match ctx.source.read(offset, 512) {
                Ok(footer) if filevault::storage::check_volume_signature(&footer) => {
                    return Ok(Some(VolumeProbeHit::new(
                        "filevault",
                        PartitionKind::Filevault,
                        90,
                        offset,
                        "CoreStorage backup CS header with verified CRC-32C",
                    )));
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::debug!(%error,"optional CoreStorage backup probe unavailable")
                }
            }
        }
        Ok(None)
    }
}

struct BitlockerProbe;
struct SquashfsProbe;
struct XfsProbe;
struct ExtProbe;
struct HfsProbe;
struct ApfsProbe;
struct LvmProbe;
struct LuksProbe;
struct NtfsProbe;
struct RefsProbe;
struct ExfatProbe;
struct CdfsProbe;
struct FatProbe;
struct BtrfsProbe;
struct UfsProbe;
struct ZfsProbe;

impl VolumeProbe for BitlockerProbe {
    fn name(&self) -> &'static str {
        "bitlocker"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
        if bitlocker::is_bitlocker_boot_sector_bytes(ctx.head) {
            Ok(Some(VolumeProbeHit::new(
                self.name(),
                PartitionKind::Bitlocker,
                98,
                0,
                "BitLocker FVE boot-sector signature",
            )))
        } else {
            Ok(None)
        }
    }
}

impl VolumeProbe for ApfsProbe {
    fn name(&self) -> &'static str {
        "apfs"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
        if !has_apfs_header(ctx.head) {
            return Ok(None);
        }
        let size = u32::from_le_bytes(ctx.head[36..40].try_into()?);
        let block = ctx.source.read(0, size as usize)?;
        let superblock = match apfs::superblock::NxSuperblock::parse(&block) {
            Ok(superblock) => superblock,
            Err(_) => return Ok(None),
        };
        if block.len() != size as usize
            || !apfs::fletcher::verify_object(&block)
            || superblock
                .block_count
                .checked_mul(u64::from(size))
                .is_none_or(|length| length > ctx.size)
        {
            return Ok(None);
        }
        Ok(Some(VolumeProbeHit::new(
            self.name(),
            PartitionKind::ApfsContainer,
            98,
            0,
            "APFS NXSB identity, geometry and Fletcher checksum",
        )))
    }
}

impl VolumeProbe for SquashfsProbe {
    fn name(&self) -> &'static str {
        "squashfs"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
        if is_squashfs_magic(ctx.head) {
            Ok(Some(VolumeProbeHit::new(
                self.name(),
                PartitionKind::Squashfs,
                96,
                0,
                "SquashFS magic at volume offset 0",
            )))
        } else {
            Ok(None)
        }
    }
}

impl VolumeProbe for XfsProbe {
    fn name(&self) -> &'static str {
        "xfs"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
        if is_xfs_magic(ctx.head) {
            Ok(Some(VolumeProbeHit::new(
                self.name(),
                PartitionKind::Xfs,
                96,
                0,
                "XFSB superblock magic at volume offset 0",
            )))
        } else {
            Ok(None)
        }
    }
}

impl VolumeProbe for ExtProbe {
    fn name(&self) -> &'static str {
        "ext"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
        if has_ext_magic(ctx.head) {
            Ok(Some(VolumeProbeHit::new(
                self.name(),
                PartitionKind::Ext4,
                88,
                EXT_MAGIC_OFFSET as u64,
                "Ext superblock magic 0xef53",
            )))
        } else {
            Ok(None)
        }
    }
}

impl VolumeProbe for HfsProbe {
    fn name(&self) -> &'static str {
        "hfsplus"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
        if !is_hfs_plus_magic(ctx.head) {
            return Ok(None);
        }
        let magic = &ctx.head[HFS_VOLUME_HEADER_OFFSET..HFS_VOLUME_HEADER_OFFSET + 2];
        let kind = if magic == HFSX_MAGIC {
            PartitionKind::HfsX
        } else {
            PartitionKind::HfsPlus
        };
        Ok(Some(VolumeProbeHit::new(
            self.name(),
            kind,
            94,
            HFS_VOLUME_HEADER_OFFSET as u64,
            "HFS+/HFSX volume header magic",
        )))
    }
}

impl VolumeProbe for LvmProbe {
    fn name(&self) -> &'static str {
        "lvm2"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
        if LVM2_SCAN_OFFSETS.iter().any(|&offset| {
            ctx.head.get(offset..offset + 512).is_some_and(|label| {
                crate::core::evidence::partitions::lvm2::header::parse_label(
                    label,
                    offset as u64,
                    ctx.size,
                )
                .is_ok()
            })
        }) {
            Ok(Some(VolumeProbeHit::new(
                self.name(),
                PartitionKind::Lvm,
                90,
                0,
                "LVM2 physical volume label with valid CRC and geometry",
            )))
        } else {
            Ok(None)
        }
    }
}

impl VolumeProbe for LuksProbe {
    fn name(&self) -> &'static str {
        "luks"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
        if ctx.head.len() >= LUKS_MAGIC.len() && ctx.head[0..LUKS_MAGIC.len()] == LUKS_MAGIC {
            Ok(Some(VolumeProbeHit::new(
                self.name(),
                PartitionKind::Luks,
                99,
                0,
                "LUKS magic at volume offset 0",
            )))
        } else {
            Ok(None)
        }
    }
}

impl VolumeProbe for NtfsProbe {
    fn name(&self) -> &'static str {
        "ntfs"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
        if has_ntfs_oem(ctx.head) {
            Ok(Some(VolumeProbeHit::new(
                self.name(),
                PartitionKind::Ntfs,
                94,
                NTFS_OEM_OFFSET as u64,
                "NTFS OEM ID at boot-sector offset 3",
            )))
        } else {
            Ok(None)
        }
    }
}

impl VolumeProbe for RefsProbe {
    fn name(&self) -> &'static str {
        "refs"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
        if has_refs_signature(ctx.head) {
            Ok(Some(VolumeProbeHit::new(
                self.name(),
                PartitionKind::Refs,
                96,
                REFS_OEM_OFFSET as u64,
                "ReFS OEM ID plus FSRS signature",
            )))
        } else {
            Ok(None)
        }
    }
}

impl VolumeProbe for ExfatProbe {
    fn name(&self) -> &'static str {
        "exfat"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
        if has_exfat_oem(ctx.head) {
            Ok(Some(VolumeProbeHit::new(
                self.name(),
                PartitionKind::ExFat,
                96,
                EXFAT_OEM_OFFSET as u64,
                "exFAT OEM ID and reserved-zero range",
            )))
        } else {
            Ok(None)
        }
    }
}

impl VolumeProbe for FatProbe {
    fn name(&self) -> &'static str {
        "fat"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
        if let Some(kind) = detect_valid_fat_boot_sector(ctx.head, ctx.size) {
            return Ok(Some(VolumeProbeHit::new(
                self.name(),
                kind,
                92,
                0,
                "valid FAT boot sector",
            )));
        }
        let backup_offset = FAT32_BACKUP_BOOT_SECTOR.saturating_mul(ctx.sector_size as u64);
        if backup_offset + 512 <= ctx.size {
            let backup = ctx.source.read(backup_offset, 512)?;
            if let Some(kind) = detect_valid_fat_boot_sector(&backup, ctx.size) {
                return Ok(Some(VolumeProbeHit::new(
                    self.name(),
                    kind,
                    86,
                    backup_offset,
                    "valid FAT32 backup boot sector",
                )));
            }
        }
        if recover_fat32_boot_sector(ctx.source, ctx.size, ctx.sector_size as u64)?.is_some() {
            return Ok(Some(VolumeProbeHit::new(
                self.name(),
                PartitionKind::Fat32,
                82,
                0,
                "recoverable FAT32 geometry from FSInfo, FAT, and root directory",
            )));
        }
        Ok(None)
    }
}

impl VolumeProbe for CdfsProbe {
    fn name(&self) -> &'static str {
        "cdfs"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
        if crate::core::evidence::readers::optical_disc::DataTrackLayout::probe(ctx.source)?
            .is_some()
        {
            return Ok(Some(VolumeProbeHit::new(
                self.name(),
                PartitionKind::Cdfs,
                100,
                0,
                "raw Mode 1 / XA Form 1 CD with validated ISO 9660 primary descriptor",
            )));
        }
        if ctx.size < CDFS_FIRST_VOLUME_DESCRIPTOR + 7 {
            return Ok(None);
        }
        for index in 0..CDFS_MAX_VOLUME_DESCRIPTORS {
            let offset = CDFS_FIRST_VOLUME_DESCRIPTOR + index * CDFS_VOLUME_DESCRIPTOR_SIZE as u64;
            if offset + 7 > ctx.size {
                return Ok(None);
            }
            let descriptor = ctx.source.read(offset, 7)?;
            if descriptor.len() < 7 || &descriptor[1..6] != b"CD001" || descriptor[6] != 1 {
                return Ok(None);
            }
            match descriptor[0] {
                1 | 2 => {
                    return Ok(Some(VolumeProbeHit::new(
                        self.name(),
                        PartitionKind::Cdfs,
                        98,
                        offset,
                        "ISO 9660 volume descriptor signature CD001",
                    )))
                }
                255 => return Ok(None),
                _ => {}
            }
        }
        Ok(None)
    }
}

impl VolumeProbe for BtrfsProbe {
    fn name(&self) -> &'static str {
        "btrfs"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
        if ctx.size < BTRFS_PRIMARY_SUPERBLOCK + 0x48 {
            return Ok(None);
        }
        let probe = ctx.source.read(BTRFS_PRIMARY_SUPERBLOCK, 0x100)?;
        if probe.len() >= 0x48 && &probe[0x40..0x48] == BTRFS_MAGIC {
            Ok(Some(VolumeProbeHit::new(
                self.name(),
                PartitionKind::Btrfs,
                96,
                BTRFS_PRIMARY_SUPERBLOCK + 0x40,
                "Btrfs _BHRfS_M superblock magic",
            )))
        } else {
            Ok(None)
        }
    }
}

impl VolumeProbe for UfsProbe {
    fn name(&self) -> &'static str {
        "ufs"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
        let needed = UFS_SBLOCK_UFS2 as u64 + UFS_MAGIC_OFFSET + 4;
        if ctx.size < needed {
            return Ok(None);
        }
        let probe = ctx.source.read(UFS_SBLOCK_UFS2 as u64, 0x800)?;
        if is_ufs2_magic_at_superblock(&probe) {
            Ok(Some(VolumeProbeHit::new(
                self.name(),
                PartitionKind::Ufs,
                93,
                UFS_SBLOCK_UFS2 as u64 + UFS_MAGIC_OFFSET,
                "UFS2 superblock magic",
            )))
        } else {
            Ok(None)
        }
    }
}

impl VolumeProbe for ZfsProbe {
    fn name(&self) -> &'static str {
        "zfs"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
        if ctx.size < ZFS_LABEL_SIZE {
            return Ok(None);
        }
        let probe_len = ZFS_UBERBLOCK_STRIDE as usize * ZFS_UBERBLOCK_PROBE_COUNT;
        let probe = ctx.source.read(ZFS_UBERBLOCK_ARRAY_OFFSET, probe_len)?;
        if probe.len() < probe_len {
            return Ok(None);
        }
        for index in 0..ZFS_UBERBLOCK_PROBE_COUNT {
            let offset = index * ZFS_UBERBLOCK_STRIDE as usize;
            let magic = &probe[offset..offset + 8];
            if magic != ZFS_UBERBLOCK_MAGIC_LE && magic != ZFS_UBERBLOCK_MAGIC_BE {
                return Ok(None);
            }
        }
        Ok(Some(VolumeProbeHit::new(
            self.name(),
            PartitionKind::Zfs,
            94,
            ZFS_UBERBLOCK_ARRAY_OFFSET,
            "three consecutive ZFS uberblock magic values",
        )))
    }
}

fn has_apfs_header(bytes: &[u8]) -> bool {
    let Some(size) = bytes.get(36..40) else {
        return false;
    };
    let size = u32::from_le_bytes([size[0], size[1], size[2], size[3]]);
    bytes.get(32..36) == Some(b"NXSB") && (4096..=65536).contains(&size) && size.is_power_of_two()
}

fn has_ext_magic(bytes: &[u8]) -> bool {
    bytes.len() >= EXT_MAGIC_OFFSET + 2
        && bytes[EXT_MAGIC_OFFSET] == 0x53
        && bytes[EXT_MAGIC_OFFSET + 1] == 0xEF
}

fn has_lvm_label(bytes: &[u8]) -> bool {
    LVM2_SCAN_OFFSETS.iter().any(|&offset| {
        bytes.get(offset..offset + 8) == Some(b"LABELONE")
            && bytes.get(offset + 24..offset + 32) == Some(b"LVM2 001")
    })
}

fn has_ntfs_oem(bytes: &[u8]) -> bool {
    bytes.len() >= NTFS_OEM_OFFSET + 8
        && &bytes[NTFS_OEM_OFFSET..NTFS_OEM_OFFSET + 8] == b"NTFS    "
}

fn has_refs_signature(bytes: &[u8]) -> bool {
    bytes.len() >= REFS_FSRS_OFFSET + 4
        && &bytes[REFS_OEM_OFFSET..REFS_OEM_OFFSET + 4] == b"ReFS"
        && &bytes[REFS_FSRS_OFFSET..REFS_FSRS_OFFSET + 4] == b"FSRS"
}

fn has_exfat_oem(bytes: &[u8]) -> bool {
    bytes.len() >= 64
        && &bytes[EXFAT_OEM_OFFSET..EXFAT_OEM_OFFSET + 8] == b"EXFAT   "
        && bytes[11..64].iter().all(|&byte| byte == 0)
}

fn has_cdfs_descriptor(bytes: &[u8]) -> bool {
    let offset = CDFS_FIRST_VOLUME_DESCRIPTOR as usize;
    bytes.len() >= offset + 7
        && matches!(bytes[offset], 1 | 2)
        && &bytes[offset + 1..offset + 6] == b"CD001"
        && bytes[offset + 6] == 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::evidence::partitions::test_support::BufSource;

    #[test]
    fn detects_ntfs_from_shared_probe() {
        let mut bytes = vec![0u8; 4096];
        bytes[3..11].copy_from_slice(b"NTFS    ");
        let source = BufSource(bytes);
        let hit = probe_volume(&source, 512)
            .expect("probe")
            .expect("ntfs hit");
        assert_eq!(hit.kind, PartitionKind::Ntfs);
    }

    #[test]
    fn detects_btrfs_beyond_boot_probe() {
        let mut bytes = vec![0u8; 0x11000];
        bytes[BTRFS_PRIMARY_SUPERBLOCK as usize + 0x40..BTRFS_PRIMARY_SUPERBLOCK as usize + 0x48]
            .copy_from_slice(BTRFS_MAGIC);
        let source = BufSource(bytes);
        let hit = probe_volume(&source, 512)
            .expect("probe")
            .expect("btrfs hit");
        assert_eq!(hit.kind, PartitionKind::Btrfs);
        assert_eq!(
            detect_filesystem_bytes(&source.0, PartitionKind::Unknown),
            hit.kind
        );
        let magic_end = BTRFS_PRIMARY_SUPERBLOCK as usize + 0x48;
        assert_eq!(
            detect_filesystem_bytes(&source.0[..magic_end - 1], PartitionKind::Unknown),
            PartitionKind::Unknown
        );
        assert_eq!(
            detect_filesystem_bytes(&source.0[..magic_end], PartitionKind::Unknown),
            PartitionKind::Btrfs
        );
    }
}
