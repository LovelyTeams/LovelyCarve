//! Unified filesystem dispatch: a real `Volume` trait + a `&'static`
//! `FsAdapter` registry, mirroring the `PARSERS` table in
//! [`crate::core::evidence::partitions`].
//!
//! Before this module, `fs_tree` carried five hand-written `parse_<fs>_id`
//! ladders (one per operation: children / read / stat / stream, plus the
//! `FsSession` fast path) that every new filesystem had to be threaded
//! through. Here each filesystem is one [`FsAdapter`] that knows how to open a
//! [`Volume`] for a node id, and one boxed [`Volume`] that serves the four
//! operations. The free dispatchers in `fs_tree` collapse to "front-guards +
//! [`dispatch`]".
//!
//! Behaviour is intentionally identical to the old ladders: each adapter wraps
//! the very same `open_<fs>_source` + per-volume methods, in the same order
//! (`ADAPTERS`), with the same kind-checks and error strings. `lvm:` /
//! `part:` ids are routing/synthetic and stay as explicit
//! orchestration in `fs_tree`, never reaching the registry.

mod ntfs;
use ntfs::*;
mod ext4;
use ext4::*;
mod btrfs;
use btrfs::*;
mod zfs;
use zfs::*;
mod refs;
use refs::*;

use std::sync::Arc;

use anyhow::Result;

use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::tree::fs_tree::{
    detect_mime_bytes, ntfs_deleted_snapshot_for_listing, open_ntfs_for_source, shannon_entropy,
    ChildrenOptions, FsDataRun, FsNode, FsStat, NodeKind,
};
use crate::core::filesystem::volumes::apfs::{open_apfs_source, parse_apfs_id};
use crate::core::filesystem::volumes::btrfs::{open_btrfs_source, parse_btrfs_id};
use crate::core::filesystem::volumes::cdfs::{open_cdfs_source, parse_cdfs_id};
use crate::core::filesystem::volumes::exfat::{open_exfat_source, parse_exfat_id};
use crate::core::filesystem::volumes::ext4::{canonical_ext4_id, open_ext4_source, parse_ext4_id};
use crate::core::filesystem::volumes::fat::{open_fat_source, parse_fat_id};
use crate::core::filesystem::volumes::hfsplus::{open_hfsplus_source, parse_hfsplus_id};
use crate::core::filesystem::volumes::ntfs::NtfsSource;
use crate::core::filesystem::volumes::ntfs::{parse_ntfs_id, NtfsVolume};
use crate::core::filesystem::volumes::refs::{parse_refs_id, parse_refs_snap_id};
use crate::core::filesystem::volumes::squashfs::{open_squashfs_source, parse_squashfs_id};
use crate::core::filesystem::volumes::ubi::{open_ubi_source, parse_ubi_id};
use crate::core::filesystem::volumes::ufs::{open_ufs_source, parse_ufs_id};
use crate::core::filesystem::volumes::xfs::{open_xfs_source, parse_xfs_id};
use crate::core::filesystem::volumes::zfs::{open_zfs_source, parse_zfs_id, ZfsPath};

pub(crate) struct VolumeChildrenPage {
    pub(crate) items: Vec<FsNode>,
    pub(crate) next_cursor: Option<String>,
    pub(crate) total: Option<usize>,
}

/// A [`Volume`] read asked for a data stream the file does not have: NTFS
/// metadata files keep only named streams or indexes (`$Secure`,
/// `$Extend\$ObjId`, ...), and an image captured mid-update can hold records
/// left with nothing but their names. Content scans treat the file as empty;
/// readers that must deliver its bytes (preview, hash, export) report it.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub(crate) struct MissingDataStream {
    pub(crate) message: String,
}

