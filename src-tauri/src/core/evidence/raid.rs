//! End-to-end RAID detection and assembly across loaded disk images.
//!
//! The pipeline runs in passes:
//!
//! 1. **LVM.** Walk every loaded image's partitions for LVM PV labels, join
//!    PVs into volume groups and materialise complete linear LVs as
//!    `LinearLvSource` devices (md members may live inside LVs).
//! 2. **md superblocks.** Probe every LV, partition and whole image for a
//!    Linux md superblock (versions 0.90, 1.0, 1.1, 1.2) and group members
//!    by set UUID.
//! 3. **md assembly.** For each set, decide which members are current from
//!    the event counters and the freshest role table, and build the reader
//!    for its level — linear, RAID0/1/4/5/6/10 ([`md_array`]). The assembled
//!    device is scanned for a partition table or bare volume.
//! 4. **Firmware RAID.** Whole images not used by md are probed for Intel
//!    IMSM ([`imsm`]) and SNIA DDF ([`ddf`]) container metadata; every volume
//!    they describe is assembled with the same readers ([`firmware`]).
//! 5. **LVM over arrays.** PVs found on assembled arrays join the volume
//!    groups; every complete LV not used as an md member becomes its own device.
//! 6. **Metadata-less arrays.** Remaining equal-size images are searched for
//!    hardware-RAID layouts ([`heuristic`]).
//!
//! The output is descriptive (`RaidStack`) plus the assembled readers; no
//! persistent state is mutated and the IPC layer can re-run detection on
//! demand.

pub mod ddf;
pub mod heuristic;
pub mod imsm;
pub mod lvm_meta;
pub mod mdadm_super;
pub mod source;
pub mod xfs_probe;

mod manual;
pub use manual::manual_assemble;
mod ddf_array;
mod ddf_geometry;
mod detect;
pub use detect::{detect_with_options, detect_with_options_observed, detect_with_sources};
mod progress;
pub use progress::{DetectProgress, DetectStage};
mod firmware;
mod imsm_array;
mod imsm_identity;
mod lvm_stack;
mod md_array;
mod members;
pub use members::resolve_array_sources;
use members::*;
mod probe;
use probe::*;

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::core::evidence::image::DiskImage;
use crate::core::evidence::partitions::{
    detect_partitions_with_meta, refine_partition_kinds, Partition, PartitionKind, SectorSource,
    BTRFS_MAGIC, BTRFS_PRIMARY_SUPERBLOCK,
};
use lvm_meta::LvmPvMetadata;
use mdadm_super::MdadmSuperblock;
use source::{LinearLvSource, LvByteSegment, MemberReader, StripedMember, WholeDiskMember};

