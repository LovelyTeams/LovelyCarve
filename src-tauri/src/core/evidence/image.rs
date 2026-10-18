//! Disk image loading and partition detection.
//!
//! Opens raw `.dd / .raw / .img` files via `memmap2` and container formats
//! such as E01/EWF, VMDK, QCOW2, VHD/VHDX, and PMFX through the matching
//! libyal DLL or native reader. Partition tables are parsed eagerly over the
//! logical disk byte stream; filesystems are detected lazily when the user
//! navigates into a partition.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context};
use memmap2::Mmap;
use parking_lot::{Condvar, Mutex as FairMutex};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::core::evidence::crypto::bde_image::{
    BdeEncryptionMethod, BdeUnlockKey, VolumeByteReader,
};
use crate::core::evidence::crypto::bde_vmk;
use crate::core::evidence::crypto::bitlocker::{self, UnlockedVolume};
use crate::core::evidence::partitions::{self, SectorSource, TableScheme};
use crate::core::evidence::physical_disk;
use crate::core::evidence::readers::ad1_image::{self, Ad1Image};
use crate::core::evidence::readers::docker_image::DockerImage;
use crate::core::evidence::readers::ewf_image::{EwfAcquiryInfo, EwfHandle};
use crate::core::evidence::readers::folder_image::FolderImage;
use crate::core::evidence::readers::pmfx::{self, PmfxImage};
use crate::core::evidence::readers::sevenz_image::SevenzImage;
use crate::core::evidence::readers::tar_image::TarImage;
use crate::core::evidence::readers::vhd_image::VhdHandle;
use crate::core::evidence::readers::vmdk_image::VmdkHandle;
use crate::core::evidence::readers::zip_image::ZipImage;

pub use crate::core::evidence::partitions::{detect_filesystem, Partition, PartitionKind};

pub const INDEX_ONLY_READ_ERROR: &str = "原镜像不可用，当前仅能浏览已有索引；请恢复原镜像后重试。";

/// Provenance of a session-only image derived from another loaded image.
/// Derived images are never written to the case evidence list and are
/// unloaded together with their parent.
#[derive(Debug, Clone)]
pub enum DerivedImage {
    /// Plaintext view of an unlocked LUKS / VeraCrypt container.
    Decrypted { parent: String },
    /// Point-in-time view of an NTFS volume reconstructed from a shadow copy.
    ShadowCopy {
        parent: String,
        origin: crate::core::evidence::vss_image::ShadowCopyOrigin,
    },
}

impl DerivedImage {
    /// Id of the loaded image this one was derived from.
    pub fn parent(&self) -> &str {
        match self {
            Self::Decrypted { parent } | Self::ShadowCopy { parent, .. } => parent,
        }
    }
}

