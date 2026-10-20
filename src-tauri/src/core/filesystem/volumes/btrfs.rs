//! Read-only Btrfs filesystem walker.
//!
//! Implements just enough of the on-disk format to enumerate the default
//! subvolume's directory tree and stream regular files. Built on top of the
//! `btrfs-diskformat` crate, which provides the zero-copy structs for the
//! superblock, B-tree node header, items, key pointers, chunks, stripes,
//! inode items and root items. Higher-level concepts not supplied by that
//! crate — the chunk-tree → logical/physical map, the leaf-cursor B-tree
//! iterator, file-extent and dir-item payload parsers, and the zlib/zstd
//! decompressors — are provided here.
//!
//! The walker assumes a single-device filesystem and the first stripe in
//! every chunk. RAID0/1/10/5/6 striping, multi-device pools, encrypted
//! extents are not supported and are surfaced as errors; we keep the walker
//! honest rather than silently returning wrong bytes.

mod directory;
mod open;
mod tree;
use tree::*;
mod codec;
mod data;
#[cfg(test)]
pub(crate) mod test_support;
use codec::*;

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::io::Read as _;

use anyhow::{anyhow, bail, Context};
use btrfs_diskformat::{Chunk, InodeItem, Item, Key, KeyPointer, RootItem, Stripe};
use chrono::{TimeZone, Utc};
use zerocopy::FromBytes;

use crate::core::evidence::image::find_partition;
use crate::core::evidence::image::DiskImage;
use crate::core::evidence::partitions::BTRFS_PRIMARY_SUPERBLOCK;
use crate::core::filesystem::tree::fs_tree::{FsDataRun, FsNode, Mactb, NodeKind};

/// Reserved object IDs (subset). See `fs/btrfs/ctree.h` in the kernel.
#[allow(dead_code)]
const ROOT_TREE_OBJECTID: u64 = 1;
const FS_TREE_OBJECTID: u64 = 5;
/// First object ID available for user-visible inodes. Each subvolume's root
/// directory is allocated at exactly this value, so it doubles as the
/// canonical "root inode" for the default FS tree.
pub(crate) const FIRST_FREE_OBJECTID: u64 = 256;
/// Highest ordinary subvolume objectid. Anything past this is a kernel-only
/// reserved tree (orphan / data-reloc / quota) and should not be surfaced as
/// a subvolume in the UI.
const LAST_FREE_OBJECTID: u64 = u64::MAX - 0xFF;

/// Key types we read. The full set is large; these are the ones the walker
/// actually consults to enumerate directories and stream files.
const KEY_INODE_ITEM: u8 = 1;
const KEY_DIR_INDEX: u8 = 96;
const KEY_EXTENT_DATA: u8 = 108;
const KEY_ROOT_ITEM: u8 = 132;
const KEY_ROOT_REF: u8 = 156;
const KEY_CHUNK_ITEM: u8 = 228;

const HEADER_SIZE: usize = 101;
const ITEM_RECORD_SIZE: usize = 25;
const KEY_POINTER_SIZE: usize = 33;
const KEY_RECORD_SIZE: usize = 17;

/// Btrfs `BTRFS_FT_*` directory entry types. Mirrors POSIX d_type.
const FT_REGULAR: u8 = 1;
const FT_DIRECTORY: u8 = 2;
const FT_SYMLINK: u8 = 7;

/// `btrfs_file_extent_item` types.
const EXTENT_INLINE: u8 = 0;
#[allow(dead_code)]
const EXTENT_REGULAR: u8 = 1;
const EXTENT_PREALLOC: u8 = 2;

/// `btrfs_file_extent_item` compression codes.
const COMPRESS_NONE: u8 = 0;
const COMPRESS_ZLIB: u8 = 1;
const COMPRESS_LZO: u8 = 2;
const COMPRESS_ZSTD: u8 = 3;
const LZO_LEN_SIZE: usize = 4;
const LZO_MAX_SECTOR_SIZE: usize = 64 * 1024;

/// Hard cap on tree depth so a malformed image cannot send the walker into
/// unbounded recursion. Real Btrfs trees are typically <= 6 levels even on
/// terabyte volumes; 16 is generous.
const MAX_TREE_DEPTH: usize = 16;

/// How often [`BtrfsVolume::scan_subvol_tree`] invokes its heartbeat
/// callback, counted over ALL visited tree items (not just emitted records).
const SCAN_HEARTBEAT_EVERY: u64 = 4_096;

/// Bytes required for a `btrfs_file_extent_item` regular/prealloc payload.
/// Inline payloads only need the 21-byte fixed header followed by the
/// (compressed) data, so this constant is only checked for the regular path.
const FILE_EXTENT_REGULAR_FIXED: usize = 21 + 32;

