//! Read-only ZFS volume adapter built on the vendored `zfs-rs` crate.
//!
//! A volume holds the verified topology and routes each DVA to its member
//! partition or group. Traversal borrows this immutable drive.
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

use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use chrono::{DateTime, Utc};
use zfs_rs::dmu::OsType;
use zfs_rs::pool::Pool;
use zfs_rs::zap::{ToByteSlice, ZapString};
use zfs_rs::zpl::{DirEntryType, SAValue};
use zfs_rs::{
    ZfsDirEntry, ZfsDrive, ZfsDslDir, ZfsMetaObjectSet, ZfsZPLDir, ZfsZPLFile, ZfsZPLObjectSet,
};

use crate::core::evidence::image::find_partition;
use crate::core::evidence::image::{DiskImage, Partition};
use crate::core::evidence::partitions::zfs::PartitionSource;
use crate::core::filesystem::tree::fs_tree::{
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
    partition_id: u8,
    drive: ZfsDrive<PoolDrive>,
}

type PoolDrive = Pool<PartitionSource>;

impl ZfsVolume {
    pub fn open(image: Arc<DiskImage>, partition: &Partition) -> Result<Self> {
        let source = PartitionSource::new(&image, partition)?;
        let label = source.label()?;
        let pool_guid = label.identity.pool_guid;
        let mut members = vec![(source, label)];
        for candidate in image
            .partitions
            .iter()
            .filter(|part| part.id != partition.id && part.kind == partition.kind)
        {
            let Ok(source) = PartitionSource::new(&image, candidate) else {
                continue;
            };
            let Ok(label) = source.label() else { continue };
            if label.identity.pool_guid == pool_guid {
                members.push((source, label));
            }
        }
        let (pool, label) = Pool::from_members(members)?;
        Ok(Self {
            partition_id: partition.id,
            drive: ZfsDrive::new_with_label(pool, label),
        })
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