#[derive(Debug)]
pub struct DiskImage {
    /// Set for a session-only image derived from another loaded image.
    pub derived: Option<DerivedImage>,
    pub id: String,
    pub path: PathBuf,
    source: ImageSource,
    pub sector_size: usize,
    pub partitions: Vec<Partition>,
    /// Set when the on-disk file is a Docker / OCI image archive
    /// (`docker save` tar or OCI layout tar). When present, partition
    /// detection is skipped and `core::filesystem::tree::fs_tree` routes navigation /
    /// reads through the merged container filesystem instead.
    pub docker: Option<Arc<DockerImage>>,
    /// Set when the user picked a folder as the evidence source. When
    /// present, partition detection is skipped and `core::filesystem::tree::fs_tree` routes
    /// navigation through the on-disk folder tree. Byte-level commands
    /// such as hex reject folder-backed images explicitly.
    pub folder: Option<Arc<FolderImage>>,
    /// Set when the on-disk file is a plain (uncompressed) `.tar` archive that
    /// is not a Docker / OCI image. Like `docker` / `folder`, partition
    /// detection is skipped and `core::filesystem::tree::fs_tree` routes navigation through the
    /// archive's merged file tree. Byte-level commands (hex) still work — they
    /// read the raw outer-tar bytes through the mmap.
    pub tar: Option<Arc<TarImage>>,
    /// Set when the on-disk file is a `.zip` archive (logical extraction /
    /// triage collection / evidence bundle). Mirrors `tar`: partition detection
    /// is skipped and `core::filesystem::tree::fs_tree` routes navigation through the archive's
    /// merged file tree, decompressing DEFLATE members on read. Byte-level
    /// commands (hex) read the raw outer-zip bytes through the mmap.
    pub zip: Option<Arc<ZipImage>>,
    /// Set when the on-disk file is a `.7z` archive. Mirrors `zip` as a
    /// logical evidence tree, with members decompressed on demand.
    pub sevenz: Option<Arc<SevenzImage>>,
    /// Set when the evidence is an AccessData AD1 logical image (FTK Imager
    /// "Custom Content Image"). Mirrors `zip`: navigation goes through the
    /// image's item tree, file data inflates on demand, and the hex view reads
    /// the raw segment bytes.
    pub ad1: Option<Arc<Ad1Image>>,
    unlocked_bde: Mutex<HashMap<u8, Arc<UnlockedVolume>>>,
    /// Located 64 KiB block cache, present only for FFI-backed sources
    /// (EWF / VMDK / VHDX / PMFX) where each miss costs a decompress. Same-
    /// block misses are single-flighted so concurrent readers wait for the
    /// first backend read instead of repeating the same decompression. `None`
    /// for mmap-backed sources, which are already zero-copy.
    block_cache: Option<SharedBlockCache>,
    /// Absolute-offset spans that were zero-filled because the backend could
    /// not read them (P0-5 bad-sector tolerance). Recorded the first time a
    /// span is read so the fabrication stays forensically auditable: a re-read
    /// served from [`Self::block_cache`] returns the synthetic zeros silently,
    /// so without this registry a caller could not tell error-induced zeros
    /// from genuine disk content. Queried via [`Self::bad_regions`].
    bad_regions: Mutex<Vec<BadRegion>>,
    /// Which partition-table layout claimed the device at open, and the
    /// on-disk disk identifier (GPT disk GUID / MBR signature) when the
    /// scheme defines one. Display data for the image-info page.
    pub table_scheme: Option<TableScheme>,
    pub disk_id: Option<String>,
    /// On-disk footprint of the source file(s) — for compressed containers
    /// (EWF) this is the summed segment-file size, distinct from the logical
    /// media size. `None` for sources without a stat-able backing file.
    pub on_disk_bytes: Option<u64>,
    /// Source-file timestamps (Unix epoch milliseconds), best-effort.
    pub modified_unix_ms: Option<i64>,
    pub created_unix_ms: Option<i64>,
    /// EWF acquisition metadata captured at open; `None` for non-EWF sources.
    pub ewf_acquiry: Option<EwfAcquiryInfo>,
    /// Device-reported serial captured from a physical-disk handle on Windows.
    /// None for other source types, unsupported queries and other platforms.
    pub physical_drive_serial: Option<physical_disk::DriveSerial>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceFileSnapshot {
    path: PathBuf,
    len: u64,
    modified_unix_ms: Option<i64>,
    created_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageInfo {
    /// Session-unlocked partitions; source bytes and original disk geometry remain unchanged.
    #[serde(default)]
    pub unlocked_partitions: Vec<u8>,
    /// Metadata-only session backed by published case indexes; source bytes are unavailable.
    #[serde(default)]
    pub index_only: bool,
    pub id: String,
    pub path: String,
    pub source_format: ImageFormat,
    pub size_bytes: u64,
    pub sector_size: usize,
    pub partitions: Vec<Partition>,
    /// Partition-table layout that claimed the device (`None` when nothing
    /// matched) and its disk identifier (GPT disk GUID / MBR signature).
    #[serde(default)]
    pub table_scheme: Option<TableScheme>,
    #[serde(default)]
    pub disk_id: Option<String>,
    /// On-disk footprint of the source file(s); differs from `size_bytes`
    /// for compressed containers such as EWF.
    #[serde(default)]
    pub on_disk_bytes: Option<u64>,
    /// Source-file timestamps, Unix epoch milliseconds.
    #[serde(default)]
    pub modified_unix_ms: Option<i64>,
    #[serde(default)]
    pub created_unix_ms: Option<i64>,
    /// EWF acquisition metadata; `None` for non-EWF sources.
    #[serde(default)]
    pub ewf: Option<EwfAcquiryInfo>,
    /// Where a shadow-copy session image comes from; `None` otherwise.
    #[serde(default)]
    pub shadow_copy: Option<crate::core::evidence::vss_image::ShadowCopyOrigin>,
    /// iTunes/Finder backup metadata, including whether its logical view is locked.
    #[serde(default)]
    pub ios_backup: Option<crate::core::evidence::readers::folder_image::IosBackupInfo>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    Raw,
    Ewf,
    Vmdk,
    /// QEMU copy-on-write v2/v3 virtual disk, exposed as its sparse logical
    /// guest byte stream through the vendored pure-Rust reader.
    Qcow2,
    /// Parallels plain/sparse/split/differencing logical guest disk.
    Phdi,
    /// VirtualBox VDI 1.1 dynamic or fixed disk, exposed as its sparse logical
    /// guest byte stream through the vendored pure-Rust reader.
    Vdi,
    Vhd,
    Vhdx,
    Dmg,
    Pmfx,
    /// Xiaomi `HDR1` firmware container. Filesystem-bearing segments are
    /// exposed as byte-addressed volumes while raw reads retain the exact
    /// original container bytes.
    XiaomiFirmware,
    /// LDPlayer (雷电模拟器) `.ldbk` emulator backup — a 7z (LZMA2) container of
    /// `*.vmdk` virtual disks. Each VMDK is de-sparsed through the native sparse reader and the
    /// disks are concatenated into one synthetic block stream; partitions are
    /// detected per-VMDK and re-based, so navigation flows through the normal
    /// `part:N` pipeline. See `core::evidence::readers::ldplayer_image`.
    Ldplayer,
    /// Docker / OCI image archive (`docker save` tar or OCI image-layout
    /// tar). The on-disk file is treated as a merged read-only container
    /// filesystem rather than a block device — partitions are intentionally
    /// empty and navigation flows through `core::evidence::readers::docker_image` instead.
    Docker,
    /// Plain (uncompressed) `.tar` archive that is not a Docker / OCI image —
    /// e.g. a phone / filesystem extraction delivered as a tarball. Surfaced
    /// as a merged read-only file tree via `core::evidence::readers::tar_image`; partitions are
    /// empty and the hex view reads the raw outer-tar bytes.
    Tar,
    /// Android `adb backup` (`.ab`) or MIUI `.bak`, with a decoded TAR tree.
    /// Byte-level access retains the original backup container, including headers.
    AndroidBackup,
    /// `.zip` archive treated as a logical-evidence file tree (KAPE triage,
    /// logical extraction, collected-evidence bundle). Surfaced as a merged
    /// read-only file tree via `core::evidence::readers::zip_image`; partitions are empty and the
    /// hex view reads the raw outer-zip bytes. DEFLATE members are decompressed
    /// on read; STORED members read zero-copy from the mmap.
    Zip,
    /// `.7z` archive treated as logical evidence and surfaced as a merged
    /// read-only file tree. Members are decompressed on demand.
    Sevenz,
    /// AccessData AD1 logical image (`.ad1` plus `.ad2`… segments), surfaced
    /// as a read-only file tree via `core::evidence::readers::ad1_image`; partitions are empty
    /// and the hex view reads the raw segment bytes.
    Ad1,
    /// Raw block device exposed by the host OS (e.g. `\\.\PhysicalDrive0`
    /// on Windows, `/dev/sdX` on Linux). Reads go through a positioned
    /// file handle with sector-alignment handling on Windows; downstream
    /// partition / filesystem detection is identical to a Raw image.
    PhysicalDisk,
    /// Logical-evidence source backed by an on-disk folder. Has no
    /// partition table — `core::filesystem::tree::fs_tree` resolves children / reads /
    /// stats through `core::filesystem::volumes::folder`.
    Folder,
    /// Synthetic image backed by an in-memory `SectorSource` — currently
    /// used by the RAID auto-detection path to expose an assembled mdadm
    /// array as if it were a regular disk image.
    Assembled,
    /// Temporary plaintext view, available only after explicit unlock.
    Decrypted,
    /// Temporary point-in-time view of an NTFS volume, reconstructed from one
    /// of its Volume Shadow Copies.
    ShadowCopy,
}

/// Coarse progress signal emitted while a `DiskImage` is being opened, so the
/// frontend can show the current phase (and a percentage where one is knowable)
/// instead of a featureless spinner. Carries no UI strings — the command layer
/// maps each variant to localized text before emitting an event.
#[derive(Debug, Clone)]
pub enum OpenProgress {
    /// Opening the container (mmap / FFI handle / archive header). No percent.
    Opening,
    /// Decompressing data into memory (e.g. an `.ldbk` VMDK member). Bytes.
    Decompressing {
        label: String,
        done: u64,
        total: u64,
    },
    /// Scanning the partition table(s). No percent.
    DetectingPartitions,
    /// Probing each partition's filesystem. `done`/`total` are partition counts.
    DetectingFilesystems { done: usize, total: usize },
}

/// Sink for [`OpenProgress`] threaded through the open path. `Send + Sync` so it
/// can be invoked from the `spawn_blocking` worker that runs `image_load`.
pub type OpenObserver<'a> = &'a (dyn Fn(OpenProgress) + Send + Sync);

/// A no-op observer for the many callers (tests, RAID assembly) that don't care
/// about open progress.
fn noop_observer(_: OpenProgress) {}

/// Where an opener may keep listings derived from the evidence, so the next
/// open of the same source skips rebuilding them. Losing such a listing only
/// costs that rebuild.
#[derive(Debug, Clone, Copy)]
pub enum EvidenceCache<'a> {
    /// The open belongs to no case: derive everything again on every open.
    Disabled,
    /// The case's cache directory of this evidence (`evidence/<uid>/cache`).
    Directory(&'a Path),
}

/// Reads a BitLocker volume directly out of the decoded image
/// (raw / E01 / VMDK alike) through a bounded Rust reader — see
/// `core::evidence::crypto::bitlocker::unlock_location`. The handle holds only a `Weak` to the
/// image, so an unlocked volume never keeps the image (and its mmap / evidence
/// file handle) alive past case-close.
impl VolumeByteReader for DiskImage {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> anyhow::Result<usize> {
        let (result, faults) = self.collect_read_faults(|| self.read_into(offset, buf));
        anyhow::ensure!(
            !faults.read_faults_detected,
            "BitLocker ciphertext contains {} synthesized source bytes; refusing decryption",
            faults.synthetic_bytes
        );
        result
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BitlockerStatus {
    pub locked: bool,
    pub size_bytes: Option<u64>,
    pub encryption_method: Option<String>,
    pub cached_dump: Option<String>,
}

fn encryption_method_label(method: BdeEncryptionMethod) -> String {
    match method {
        BdeEncryptionMethod::None => "none".into(),
        BdeEncryptionMethod::Aes128CbcDiffuser => "AES-128-CBC + diffuser".into(),
        BdeEncryptionMethod::Aes256CbcDiffuser => "AES-256-CBC + diffuser".into(),
        BdeEncryptionMethod::Aes128Cbc => "AES-128-CBC".into(),
        BdeEncryptionMethod::Aes256Cbc => "AES-256-CBC".into(),
        BdeEncryptionMethod::Aes128Xts => "AES-128-XTS".into(),
        BdeEncryptionMethod::Aes256Xts => "AES-256-XTS".into(),
        BdeEncryptionMethod::Other(value) => format!("unknown (0x{value:04x})"),
    }
}

mod faults;
use faults::*;

mod cache;
use cache::*;

mod source;
use source::*;

mod detect;
use detect::*;

mod open;

mod encryption;

mod read;

mod metadata;
pub(crate) use metadata::find_partition;

pub use faults::{rebase_bad_regions, BadRegion, ReadFaultPolicy, ReadFaultReport};
#[cfg(test)]
mod android_backup_tests;
#[cfg(test)]
mod faults_tests;
#[cfg(test)]
mod phdi_tests;
#[cfg(test)]
mod smraw_tests;
#[cfg(test)]
mod tests;