/// Top-level result. Every section is filled in best-effort: if PV parsing
/// fails on one image, we still report whatever else worked. Empty
/// sub-vectors mean "nothing detected" rather than "error".
#[derive(Debug, Clone, Serialize)]
pub struct RaidStack {
    pub images: Vec<RaidImage>,
    pub volume_groups: Vec<RaidVolumeGroup>,
    pub arrays: Vec<RaidArray>,
    pub filesystems: Vec<RaidFilesystem>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RaidImage {
    pub image_id: String,
    pub path: String,
    pub size_bytes: u64,
    pub partition_count: usize,
    pub lvm_pv_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RaidVolumeGroup {
    pub vg_uuid: String,
    pub vg_name: String,
    pub extent_size_sectors: u64,
    pub members: Vec<RaidVgMember>,
    pub logical_volumes: Vec<RaidLogicalVolume>,
}

/// One PV of a volume group. `image_id` names the loaded image holding it, or
/// — for a PV on an assembled array — the array's synthetic image once the
/// IPC layer has registered it.
#[derive(Debug, Clone, Serialize)]
pub struct RaidVgMember {
    pub image_id: String,
    pub partition_id: u8,
    pub pv_local_id: String,
    pub pv_uuid: String,
    pub pe_start_sectors: u64,
    pub partition_byte_offset: u64,
    pub partition_byte_length: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RaidLogicalVolume {
    pub name: String,
    pub size_bytes: u64,
    pub segments: Vec<RaidLvSegment>,
    /// Synthetic image of a complete LV, once registered. Includes single-PV
    /// volumes; unsupported/incomplete volumes and md members have no image.
    #[serde(default)]
    pub assembled_image_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RaidLvSegment {
    pub pv_local_id: String,
    pub image_id: String,
    pub partition_id: u8,
    pub byte_offset: u64,
    pub byte_length: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RaidArray {
    pub set_uuid: String,
    pub set_name: String,
    /// md personality: `-1` linear, `0` RAID0, `1` RAID1, `4`/`5`/`6` parity
    /// RAID, `10` RAID10.
    pub level: i32,
    /// Raw md `layout` field: the parity rotation for RAID5/6, the copy
    /// layout for RAID10, the zone layout for RAID0.
    pub layout: u32,
    pub chunksize_sectors: u32,
    pub raid_disks: u32,
    pub data_offset_sectors: u64,
    /// Total assembled size of the array data area in bytes.
    pub assembled_bytes: u64,
    pub members: Vec<RaidArrayMember>,
    /// `Some(image_id)` after the IPC layer has materialised this array as a
    /// synthetic `DiskImage`. `None` for unsupported levels, sets missing
    /// members, or degraded sets awaiting the examiner's decision.
    pub assembled_image_id: Option<String>,
    /// `true` when this array was reconstructed by the metadata-less heuristic
    /// (`raid::heuristic`) — its parameters were *recovered* by brute-force +
    /// filesystem validation, not read from an mdadm superblock. The frontend
    /// surfaces this so an examiner knows the layout was inferred.
    #[serde(default)]
    pub heuristic: bool,
    /// Metadata the array was read from: the md superblock version (`0.90`,
    /// `1.0`, `1.1`, `1.2`), `imsm <version>` or `ddf <revision>`; empty for
    /// metadata-less arrays.
    #[serde(default)]
    pub metadata_version: String,
    /// The assembled device lacks at least one member: its data comes from
    /// parity or from the remaining mirrors.
    #[serde(default)]
    pub degraded: bool,
    /// Not assembled because members the metadata still expects are absent,
    /// though the present ones would suffice. Assembly needs the examiner's
    /// consent (`DetectOptions::degraded_sets`).
    #[serde(default)]
    pub degradable: bool,
}

/// What a member contributes to its array.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RaidMemberState {
    /// Current and read.
    #[default]
    Active,
    /// Hot spare without a slot.
    Spare,
    /// Marked failed by md.
    Faulty,
    /// Write journal device.
    Journal,
    /// Missed superblock updates (or, for metadata-less arrays, dropped out of
    /// the array); its slot is rebuilt instead of read.
    Stale,
    /// Still being rebuilt or replacing another member; not read.
    Rebuilding,
    /// The same device loaded twice; another copy is read.
    Duplicate,
    /// Carries the container's firmware RAID metadata but cannot be matched
    /// to one of its disk records (IMSM without the drive's serial number);
    /// not read.
    Unidentified,
}

#[derive(Debug, Clone, Serialize)]
pub struct RaidArrayMember {
    pub dev_number: u32,
    /// Slot this member occupies within the array (0..raid_disks-1), from
    /// the authoritative role table. `None` for spare, faulty and journal
    /// devices.
    pub slot: Option<u32>,
    pub device_uuid: String,
    pub data_size_sectors: u64,
    /// `true` when this member's slot is rebuilt from parity rather than read
    /// from the disk: a stale or rebuilding member of a parity array.
    #[serde(default)]
    pub reconstructed: bool,
    #[serde(default)]
    pub state: RaidMemberState,
    /// md event counter; `None` for metadata-less members.
    #[serde(default)]
    pub events: Option<u64>,
    /// Origin describing where the member's payload comes from. Either an
    /// LV-on-LVM identifier (`检材3` shape) or a whole-disk byte range
    /// (`服务器部分/server*.001` shape).
    pub origin: RaidMemberOrigin,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum RaidMemberOrigin {
    Lv {
        vg_uuid: String,
        vg_name: String,
        lv_name: String,
        size_bytes: u64,
    },
    /// Whole-image or partition member. `byte_offset` / `byte_length` are
    /// absolute against the source `DiskImage`; `partition_id` 0 with offset 0
    /// covering the image is the whole disk.
    Whole {
        image_id: String,
        partition_id: u8,
        byte_offset: u64,
        byte_length: u64,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct RaidFilesystem {
    pub set_uuid: String,
    pub kind: &'static str,
    pub fsid: Option<String>,
    pub total_bytes: Option<u64>,
    pub bytes_used: Option<u64>,
    pub num_devices: Option<u64>,
    pub sector_size: Option<u32>,
    pub node_size: Option<u32>,
}

/// Which stack record an assembled device belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssembledTarget {
    /// The `RaidStack::arrays` entry with the same `set_uuid`.
    Array,
    /// Logical volume `lv_name` of the volume group `vg_uuid`.
    LogicalVolume { vg_uuid: String, lv_name: String },
}

/// One assembled device — an array or a cross-device LV — plus the metadata
/// the IPC layer needs to register it as a synthetic `DiskImage`.
#[derive(Clone)]
pub struct AssembledArray {
    /// Stable identity: the md set UUID, a metadata-less array's id, or
    /// `lv:<vg uuid>:<lv name>`. Keys the registration cache.
    pub set_uuid: String,
    pub set_name: String,
    /// `assembled://<label>` path of the synthetic image; unique per device.
    pub label: String,
    pub target: AssembledTarget,
    /// Wrapped in `Arc` so the `ImageSource::Assembled` variant can take an
    /// owned `Arc<dyn SectorSource + Send + Sync>` and the caller can keep
    /// other handles for diagnostics or testing.
    pub source: Arc<dyn SectorSource + Send + Sync>,
    pub size_bytes: u64,
    /// Kind of the largest recognised volume on the device (`Unknown` when
    /// nothing was recognised).
    pub fs_kind: PartitionKind,
    /// Partition table or whole-device volume detected on the device. `None`
    /// registers a single whole-device `fs_kind` partition.
    pub partitions: Option<Vec<Partition>>,
}

/// `md:<name>-<uuid>` (or `md:<uuid>`) label of an array's synthetic image.
/// The full set UUID keeps it injective: a shorter prefix could be forged by
/// crafted superblocks.
pub fn array_label(set_name: &str, set_uuid: &str) -> String {
    if set_name.is_empty() {
        format!("md:{set_uuid}")
    } else {
        format!("md:{set_name}-{set_uuid}")
    }
}

/// Format the `Partition.label` shown on a synthetic assembled-image
/// partition: `"<fs>_md_<set-name-or-uuid>"`. Lives here so the
/// `kind → string` map stays alongside `AssembledArray` rather than in
/// the IPC layer.
pub fn synthetic_partition_label(array: &AssembledArray) -> String {
    let identifier = if array.set_name.is_empty() {
        array.set_uuid.as_str()
    } else {
        array.set_name.as_str()
    };
    crate::core::common::navigation_name::volume_name(&format!(
        "{}_md_{identifier}",
        kind_label(array.fs_kind)
    ))
}

/// Lowercase label for a partition/filesystem kind, shared by the synthetic
/// label builder and the manual-assemble IPC preview.
pub fn kind_label(kind: PartitionKind) -> &'static str {
    match kind {
        PartitionKind::Btrfs => "btrfs",
        PartitionKind::Ext4 => "ext4",
        PartitionKind::Ntfs => "ntfs",
        PartitionKind::Refs => "refs",
        PartitionKind::Fat32 => "fat32",
        PartitionKind::Fat12 => "fat12",
        PartitionKind::Fat16 => "fat16",
        PartitionKind::ExFat => "exfat",
        PartitionKind::Cdfs => "cdfs",
        PartitionKind::Squashfs => "squashfs",
        PartitionKind::Ubi => "ubi",
        PartitionKind::Lvm => "lvm",
        PartitionKind::Bitlocker => "bitlocker",
        PartitionKind::BiosBoot => "biosboot",
        PartitionKind::ApfsContainer => "apfs",
        PartitionKind::HfsPlus => "hfsplus",
        PartitionKind::HfsX => "hfsx",
        PartitionKind::Luks => "luks",
        PartitionKind::Filevault => "filevault",
        PartitionKind::Apm => "apm",
        PartitionKind::Bsd => "bsd",
        PartitionKind::Swap => "swap",
        PartitionKind::Extended => "extended",
        PartitionKind::Mdadm => "mdadm",
        PartitionKind::Ufs => "ufs",
        PartitionKind::Xfs => "xfs",
        PartitionKind::Zfs => "zfs",
        PartitionKind::Unknown => "unknown",
    }
}

/// Examiner choices that change what detection assembles.
#[derive(Debug, Clone, Default)]
pub struct DetectOptions {
    /// md set UUIDs to assemble without the members their metadata still
    /// expects (the present members must suffice for the level).
    pub degraded_sets: Vec<String>,
}

/// Scan every loaded image and assemble whatever LVM / md / filesystem stack
/// is hiding inside.
pub fn detect(images: &[Arc<DiskImage>]) -> RaidStack {
    let (stack, _assembled) = detect_with_sources(images);
    stack
}

/// Parameters for a user-driven manual RAID reconstruction (the "RAID builder"
/// fallback for arrays the automatic heuristic cannot crack). Members are given
/// in column/slot order; the geometry is taken verbatim, not searched.
#[derive(Debug, Clone, Deserialize)]
pub struct ManualRaidParams {
    /// Member image ids (already loaded), in column (slot) order. An empty
    /// string stands for a column whose disk is not available.
    pub member_image_ids: Vec<String>,
    /// md level: `-1` linear, `0`, `1`, `4`, `5`, `6` or `10`.
    pub level: i32,
    /// md layout: the RAID5/RAID6 rotation or the RAID10 copy layout
    /// (`near | far << 8 | offset << 16`); ignored for other levels.
    pub layout: u32,
    /// Stripe/chunk size in 512-byte sectors (ignored by RAID1).
    pub chunk_sectors: u32,
    /// Per-member reserved-header size in 512-byte sectors (data offset).
    pub data_offset_sectors: u64,
    /// Column of a dropped member to rebuild instead of read (older single
    /// slot form of `missing_slots`).
    #[serde(default)]
    pub missing_slot: Option<usize>,
    /// Columns to rebuild instead of read, up to the level's redundancy.
    #[serde(default)]
    pub missing_slots: Vec<usize>,
}