/// A read-only filesystem volume, addressed by node-id strings. Each method
/// re-parses the id internally so the trait surface stays uniform across the
/// heterogeneous on-disk addressing (NTFS record, ext4 inode, btrfs
/// (subvol,inode), ZFS dataset/file path, plain path). `image` is threaded
/// into `children` because the NTFS deleted-records snapshot needs it and
/// because passing it sidesteps the `&mut self`-vs-`&'a image` borrow conflict
/// in [`crate::core::filesystem::tree::fs_tree::FsSession`].
pub(crate) trait Volume {
    fn children(
        &self,
        image: &Arc<EvidenceSession>,
        id: &str,
        opts: ChildrenOptions,
    ) -> Result<Vec<FsNode>>;
    fn children_page(
        &self,
        _image: &Arc<EvidenceSession>,
        _id: &str,
        _opts: ChildrenOptions,
        _cursor: Option<&str>,
        _limit: usize,
    ) -> Result<Option<VolumeChildrenPage>> {
        Ok(None)
    }
    fn read_file(&self, id: &str, max_bytes: usize) -> Result<Vec<u8>>;
    /// Random-access read of `length` bytes at logical `offset`.
    /// `Ok(None)` means the filesystem has no seekable reader yet and the
    /// caller must fall back to [`Volume::stream_file`] (which costs
    /// O(offset)). Implement this for a filesystem whenever its data map
    /// (extents / runs / cluster chain) allows seeking.
    fn read_range(&self, _id: &str, _offset: u64, _length: usize) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn file_stat(&self, id: &str, node_id: &str) -> Result<FsStat>;
    fn stream_file(
        &self,
        id: &str,
        chunk_size: usize,
        visitor: &mut dyn FnMut(&[u8]) -> Result<()>,
    ) -> Result<u64>;
}

/// A filesystem adapter: claims node ids it recognises and opens a [`Volume`]
/// for them. `Sync` so the static registry can be shared across threads.
pub(crate) trait FsAdapter: Sync {
    /// The filesystem's stable name (matches the node-id prefix), for logs.
    fn name(&self) -> &'static str;

    /// `Ok(Some(vol))` — this adapter owns `id`, volume opened.
    /// `Ok(None)`      — not this filesystem, try the next adapter.
    /// `Err(e)`        — `id` is ours but opening failed (propagate).
    fn open<'a>(
        &self,
        image: &'a Arc<EvidenceSession>,
        id: &str,
    ) -> Result<Option<Box<dyn Volume + 'a>>>;
}

/// The dispatch order. Must mirror the old `parse_<fs>_id` ladders exactly:
/// NTFS first, ext4 last (ext4 alone canonicalises `recover:` ids). `lvm:` /
/// `part:` is deliberately absent — it is handled before
/// `dispatch` is ever called.
static ADAPTERS: &[&dyn FsAdapter] = &[
    &NtfsAdapter,
    &FatAdapter,
    &ExfatAdapter,
    &RefsAdapter,
    &CdfsAdapter,
    &SquashfsAdapter,
    &UbiAdapter,
    &ApfsAdapter,
    &HfsPlusAdapter,
    &UfsAdapter,
    &XfsAdapter,
    &ZfsAdapter,
    &BtrfsAdapter,
    &Ext4Adapter,
];

/// Open a [`Volume`] for `id`, or `Ok(None)` if no filesystem adapter claims
/// it. Front-guards (docker / folder / virtual-unalloc / deleted ranges) are
/// the caller's responsibility and must run before this.
pub(crate) fn dispatch<'a>(
    image: &'a Arc<EvidenceSession>,
    id: &str,
) -> Result<Option<Box<dyn Volume + 'a>>> {
    let image = image.filesystem_session(id);
    image.require_source()?;
    if let Some(volume) = image.external_volume(id) {
        return Ok(Some(Box::new(volume)));
    }
    for adapter in ADAPTERS {
        if let Some(volume) = adapter.open(image, id)? {
            tracing::trace!("fs_dispatch: {} claims {id}", adapter.name());
            return Ok(Some(volume));
        }
    }
    Ok(None)
}

