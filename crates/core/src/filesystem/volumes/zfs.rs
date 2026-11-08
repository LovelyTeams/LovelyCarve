//! Read-only ZFS volume adapter built on the vendored `zfs-rs` crate.
//!
//! Mirrors the `core::ufs_volume` / `core::squashfs_volume` shape: each
//! backend operation instantiates a fresh `ZfsDrive` over a transient
//! `PartitionDrive` against the partition slice, so the dispatcher can fan
//! out concurrent listings / reads without per-volume locking. Re-parsing
//! the disk label and walking the MOS each call costs roughly 1–5 ms on a
//! warm image; if profiling later shows this as a hotspot we can switch to
//! a lazy `Arc<Mutex<…>>` cache like `image::ntfs_deleted`.
//!
//! Two node-ID flavours are emitted, mirroring the two layers of the ZFS
//! object model:
//!
//! - `zfs:part:{N}:dsl:{hex(/dataset/path)}` — a DSL directory. Empty hex =
//!   pool root. Listing children unions DSL sub-datasets (further `dsl:`
//!   nodes) and, when the directory's head dataset is a ZPL filesystem, the
//!   ZPL root entries (`zpl:` nodes).
//! - `zfs:part:{N}:zpl:{hex(/dataset)}:{hex(/file/path)}` — a path inside a
//!   ZPL filesystem. Empty file-hex denotes the filesystem root and is only
//!   ever produced as a placeholder in tests; the dispatcher routes
//!   filesystem-root listings through the `dsl:` form so the surrounding DSL
//!   container is preserved in the tree.
//!
//! Lifetime knot: unlike `rufs::Ufs` (a single owned struct off which the
//! whole API hangs), `zfs-rs` threads borrows through
//! `ZfsDrive → ZfsMetaObjectSet → ZfsDslDir → ZfsDslDataset →
//! ZfsZPLObjectSet → ZfsZPLDir`. We therefore do not factor the chain into a
//! `with_zpl<F>` closure (HRTB on the `Z` parameter is awkward); each public
//! method materialises the chain inline and produces owned `Vec<FsNode>` /
//! `Vec<u8>` before any borrow goes out of scope.

mod directory;
use directory::*;
mod read;
use read::*;

use std::io;
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use chrono::{DateTime, Utc};
use zfs_rs::dmu::OsType;
use zfs_rs::zap::{ToByteSlice, ZapString};
use zfs_rs::zpl::{DirEntryType, SAValue};
use zfs_rs::{
    DefaultDMU, DefaultDSL, DefaultSPA, DefaultZAP, DefaultZPL, RawDevice, ZfsDirEntry, ZfsDrive,
    ZfsDslDir, ZfsMetaObjectSet, ZfsZPLDir, ZfsZPLFile, ZfsZPLObjectSet,
};

use crate::evidence::image::find_partition;
use crate::evidence::image::{DiskImage, Partition};
use crate::filesystem::tree::fs_tree::{
    detect_mime_bytes, shannon_entropy, FsDataRun, FsNode, FsStat, Mactb, NodeKind,
};

/// Streaming chunk floor; `stream_file` will use at least this many bytes
/// per pull from the ZFS reader to amortise LZ4 decompression cost over
/// fewer round-trips.
const STREAM_FLOOR: usize = 64 * 1024;
/// Bytes sampled when computing MIME / entropy / magic hex preview for
/// `file_stat`. Matches the sample size used by other volume adapters.
const SAMPLE_BYTES: usize = 4096;
/// Cap on entries surfaced for a single DSL or ZPL directory listing. A
/// well-populated FreeBSD installer image stays well below this; the cap
/// exists to bound the worst case when something goes wrong.
const MAX_CHILDREN_PER_DIR: usize = 50_000;

#[derive(Debug, Clone, Copy)]
pub enum ZfsSource {
    Partition(u8),
}

#[derive(Debug, Clone)]
pub enum ZfsPath {
    /// A DSL directory inside the pool. `dataset_path` is `/`-joined UTF-8;
    /// empty means the pool root (children of the MOS `root_dataset` ZAP).
    Dsl { dataset_path: String },
    /// A path inside a ZPL filesystem. `dataset_path` is the DSL path of the
    /// owning filesystem dataset; `file_path` is the in-filesystem absolute
    /// path (with leading `/`) or empty for the filesystem root.
    Zpl {
        dataset_path: String,
        file_path: String,
    },
}

/// Parse `zfs:part:{N}:dsl:{hex}` or `zfs:part:{N}:zpl:{hex}:{hex}`.
pub fn parse_zfs_id(id: &str) -> Option<(ZfsSource, ZfsPath)> {
    let mut parts = id.splitn(6, ':');
    let p0 = parts.next()?;
    let p1 = parts.next()?;
    let part_id = parts.next()?.parse::<u8>().ok()?;
    let kind = parts.next()?;
    let hex_a = parts.next()?;
    if p0 != "zfs" || p1 != "part" {
        return None;
    }
    let path_a = decode_hex_path(hex_a)?;
    match kind {
        "dsl" => {
            if parts.next().is_some() {
                return None;
            }
            Some((
                ZfsSource::Partition(part_id),
                ZfsPath::Dsl {
                    dataset_path: path_a,
                },
            ))
        }
        "zpl" => {
            let hex_b = parts.next()?;
            if parts.next().is_some() {
                return None;
            }
            let path_b = decode_hex_path(hex_b)?;
            Some((
                ZfsSource::Partition(part_id),
                ZfsPath::Zpl {
                    dataset_path: path_a,
                    file_path: path_b,
                },
            ))
        }
        _ => None,
    }
}