/// Maximum bytes a single inline extent may store. The btrfs kernel caps
/// inline data at one node minus headers (default 16 KiB - tree overhead).
/// We use this only as a sanity bound on `ram_bytes` to avoid pathological
/// allocations.
const INLINE_MAX_BYTES: u64 = 1 << 20;

/// Selector for how the volume was located in the parent disk image.
/// Mirrors `Ext4Source` so future extensions (Btrfs inside LUKS, inside a
/// partitioned slice, …) follow the same shape.
#[derive(Debug, Clone, Copy)]
pub enum BtrfsSource {
    Partition(u8),
    Lvm { part_id: u8, lv_index: usize },
}

pub fn parse_btrfs_id(id: &str) -> Option<(BtrfsSource, u64, u64)> {
    // Formats:
    //   btrfs:part:<part_id>:<subvol_id>:<inode>
    //   btrfs:lvm:<part_id>:<lv_index>:<subvol_id>:<inode>
    let mut parts = id.split(':');
    match parts.next()? {
        "btrfs" => {}
        _ => return None,
    }
    match parts.next()? {
        "part" => {
            let part_id = parts.next()?.parse().ok()?;
            let subvol = parts.next()?.parse().ok()?;
            let inode = parts.next()?.parse().ok()?;
            if parts.next().is_some() {
                return None;
            }
            Some((BtrfsSource::Partition(part_id), subvol, inode))
        }
        "lvm" => {
            let part_id = parts.next()?.parse().ok()?;
            let lv_index = parts.next()?.parse().ok()?;
            let subvol = parts.next()?.parse().ok()?;
            let inode = parts.next()?.parse().ok()?;
            if parts.next().is_some() {
                return None;
            }
            Some((BtrfsSource::Lvm { part_id, lv_index }, subvol, inode))
        }
        _ => None,
    }
}

pub fn open_btrfs_source(
    image: &DiskImage,
    source: BtrfsSource,
) -> anyhow::Result<BtrfsVolume<'_>> {
    match source {
        BtrfsSource::Partition(part_id) => {
            let partition = find_partition(image, &part_id.to_string())?;
            BtrfsVolume::open(
                image,
                source,
                partition.start_lba * image.sector_size as u64,
                partition.length_lba * image.sector_size as u64,
                format!("part:{part_id}"),
            )
        }
        BtrfsSource::Lvm { part_id, lv_index } => {
            let partition = find_partition(image, &part_id.to_string())?;
            let lvm = crate::core::filesystem::volumes::lvm::LvmVolume::open(image, partition)?;
            let lv = lvm.logical_volumes.get(lv_index).ok_or_else(|| {
                anyhow::anyhow!("logical volume not found: lvm:{part_id}:{lv_index}")
            })?;
            BtrfsVolume::open(
                image,
                source,
                lv.absolute_offset,
                lv.size_bytes,
                format!("lvm:{part_id}:{lv_index}"),
            )
        }
    }
}

#[derive(Debug)]
pub struct BtrfsVolume<'a> {
    image: &'a DiskImage,
    layout: Option<crate::core::evidence::partitions::lvm2::layout::LinearLayout>,
    source: BtrfsSource,
    base_offset: u64,
    length_bytes: u64,
    nodesize: u32,
    sectorsize: u32,
    label: Option<String>,
    chunks: Vec<ChunkMapping>,
    /// All subvolumes discovered while walking the root tree, keyed by the
    /// subvolume's objectid. Each entry records the `bytenr`/`level` for that
    /// subvolume's FS-tree root and an optional name resolved from
    /// `ROOT_REF` items.
    subvolumes: BTreeMap<u64, SubvolEntry>,
    /// Subvolume to expose at the volume's root. `BTRFS_FS_TREE_OBJECTID`
    /// (= 5) by default; many distro images embed the actual user files in
    /// a child subvolume (e.g. `@rootfs`) referenced from the default tree.
    default_subvol: u64,
    root_dir_inode: u64,
    root_parent_id: String,
}

#[derive(Debug, Clone)]
struct SubvolEntry {
    bytenr: u64,
    level: u8,
    name: Option<String>,
    /// 引用本子卷的父子卷（ROOT_REF 的 objectid 侧）。空 = 没有任何树挂载它
    /// （真正的孤立快照根），应提升到分区根列出。
    parent_subvols: Vec<u64>,
}