/// The cache key identifying the volume that owns `id`, for the
/// [`crate::core::filesystem::tree::fs_tree::FsSession`] handle cache. Returns `None` for
/// node ids whose filesystem keeps no per-walk cache (FAT / ZFS / HFS+ — and
/// anything unrecognised), so the walker falls back to the per-call open path.
/// Btrfs joined the cached set: its per-open cost (superblock read + a full
/// chunk-tree and root-tree iteration to map chunks and enumerate subvolumes)
/// made the historical open-per-directory walk the dominant cost of any
/// whole-volume enumeration, especially through E01/RAID-assembled backends.
pub(crate) fn volume_root_key(id: &str) -> Option<String> {
    if let Ok((plugin_id, session_id, _)) =
        crate::core::filesystem::external_volume::decode_remote_node_id(id)
    {
        return Some(format!("plugin:{plugin_id}:{session_id}"));
    }
    if let Some((source, _)) = parse_ntfs_id(id) {
        return Some(ntfs_root_key(source));
    }
    if let Some((source, _)) = parse_ext4_id(&canonical_ext4_id(id)) {
        return Some(ext4_root_key(&source));
    }
    if let Some((source, _, _)) = parse_btrfs_id(id) {
        return Some(btrfs_root_key(&source));
    }
    if let Some((source, _)) = parse_refs_id(id) {
        return Some(format!("refs:part:{}", refs_part_id(&source)));
    }
    if let Some((source, _, _)) = parse_refs_snap_id(id) {
        return Some(format!("refs:part:{}", refs_part_id(&source)));
    }
    if let Some((source, _)) = parse_ufs_id(id) {
        return Some(format!("ufs:part:{}", ufs_part_id(&source)));
    }
    if let Some((source, _)) = parse_xfs_id(id) {
        return Some(xfs_root_key(&source));
    }
    if let Some((source, _)) = parse_cdfs_id(id) {
        return Some(format!("cdfs:part:{}", cdfs_part_id(&source)));
    }
    if let Some((source, _)) = parse_squashfs_id(id) {
        return Some(source.cache_key());
    }
    if let Some((partition_id, _)) = parse_ubi_id(id) {
        return Some(format!("ubi:part:{partition_id}"));
    }
    if let Some((source, _)) = parse_exfat_id(id) {
        return Some(format!("exfat:part:{}", exfat_part_id(&source)));
    }
    if let Some((source, _)) = parse_apfs_id(id) {
        return Some(source.cache_key());
    }
    None
}

// ---- path-addressed volumes --------------------------------------------
//
// refs / cdfs / squashfs / apfs / hfsplus / ufs / xfs / fat / exfat all share
// the same shape: `parse_<fs>_id -> (Source, String path)`, `open_<fs>_source`,
// and per-path `children`/`read_file`/`stream_file`/`file_stat`. The macro
// stamps out one adapter + wrapper per filesystem. fat/exfat additionally have
// `children_with_deleted`, handled by the `deleted` variant below.

macro_rules! path_volume {
    ($adapter:ident, $vol:ident, $name:literal, $ty:ty, $parse:path, $open:path) => {
        path_volume!($adapter, $vol, $name, $ty, $parse, $open, String);
    };
    ($adapter:ident, $vol:ident, $name:literal, $ty:ty, $parse:path, $open:path, $path_type:ty) => {
        struct $adapter;
        struct $vol {
            volume: $ty,
        }
        impl FsAdapter for $adapter {
            fn name(&self) -> &'static str {
                $name
            }
            fn open<'a>(
                &self,
                image: &'a Arc<EvidenceSession>,
                id: &str,
            ) -> Result<Option<Box<dyn Volume + 'a>>> {
                let Some((source, _path)) = $parse(id) else {
                    return Ok(None);
                };
                Ok(Some(Box::new($vol {
                    volume: $open(image, source)?,
                })))
            }
        }
        impl $vol {
            fn path_of(&self, id: &str) -> Result<$path_type> {
                $parse(id)
                    .map(|(_, path)| path)
                    .ok_or_else(|| anyhow::anyhow!(concat!($name, " id reparse failed: {}"), id))
            }
        }
    };
}

