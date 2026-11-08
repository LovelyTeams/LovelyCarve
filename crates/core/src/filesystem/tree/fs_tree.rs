//! Filesystem dispatcher and shared types.
//!
//! Owns the cross-filesystem types (`FsNode`, `FsStat`, ...) and routes
//! requests to the per-filesystem adapters under `core::filesystem::volumes::*`. The
//! dispatcher itself does not know about on-disk layouts; each adapter is
//! responsible for its own parsing.

mod read;
mod session;
mod walk;

mod navigation;
use navigation::*;
pub use navigation::{children_for_image, children_for_mount};

use std::error::Error;
use std::fmt;
use std::sync::Arc;

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::filesystem::fs_dispatch::Volume as _;

use crate::evidence::image::{find_partition, PartitionKind};
use crate::filesystem::docker::host_docker;
use crate::filesystem::evidence_session::EvidenceSession;
pub use crate::filesystem::tree::fs_tree_export::{
    export_dir, export_dir_with_fault_report, export_dir_zip, export_dir_zip_with_fault_report,
    export_file, export_file_with_fault_report, FsExportResult, FsExportSummary,
};
pub use crate::filesystem::tree::fs_tree_ext4::ext4_journal;
pub use crate::filesystem::tree::fs_tree_nodes::partition_node;
pub(crate) use crate::filesystem::tree::fs_tree_nodes::{
    disk_gap_nodes, fat_system_nodes, fs_unalloc_nodes,
};
pub(crate) use crate::filesystem::tree::fs_tree_ntfs::{
    bitlocker_partition_children, ntfs_deleted_snapshot_for_listing, ntfs_lvm_root_children,
    ntfs_root_children, open_ntfs_for_source,
};
pub use crate::filesystem::tree::fs_tree_ntfs::{
    ntfs_usn_journal, open_ntfs_volume_for_key, prefetch_ntfs_deleted_snapshots,
};
pub use crate::filesystem::tree::fs_tree_read::stream_file_chunks;
pub use crate::filesystem::tree::fs_tree_read::{
    file_stat, read_file, read_file_limited, read_file_range,
};
pub use crate::filesystem::tree::fs_tree_streams::{
    export_named_stream, list_data_streams, read_named_stream,
};
pub use crate::filesystem::tree::fs_tree_walk::{
    directory_stats, walk_image, walk_image_from, FsDirStats, FsExtensionCount, FsWalkEntry,
};
use crate::filesystem::unalloc::parse_virtual_unalloc_id;
use crate::filesystem::volumes::ad1 as ad1_volume;
use crate::filesystem::volumes::apfs::apfs_volume_nodes;
use crate::filesystem::volumes::btrfs::{open_btrfs_source, BtrfsSource};
use crate::filesystem::volumes::cdfs::{open_cdfs_source, CdfsSource, CdfsVolume};
use crate::filesystem::volumes::docker as docker_volume;
use crate::filesystem::volumes::exfat::{open_exfat_source, ExfatSource, ExfatVolume};
use crate::filesystem::volumes::ext4::{open_ext4_source, Ext4Source, Ext4Volume};
use crate::filesystem::volumes::fat::FatVolume;
use crate::filesystem::volumes::folder as folder_volume;
use crate::filesystem::volumes::hfsplus::HfsPlusVolume;
use crate::filesystem::volumes::lvm::{parse_lvm_id, LvmVolume};
use crate::filesystem::volumes::ntfs::parse_ntfs_ads_id;
use crate::filesystem::volumes::refs::{
    open_refs_source, parse_refs_snap_id, RefsSource, RefsVolume,
};
use crate::filesystem::volumes::sevenz as sevenz_volume;
use crate::filesystem::volumes::squashfs::{open_squashfs_source, SquashfsSource, SquashfsVolume};
use crate::filesystem::volumes::tar as tar_volume;
use crate::filesystem::volumes::ufs::{open_ufs_source, UfsSource, UfsVolume};
use crate::filesystem::volumes::xfs::{open_xfs_source, XfsSource, XfsVolume};
use crate::filesystem::volumes::zfs::ZfsVolume;
use crate::filesystem::volumes::zip as zip_volume;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsNode {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub kind: NodeKind,
    pub size: u64,
    pub deleted: bool,
    pub inode: Option<u64>,
    pub timestamps: Mactb,
    /// 符号链接指向的目标路径（按链接内容原样展示，不解析/不跟随）。
    /// 仅在卷适配器能廉价取到时填充；`#[serde(default)]` 让旧案件快照
    /// （持久化过 FsNode）仍可反序列化。
    #[serde(default)]
    pub link_target: Option<String>,
    /// 该节点是否拥有 ReFS 文件快照流（`refsutil streamsnapshot`）。仅 ReFS
    /// 快路径在快照构建时置位（见 [`crate::catalog::snapshot::volume_snapshot_refs`]）；快照
    /// 伪文件行本身恒为 false。`#[serde(default)]` 让旧 `.dirz` / 案件快照仍可
    /// 反序列化（缺该字段时为 false）。
    #[serde(default)]
    pub has_snapshot: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    Dir,
    File,
    Symlink,
    Unallocated,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Mactb {
    pub modified: Option<String>,
    pub accessed: Option<String>,
    pub created: Option<String>,
    pub mft_changed: Option<String>,
    pub birth: Option<String>,
}

impl Mactb {
    /// Parses one stored RFC3339 timestamp back into a Unix epoch second
    /// count. Returns `None` for absent or unparseable values. Used for
    /// chronological (not lexicographic) ordering and for numeric body-file /
    /// timeline export. The strings are written UTC-normalised by every
    /// volume adapter, so this round-trips losslessly to seconds.
    fn epoch_of(value: &Option<String>) -> Option<i64> {
        let raw = value.as_deref()?;
        chrono::DateTime::parse_from_rfc3339(raw)
            .ok()
            .map(|dt| dt.timestamp())
    }

    pub fn modified_epoch(&self) -> Option<i64> {
        Self::epoch_of(&self.modified)
    }
    pub fn accessed_epoch(&self) -> Option<i64> {
        Self::epoch_of(&self.accessed)
    }
    pub fn created_epoch(&self) -> Option<i64> {
        Self::epoch_of(&self.created)
    }
    pub fn mft_changed_epoch(&self) -> Option<i64> {
        Self::epoch_of(&self.mft_changed)
    }
    pub fn birth_epoch(&self) -> Option<i64> {
        Self::epoch_of(&self.birth)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsDataRun {
    pub logical_offset: u64,
    pub physical_offset: u64,
    pub length: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsStat {
    pub node_id: String,
    pub size: u64,
    pub inode: Option<u64>,
    pub mode: Option<u16>,
    pub mime: String,
    pub magic_hex: Option<String>,
    pub entropy: Option<f64>,
    pub data_runs: Vec<FsDataRun>,
}

#[derive(Clone, Copy)]
pub struct ChildrenOptions {
    pub include_deleted_ntfs: bool,
    pub include_unallocated_nodes: bool,
}

pub(crate) fn detect_mime_bytes(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else if bytes.starts_with(b"%PDF") {
        "application/pdf"
    } else if bytes.starts_with(b"PK\x03\x04") {
        "application/zip"
    } else if bytes.starts_with(b"\x7fELF") {
        "application/x-elf"
    } else if bytes.starts_with(b"SQLite format 3") {
        "application/vnd.sqlite3"
    } else if bytes
        .iter()
        .take(512)
        .all(|b| *b == 9 || *b == 10 || *b == 13 || (*b >= 32 && *b < 127) || *b >= 0x80)
    {
        "text/plain"
    } else {
        "application/octet-stream"
    }
}

pub(crate) fn shannon_entropy(bytes: &[u8]) -> f64 {
    let mut counts = [0u64; 256];
    for byte in bytes {
        counts[*byte as usize] += 1;
    }
    let len = bytes.len() as f64;
    counts
        .into_iter()
        .filter(|count| *count > 0)
        .map(|count| {
            let p = count as f64 / len;
            -p * p.log2()
        })
        .sum()
}

/// The ext4 root directory inode. Children of this inode are filed under the
/// volume root key (`"part:N"` / `"lvm:N:M"`) rather than an `ext4:…` node id,
/// matching what a live root listing returns.
const EXT4_ROOT_INODE: u64 = 2;

/// Request-scoped cache of opened filesystem volumes.
///
/// The generic dispatcher [`children_for_image`] re-opens the volume on
/// *every* directory listing. For ext4 that re-parses the superblock plus
/// every block-group descriptor (`Ext4::load`), and for LVM-backed ext4 also
/// re-reads the 4 MiB LVM metadata (≈18 min for ~150k entries on a 38 GiB
/// Ubuntu LVM root). For ReFS it re-runs `fsapi_volume_mount` — rebuilding
/// librefs' block map and discarding its warm node LRU — once per directory.
/// (ReFS now has its own single-pass fast path in
/// [`crate::catalog::snapshot::volume_snapshot_refs`], mirroring NTFS/APFS/Btrfs; this
/// cached walker is only its fallback when the loaded DLL predates the
/// `fsapi_volume_walk_tree` export.) The same per-open tax hits UFS (re-parses
/// the superblock + every cylinder group, ~30–60 ms), XFS (~5–15 ms), SquashFS
/// (re-decompresses the inode/dir tables), exFAT (re-reads the whole FAT
/// region) and APFS (checkpoint scan + dual OMAP B-tree lookups). This walker
/// opens each such volume once and reuses the handle for the whole walk,
/// avoiding repeated volume initialization during traversal. FAT
/// (boot-sector-only open) and anything else it does not recognise fall back to
/// the generic dispatcher, so NTFS (its own fast path) etc. are unaffected.
///
/// The first implementation lived under the `SnapshotWalker` name because
/// volume snapshots were the first hot path that needed it. Directory export,
/// directory statistics and hash scans now use the same request-level session
/// so each command opens a volume once instead of once per directory or file.
pub struct FsSession<'a> {
    image: &'a Arc<EvidenceSession>,
    /// One cached [`Volume`] handle per volume-root, keyed by
    /// [`crate::filesystem::fs_dispatch::volume_root_key`]. Replaces the per-kind
    /// `HashMap`s the walker used before P0-10: the registry already knows how
    /// to open every filesystem, so the walker only needs to remember the open
    /// handle. Filesystems with no fast-path cache (FAT / ZFS / HFS+) return
    /// `None` from `volume_root_key` and fall through to
    /// [`children_for_image`] / [`stream_file_chunks`] unchanged.
    volumes: HashMap<String, Box<dyn crate::filesystem::fs_dispatch::Volume + 'a>>,
}

pub type SnapshotWalker<'a> = FsSession<'a>;

/// `prune` argument for the walk variants that descend everywhere.
fn no_prune(_: &FsNode) -> bool {
    false
}

#[cfg(test)]
mod tests;