#[derive(Debug, Clone, Copy)]
struct ChunkMapping {
    logical: u64,
    length: u64,
    physical: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct BtrfsInode {
    pub objectid: u64,
    pub mode: u32,
    pub size: u64,
    pub atime: i64,
    pub ctime: i64,
    pub mtime: i64,
    pub otime: i64,
    pub flags: u64,
}

impl BtrfsInode {
    pub fn kind(self) -> NodeKind {
        match self.mode & 0xF000 {
            0x4000 => NodeKind::Dir,
            0x8000 => NodeKind::File,
            0xA000 => NodeKind::Symlink,
            _ => NodeKind::Unallocated,
        }
    }
}

impl<'a> BtrfsVolume<'a> {
    pub fn root_inode(&self) -> u64 {
        self.root_dir_inode
    }

    pub fn default_subvol(&self) -> u64 {
        self.default_subvol
    }

    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    pub fn base_offset(&self) -> u64 {
        self.base_offset
    }

    pub fn length_bytes(&self) -> u64 {
        self.length_bytes
    }

    pub fn sector_size_fs(&self) -> u32 {
        self.sectorsize
    }
}

fn should_list_subvol_at_partition_root(
    subvol: u64,
    default_subvol: u64,
    entry: &SubvolEntry,
    subvolumes: &BTreeMap<u64, SubvolEntry>,
) -> bool {
    if subvol == default_subvol {
        return false;
    }
    // 只要有一条 ROOT_REF 指向「仍然存在的」父子卷（含默认树 5），它就会以
    // 挂载点目录的形式出现在那棵树里，不必再提升到分区根——否则同一个节点
    // id 会在两处出现。悬空引用（父子卷的 ROOT_ITEM 已消失）不算可达。
    !entry
        .parent_subvols
        .iter()
        .any(|parent| *parent == FS_TREE_OBJECTID || subvolumes.contains_key(parent))
}

fn root_subvolume_name(subvol: u64, entry: &SubvolEntry) -> String {
    match entry.name.as_deref().filter(|name| !name.trim().is_empty()) {
        Some(name) => {
            crate::core::common::navigation_name::volume_name(&format!("Subvol_{subvol}_{name}"))
        }
        None => format!("Subvol_{subvol}"),
    }
}

pub(crate) fn sort_dirs_first_then_name(nodes: &mut [FsNode]) {
    nodes.sort_by(|a, b| {
        let ak = if a.kind == NodeKind::Dir { 0 } else { 1 };
        let bk = if b.kind == NodeKind::Dir { 0 } else { 1 };
        ak.cmp(&bk)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

#[derive(Debug, Clone, Copy)]
struct ChunkMappingResult {
    physical: u64,
    length: u64,
}

/// One parsed `DIR_INDEX` payload: a name inside its parent directory plus
/// the location key it points at. `pub(crate)` so the snapshot fast path can
/// classify entries with the exact same rules as [`BtrfsVolume::children`].
#[derive(Debug, Clone)]
pub(crate) struct DirEntry {
    pub(crate) name: String,
    location_objectid: u64,
    location_key_type: u8,
    pub(crate) ftype: u8,
}

/// What a directory entry's location key resolves to.
#[derive(Debug, Clone, Copy)]
pub(crate) enum DirEntryTarget {
    /// A plain inode within the same subvolume.
    Inode(u64),
    /// The root directory of another subvolume mounted at this name.
    Subvol(u64),
    /// A location key type the walker does not support (skipped, logged).
    Unsupported(u8),
}

impl DirEntry {
    pub(crate) fn target(&self) -> DirEntryTarget {
        match self.location_key_type {
            KEY_INODE_ITEM => DirEntryTarget::Inode(self.location_objectid),
            KEY_ROOT_ITEM => DirEntryTarget::Subvol(self.location_objectid),
            other => DirEntryTarget::Unsupported(other),
        }
    }
}

/// One record surfaced by [`BtrfsVolume::scan_subvol_tree`] — the subset of
/// FS-tree items the snapshot fast path consumes.
#[derive(Debug, Clone)]
pub(crate) enum BtrfsTreeRecord {
    /// An `INODE_ITEM` record. `item` is `None` when the payload failed to
    /// parse — surfaced so consumers can replicate `read_inode`'s
    /// first-record-wins behaviour (a malformed first record hides the
    /// inode even if a later duplicate key parses).
    Inode {
        objectid: u64,
        item: Option<BtrfsInode>,
    },
    /// A `DIR_INDEX` entry: one name inside directory `parent`.
    DirIndex { parent: u64, entry: DirEntry },
}

#[derive(Debug, Clone)]
struct FileExtent {
    file_offset: u64,
    kind: ExtentKind,
}

#[derive(Debug, Clone)]
enum ExtentKind {
    Inline {
        ram_bytes: u64,
        compression: u8,
        data: Vec<u8>,
    },
    Regular {
        ext_type: u8,
        disk_bytenr: u64,
        disk_num_bytes: u64,
        offset: u64,
        num_bytes: u64,
        ram_bytes: u64,
        compression: u8,
    },
}

#[cfg(test)]
mod tests;