macro_rules! path_volume_impl {
    ($vol:ident, plain_range) => {
        impl Volume for $vol {
            fn children(&self, _image: &Arc<EvidenceSession>, id: &str, _opts: ChildrenOptions) -> Result<Vec<FsNode>> {
                self.volume.children(&self.path_of(id)?)
            }
            fn read_range(&self, id: &str, offset: u64, length: usize) -> Result<Option<Vec<u8>>> {
                self.volume.read_file_range(&self.path_of(id)?, offset, length).map(Some)
            }
            path_volume_impl!(@common $vol);
        }
    };
    ($vol:ident, deleted_range) => {
        impl Volume for $vol {
            fn children(&self, _image: &Arc<EvidenceSession>, id: &str, opts: ChildrenOptions) -> Result<Vec<FsNode>> {
                let path = self.path_of(id)?;
                if opts.include_deleted_ntfs { self.volume.children_with_deleted(&path) }
                else { self.volume.children(&path) }
            }
            fn read_range(&self, id: &str, offset: u64, length: usize) -> Result<Option<Vec<u8>>> {
                self.volume.read_file_range(&self.path_of(id)?, offset, length).map(Some)
            }
            path_volume_impl!(@common $vol);
        }
    };
    ($vol:ident, deleted) => {
        impl Volume for $vol {
            fn children(&self, _image: &Arc<EvidenceSession>, id: &str, opts: ChildrenOptions) -> Result<Vec<FsNode>> {
                let path = self.path_of(id)?;
                if opts.include_deleted_ntfs {
                    self.volume.children_with_deleted(&path)
                } else {
                    self.volume.children(&path)
                }
            }
            path_volume_impl!(@common $vol);
        }
    };
    ($vol:ident, plain) => {
        impl Volume for $vol {
            fn children(&self, _image: &Arc<EvidenceSession>, id: &str, _opts: ChildrenOptions) -> Result<Vec<FsNode>> {
                self.volume.children(&self.path_of(id)?)
            }
            path_volume_impl!(@common $vol);
        }
    };
    (@common $vol:ident) => {
        fn read_file(&self, id: &str, max_bytes: usize) -> Result<Vec<u8>> {
            self.volume.read_file(&self.path_of(id)?, max_bytes)
        }
        fn file_stat(&self, id: &str, node_id: &str) -> Result<FsStat> {
            self.volume.file_stat(&self.path_of(id)?, node_id)
        }
        fn stream_file(
            &self,
            id: &str,
            chunk_size: usize,
            visitor: &mut dyn FnMut(&[u8]) -> Result<()>,
        ) -> Result<u64> {
            self.volume.stream_file(&self.path_of(id)?, chunk_size, visitor)
        }
    };
}

path_volume!(
    CdfsAdapter,
    CdfsVol,
    "cdfs",
    crate::core::filesystem::volumes::cdfs::CdfsVolume,
    parse_cdfs_id,
    open_cdfs_source
);
path_volume_impl!(CdfsVol, plain);
path_volume!(
    SquashfsAdapter,
    SquashfsVol,
    "squashfs",
    crate::core::filesystem::volumes::squashfs::SquashfsVolume,
    parse_squashfs_id,
    open_squashfs_source
);
path_volume_impl!(SquashfsVol, plain);
path_volume!(
    UbiAdapter,
    UbiVol,
    "ubi",
    crate::core::filesystem::volumes::ubi::UbiVolume,
    parse_ubi_id,
    open_ubi_source,
    u32
);
path_volume_impl!(UbiVol, plain_range);
path_volume!(
    ApfsAdapter,
    ApfsVol,
    "apfs",
    crate::core::filesystem::volumes::apfs::ApfsVolume,
    parse_apfs_id,
    open_apfs_source
);
path_volume_impl!(ApfsVol, plain_range);
path_volume!(
    HfsPlusAdapter,
    HfsPlusVol,
    "hfsplus",
    crate::core::filesystem::volumes::hfsplus::HfsPlusVolume,
    parse_hfsplus_id,
    open_hfsplus_source
);
path_volume_impl!(HfsPlusVol, plain_range);
path_volume!(
    UfsAdapter,
    UfsVol,
    "ufs",
    crate::core::filesystem::volumes::ufs::UfsVolume,
    parse_ufs_id,
    open_ufs_source
);
path_volume_impl!(UfsVol, plain);
path_volume!(
    XfsAdapter,
    XfsVol,
    "xfs",
    crate::core::filesystem::volumes::xfs::XfsVolume,
    parse_xfs_id,
    open_xfs_source,
    Vec<u8>
);
path_volume_impl!(XfsVol, plain_range);
path_volume!(
    FatAdapter,
    FatVol,
    "fat",
    crate::core::filesystem::volumes::fat::FatVolume,
    parse_fat_id,
    open_fat_source
);
path_volume_impl!(FatVol, deleted_range);
path_volume!(
    ExfatAdapter,
    ExfatVol,
    "exfat",
    crate::core::filesystem::volumes::exfat::ExfatVolume,
    parse_exfat_id,
    open_exfat_source
);
path_volume_impl!(ExfatVol, deleted_range);