fn decode_hex_path(hex_path: &str) -> Option<String> {
    if hex_path.is_empty() {
        return Some(String::new());
    }
    let bytes = hex::decode(hex_path).ok()?;
    String::from_utf8(bytes).ok()
}

pub fn open_zfs_source(image: &Arc<DiskImage>, source: ZfsSource) -> Result<ZfsVolume> {
    match source {
        ZfsSource::Partition(part_id) => {
            let partition = find_partition(image, &part_id.to_string())?;
            ZfsVolume::open(Arc::clone(image), partition)
        }
    }
}

pub struct ZfsVolume {
    image: Arc<DiskImage>,
    partition_id: u8,
    base_offset: u64,
    length_bytes: u64,
}

impl ZfsVolume {
    pub fn open(image: Arc<DiskImage>, partition: &Partition) -> Result<Self> {
        let sector_size = image.sector_size as u64;
        let volume = Self {
            image,
            partition_id: partition.id,
            base_offset: partition.start_lba * sector_size,
            length_bytes: partition.length_lba * sector_size,
        };
        // Eager-validate the label so the caller sees a parse error here
        // rather than only when listing children.
        volume.open_drive()?;
        Ok(volume)
    }

    /// Children of the partition root (the pool root). Lists the top-level
    /// DSL dirs (`$MOS`, `$FREE`, `$ORIGIN`, `ROOT`, `home`, …). Every entry
    /// is shown as a directory; whether it has ZPL content is determined
    /// only when the user expands it.
    pub fn partition_children(&self) -> Result<Vec<FsNode>> {
        self.dsl_children("")
    }

    fn dsl_id(&self, dataset_path: &str) -> String {
        if dataset_path.is_empty() {
            // Pool root maps to the partition node itself; the dispatcher
            // never actually serialises this string because it short-
            // circuits to `partition_children` via the `part:N` arm.
            format!("part:{}", self.partition_id)
        } else {
            format!(
                "zfs:part:{}:dsl:{}",
                self.partition_id,
                hex::encode(dataset_path.as_bytes())
            )
        }
    }

    fn zpl_id(&self, dataset_path: &str, file_path: &str) -> String {
        format!(
            "zfs:part:{}:zpl:{}:{}",
            self.partition_id,
            hex::encode(dataset_path.as_bytes()),
            hex::encode(file_path.as_bytes())
        )
    }

    /// Construct a fresh `ZfsDrive` over the partition slice. Each public
    /// method opens its own drive — the cost is dominated by parsing the
    /// 256 KiB label, which is cached in the OS page cache after the first
    /// call.
    fn open_drive(&self) -> Result<ZfsDrive<PartitionDrive>> {
        let cursor = PartitionDrive {
            image: Arc::clone(&self.image),
            base_offset: self.base_offset,
            length_bytes: self.length_bytes,
        };
        ZfsDrive::new(cursor)
            .map_err(|err| anyhow!("ZfsDrive::new part:{}: {err}", self.partition_id))
    }
}

/// Decode a ZapString as a (lossy) UTF-8 owned String for display.
fn zap_string_to_string(zap: &ZapString) -> String {
    String::from_utf8_lossy(zap.as_byte_slice()).into_owned()
}

/// Split a `/`-rooted path into `(parent_path, leaf_name)`. `parent_path`
/// preserves the leading `/` (or is empty when the leaf sits at the root).
/// Used to look up a file by walking to its parent dir then issuing a
/// `get_child`.
fn split_path(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(idx) => (&path[..idx], &path[idx + 1..]),
        None => ("", path),
    }
}

/// Adapter that exposes a partition slice of `DiskImage` as a `RawDevice`
/// for `zfs-rs`. ZFS labels live at byte offset 0 and 0x40000 of the vdev
/// (which is the partition for a partitioned ZFS pool); the upstream
/// `DefaultDrive::read_offset` adds an additional 4 MiB boot-block offset
/// for the active filesystem area, and we leave that addition to the
/// upstream `Device` blanket impl rather than baking it into our base
/// offset.
pub struct PartitionDrive {
    image: Arc<DiskImage>,
    base_offset: u64,
    length_bytes: u64,
}

impl RawDevice for PartitionDrive {
    type Block = Vec<u8>;
    fn read_raw(&self, addr: u64, size: u64) -> io::Result<Self::Block> {
        if size == 0 {
            return Ok(Vec::new());
        }
        // Range-check against the partition bounds — anything past the end
        // is a parse error in the upstream code, so we surface it as
        // `UnexpectedEof` for a clearer message.
        let end = addr
            .checked_add(size)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "ZFS read overflow"))?;
        if end > self.length_bytes {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!(
                    "ZFS read past partition end: addr={addr} size={size} len={}",
                    self.length_bytes
                ),
            ));
        }
        self.image
            .read_range(self.base_offset + addr, size as usize)
            .map_err(|err| io::Error::other(err.to_string()))
    }
}

impl DefaultSPA for PartitionDrive {}
impl DefaultDMU for PartitionDrive {}
impl DefaultZAP for PartitionDrive {}
impl DefaultDSL for PartitionDrive {}
impl DefaultZPL for PartitionDrive {}

// `DirEntryType` is currently only used for documentation of the upstream
// enum shape; we route through `ZfsDirEntry` which already carves
// directories vs. files. Re-exported to avoid an unused-import warning if
// later helpers want to inspect symlink/socket/etc. variants.
#[allow(dead_code)]
fn _direntry_type_marker(t: DirEntryType) -> DirEntryType {
    t
}

#[cfg(test)]
mod tests;