// ---- shared helpers -----------------------------------------------------

/// Builds an [`FsStat`] from a sampled prefix, identical to the inline blocks
/// the ext4 / btrfs dispatchers used before the refactor (mime sniff, 16-byte
/// magic, Shannon entropy — all skipped for an empty sample).
pub(crate) fn build_stat(
    node_id: &str,
    size: u64,
    inode: Option<u64>,
    mode: Option<u16>,
    sample: &[u8],
    data_runs: Vec<FsDataRun>,
) -> FsStat {
    FsStat {
        node_id: node_id.to_string(),
        size,
        inode,
        mode,
        mime: detect_mime_bytes(sample).to_string(),
        magic_hex: if sample.is_empty() {
            None
        } else {
            Some(
                sample
                    .iter()
                    .take(16)
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        },
        entropy: if sample.is_empty() {
            None
        } else {
            Some(shannon_entropy(sample))
        },
        data_runs,
    }
}

// Cache-key part-id extractors. These mirror the old `*_root_key` helpers in
// `fs_tree`; each filesystem's Source carries the partition id we key on.
fn refs_part_id(source: &crate::core::filesystem::volumes::refs::RefsSource) -> u8 {
    match source {
        crate::core::filesystem::volumes::refs::RefsSource::Partition(part_id) => *part_id,
    }
}
fn ufs_part_id(source: &crate::core::filesystem::volumes::ufs::UfsSource) -> u8 {
    match source {
        crate::core::filesystem::volumes::ufs::UfsSource::Partition(part_id) => *part_id,
    }
}
fn cdfs_part_id(source: &crate::core::filesystem::volumes::cdfs::CdfsSource) -> u8 {
    match source {
        crate::core::filesystem::volumes::cdfs::CdfsSource::Partition(part_id) => *part_id,
    }
}
fn exfat_part_id(source: &crate::core::filesystem::volumes::exfat::ExfatSource) -> u8 {
    match source {
        crate::core::filesystem::volumes::exfat::ExfatSource::Partition(part_id) => *part_id,
    }
}
fn xfs_root_key(source: &crate::core::filesystem::volumes::xfs::XfsSource) -> String {
    match source {
        crate::core::filesystem::volumes::xfs::XfsSource::Partition(part_id) => {
            format!("xfs:part:{part_id}")
        }
        crate::core::filesystem::volumes::xfs::XfsSource::Lvm { part_id, lv_index } => {
            format!("xfs:lvm:{part_id}:{lv_index}")
        }
    }
}
fn ext4_root_key(source: &crate::core::filesystem::volumes::ext4::Ext4Source) -> String {
    match source {
        crate::core::filesystem::volumes::ext4::Ext4Source::Partition(part_id) => {
            format!("part:{part_id}")
        }
        crate::core::filesystem::volumes::ext4::Ext4Source::Lvm { part_id, lv_index } => {
            format!("lvm:{part_id}:{lv_index}")
        }
    }
}
fn btrfs_root_key(source: &crate::core::filesystem::volumes::btrfs::BtrfsSource) -> String {
    match source {
        crate::core::filesystem::volumes::btrfs::BtrfsSource::Partition(part_id) => {
            format!("btrfs:part:{part_id}")
        }
        crate::core::filesystem::volumes::btrfs::BtrfsSource::Lvm { part_id, lv_index } => {
            format!("btrfs:lvm:{part_id}:{lv_index}")
        }
    }
}
