//! Read-only HFS+ / HFSX volume adapter built on the vendored `hfsplus`
//! crate.
//!
//! Each backend operation instantiates a fresh `hfsplus::HfsVolume` over a
//! transient cursor against the partition slice (mirrors `core::filesystem::volumes::apfs`)
//! — there is no shared mutable state, so the dispatcher can fan out
//! concurrent listings / reads without per-volume locking.
//!
//! Node IDs encode the absolute UTF-8 path inside the volume as hex bytes:
//! `hfsplus:part:{N}:{hex(/path/to/entry)}`. The empty-hex form denotes the
//! volume root and is never emitted directly — the partition node itself
//! stands in for the root.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use chrono::{TimeZone, Utc};
use hfsplus::{EntryKind, HfsVolume};

use crate::evidence::image::find_partition;
use crate::evidence::image::{DiskImage, Partition};
use crate::filesystem::tree::fs_tree::{
    detect_mime_bytes, shannon_entropy, FsDataRun, FsNode, FsStat, Mactb, NodeKind,
};

const SAMPLE_BYTES: usize = 4096;
const STREAM_FLOOR: usize = 64 * 1024;

/// Seconds from 1904-01-01 UTC to the Unix epoch. HFS+ timestamps are UTC
/// seconds (TN1150); traditional HFS local-time fields are unsupported.
const HFS_EPOCH_OFFSET_SECS: i64 = 2_082_844_800;

#[derive(Debug, Clone, Copy)]
pub enum HfsPlusSource {
    Partition(u8),
}

/// Parse an `hfsplus:part:{N}:{hex(path)}` ID. Returns the source plus the
/// decoded UTF-8 path (with a leading `/`, or empty for the volume root).
pub fn parse_hfsplus_id(id: &str) -> Option<(HfsPlusSource, String)> {
    let mut parts = id.splitn(4, ':');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("hfsplus"), Some("part"), Some(part_id), Some(hex_path)) => {
            let part_id = part_id.parse().ok()?;
            let path = if hex_path.is_empty() {
                String::new()
            } else {
                let bytes = hex::decode(hex_path).ok()?;
                String::from_utf8(bytes).ok()?
            };
            Some((HfsPlusSource::Partition(part_id), path))
        }
        _ => None,
    }
}

pub fn open_hfsplus_source(image: &Arc<DiskImage>, source: HfsPlusSource) -> Result<HfsPlusVolume> {
    match source {
        HfsPlusSource::Partition(part_id) => {
            let partition = find_partition(image, &part_id.to_string())?;
            HfsPlusVolume::open(Arc::clone(image), partition)
        }
    }
}

pub struct HfsPlusVolume {
    image: Arc<DiskImage>,
    partition_id: u8,
    base_offset: u64,
    length_bytes: u64,
}

impl HfsPlusVolume {
    pub fn open(image: Arc<DiskImage>, partition: &Partition) -> Result<Self> {
        let sector_size = image.sector_size as u64;
        let base_offset = partition
            .start_lba
            .checked_mul(sector_size)
            .ok_or_else(|| anyhow!("HFS+ partition offset overflow"))?;
        let length_bytes = partition
            .length_lba
            .checked_mul(sector_size)
            .ok_or_else(|| anyhow!("HFS+ partition length overflow"))?;
        anyhow::ensure!(
            base_offset
                .checked_add(length_bytes)
                .is_some_and(|end| end <= image.len()),
            "HFS+ partition exceeds evidence"
        );
        let volume = Self {
            image,
            partition_id: partition.id,
            base_offset,
            length_bytes,
        };
        // Eager-validate the volume header so the caller sees parse errors
        // at open time instead of only when listing the root.
        volume.with_volume(|_| Ok(()))?;
        Ok(volume)
    }

    pub fn root_children(&self) -> Result<Vec<FsNode>> {
        self.children("")
    }

    /// Unallocated bytes in this volume, read straight from the HFS+ volume
    /// header's `free_blocks × block_size`. Exact and cheap — the header is
    /// already parsed on open.
    pub fn free_bytes(&self) -> Result<u64> {
        self.with_volume(|vol| {
            let header = vol.volume_header();
            Ok(header.free_blocks as u64 * header.block_size as u64)
        })
    }

    /// Lists the children of a path inside the volume. The path uses Unix-
    /// style separators with a leading `/` (or the empty string for the
    /// volume root).
    pub fn children(&self, path: &str) -> Result<Vec<FsNode>> {
        let parent_id = self.node_id(path);
        let parent_path = path.to_string();
        let probe_path = if path.is_empty() {
            "/".to_string()
        } else {
            path.to_string()
        };
        self.with_volume(move |vol| {
            let entries = vol
                .list_directory(&probe_path)
                .map_err(|err| anyhow!("HFS+ list_directory {probe_path}: {err}"))?;

            let mut out = Vec::new();
            for entry in entries {
                let child_path = join_path(&parent_path, &entry.name);
                let kind = node_kind_from(entry.kind);
                let size = if kind == NodeKind::Dir { 0 } else { entry.size };
                let timestamps = hfsplus_timestamps(
                    entry.create_date,
                    entry.modify_date,
                    entry.attribute_mod_date,
                    entry.access_date,
                );
                out.push(FsNode {
                    id: self.node_id(&child_path),
                    name: entry.name,
                    parent_id: Some(parent_id.clone()),
                    kind,
                    size,
                    deleted: false,
                    inode: Some(entry.cnid as u64),
                    timestamps,
                    link_target: if kind == NodeKind::Symlink {
                        Some(
                            String::from_utf8(vol.read_file(&child_path)?)
                                .map_err(|error| anyhow!("HFS+ symlink {child_path}: {error}"))?,
                        )
                    } else {
                        None
                    },
                    has_snapshot: false,
                });
            }
            out.sort_by(|a, b| {
                let ak = u8::from(a.kind != NodeKind::Dir);
                let bk = u8::from(b.kind != NodeKind::Dir);
                ak.cmp(&bk)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
            Ok(out)
        })
    }

    pub fn read_file(&self, path: &str, max_bytes: usize) -> Result<Vec<u8>> {
        if path.is_empty() {
            anyhow::bail!("cannot read HFS+ volume root as a file");
        }
        let owned = path.to_string();
        self.with_volume(move |vol| {
            let mut file = vol
                .open_file(&owned)
                .map_err(|err| anyhow!("HFS+ open_file {owned}: {err}"))?;
            let mut out = Vec::new();
            let mut buf = vec![0u8; STREAM_FLOOR];
            while out.len() < max_bytes {
                let want = (max_bytes - out.len()).min(buf.len());
                let read = std::io::Read::read(&mut file, &mut buf[..want])
                    .map_err(|err| anyhow!("HFS+ read {owned}: {err}"))?;
                if read == 0 {
                    break;
                }
                out.extend_from_slice(&buf[..read]);
            }
            Ok(out)
        })
    }

    pub fn read_file_range(&self, path: &str, offset: u64, length: usize) -> Result<Vec<u8>> {
        self.with_volume(|volume| {
            let mut file = volume.open_file(path)?;
            file.seek(SeekFrom::Start(offset))?;
            let mut output = Vec::new();
            file.take(length as u64).read_to_end(&mut output)?;
            Ok(output)
        })
    }

    pub fn stream_file<F>(&self, path: &str, chunk_size: usize, mut visitor: F) -> Result<u64>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        if path.is_empty() {
            anyhow::bail!("cannot stream HFS+ volume root as a file");
        }
        let owned = path.to_string();
        self.with_volume(move |vol| {
            let mut file = vol
                .open_file(&owned)
                .map_err(|err| anyhow!("HFS+ open_file {owned}: {err}"))?;
            let mut buf = vec![0u8; chunk_size.max(STREAM_FLOOR)];
            let mut total = 0u64;
            loop {
                let read = std::io::Read::read(&mut file, &mut buf)
                    .map_err(|err| anyhow!("HFS+ stream {owned}: {err}"))?;
                if read == 0 {
                    break;
                }
                visitor(&buf[..read])?;
                total += read as u64;
            }
            Ok(total)
        })
    }

    pub fn file_stat(&self, path: &str, node_id: &str) -> Result<FsStat> {
        if path.is_empty() {
            anyhow::bail!("cannot stat HFS+ volume root as a file");
        }
        let owned = path.to_string();
        let node_id = node_id.to_string();
        self.with_volume(move |vol| {
            let stat = vol
                .stat(&owned)
                .map_err(|err| anyhow!("HFS+ stat {owned}: {err}"))?;

            let compressed = stat.compression.is_some();

            let sample = if matches!(stat.kind, EntryKind::File) && stat.size > 0 {
                let mut file = vol
                    .open_file(&owned)
                    .map_err(|err| anyhow!("HFS+ open_file {owned}: {err}"))?;
                let want = SAMPLE_BYTES.min(stat.size as usize);
                let mut buf = vec![0u8; want];
                let mut filled = 0usize;
                while filled < buf.len() {
                    let read = std::io::Read::read(&mut file, &mut buf[filled..])
                        .map_err(|err| anyhow!("HFS+ stat read {owned}: {err}"))?;
                    if read == 0 {
                        break;
                    }
                    filled += read;
                }
                buf.truncate(filled);
                buf
            } else {
                Vec::new()
            };

            let mime = detect_mime_bytes(&sample).to_string();
            let data_runs = if compressed
                || matches!(
                    self.image.derived,
                    Some(crate::evidence::image::DerivedImage::Decrypted { .. })
                ) {
                Vec::new()
            } else {
                let block_size = u64::from(vol.volume_header().block_size);
                let mut logical_offset = 0u64;
                vol.data_extents(&owned)?
                    .into_iter()
                    .filter_map(|extent| {
                        let length = (u64::from(extent.block_count) * block_size)
                            .min(stat.size.saturating_sub(logical_offset));
                        let run = FsDataRun {
                            logical_offset,
                            physical_offset: self.base_offset
                                + u64::from(extent.start_block) * block_size,
                            length,
                        };
                        logical_offset += length;
                        (length > 0).then_some(run)
                    })
                    .collect()
            };

            Ok(FsStat {
                node_id,
                size: stat.size,
                inode: Some(stat.cnid as u64),
                mode: Some(stat.permissions.mode),
                mime,
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
                    Some(shannon_entropy(&sample))
                },
                data_runs,
            })
        })
    }

    fn node_id(&self, path: &str) -> String {
        if path.is_empty() {
            format!("part:{}", self.partition_id)
        } else {
            format!(
                "hfsplus:part:{}:{}",
                self.partition_id,
                hex::encode(path.as_bytes())
            )
        }
    }

    /// Constructs a fresh `hfsplus::HfsVolume` over the partition slice and
    /// hands it to the closure. The volume is dropped immediately after the
    /// closure returns; the cursor is read-only by construction so the
    /// underlying image is never mutated.
    fn with_volume<R>(
        &self,
        op: impl FnOnce(&mut HfsVolume<PartitionCursor>) -> Result<R>,
    ) -> Result<R> {
        let cursor = PartitionCursor {
            image: Arc::clone(&self.image),
            base_offset: self.base_offset,
            length_bytes: self.length_bytes,
            position: 0,
        };
        let mut vol = HfsVolume::open(cursor)
            .map_err(|err| anyhow!("HFS+ open partition {}: {err}", self.partition_id))?;
        op(&mut vol)
    }
}

fn node_kind_from(kind: EntryKind) -> NodeKind {
    match kind {
        EntryKind::Directory => NodeKind::Dir,
        EntryKind::Symlink => NodeKind::Symlink,
        EntryKind::File => NodeKind::File,
    }
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() || parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

fn hfsplus_timestamps(
    create_date: u32,
    modify_date: u32,
    attribute_mod_date: u32,
    access_date: u32,
) -> Mactb {
    let created = hfs_time_to_rfc3339(create_date);
    Mactb {
        modified: hfs_time_to_rfc3339(modify_date),
        accessed: hfs_time_to_rfc3339(access_date),
        created: created.clone(),
        mft_changed: hfs_time_to_rfc3339(attribute_mod_date),
        birth: created,
    }
}

/// HFS+ stores 32-bit timestamps as seconds since 1904-01-01. Unset entries
/// are zero — pass those through as `None` so the UI doesn't render a 1904
/// date.
fn hfs_time_to_rfc3339(seconds: u32) -> Option<String> {
    if seconds == 0 {
        return None;
    }
    let unix_secs = seconds as i64 - HFS_EPOCH_OFFSET_SECS;
    Utc.timestamp_opt(unix_secs, 0)
        .single()
        .map(|dt| dt.to_rfc3339())
}

/// Cursor exposing a partition slice of `DiskImage` as `std::io::Read +
/// Seek`. Read-only — HFS+ parsing never writes back, so no `Write` impl is
/// needed (mirrors `core::filesystem::volumes::apfs::PartitionCursor`).
struct PartitionCursor {
    image: Arc<DiskImage>,
    base_offset: u64,
    length_bytes: u64,
    position: u64,
}

impl Read for PartitionCursor {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.position >= self.length_bytes || buf.is_empty() {
            return Ok(0);
        }
        let remaining = self.length_bytes - self.position;
        let want = (buf.len() as u64).min(remaining) as usize;
        let bytes = self
            .image
            .read_range(self.base_offset + self.position, want)
            .map_err(|err| io::Error::other(err.to_string()))?;
        let copied = bytes.len();
        buf[..copied].copy_from_slice(&bytes);
        self.position += copied as u64;
        Ok(copied)
    }
}

impl Seek for PartitionCursor {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(offset) => offset as i128,
            SeekFrom::Current(delta) => self.position as i128 + delta as i128,
            SeekFrom::End(delta) => self.length_bytes as i128 + delta as i128,
        };
        if target < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "HFS+ cursor seek before start",
            ));
        }
        self.position = target as u64;
        Ok(self.position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hfs_timestamp(unix_seconds: i64) -> u32 {
        (unix_seconds + HFS_EPOCH_OFFSET_SECS) as u32
    }

    #[test]
    fn hfsplus_timestamps_include_access_and_attribute_change_times() {
        let timestamps = hfsplus_timestamps(
            hfs_timestamp(1_700_000_000),
            hfs_timestamp(1_700_000_001),
            hfs_timestamp(1_700_000_002),
            hfs_timestamp(1_700_000_003),
        );

        assert_eq!(
            timestamps.created.as_deref(),
            Some("2023-11-14T22:13:20+00:00")
        );
        assert_eq!(
            timestamps.modified.as_deref(),
            Some("2023-11-14T22:13:21+00:00")
        );
        assert_eq!(
            timestamps.mft_changed.as_deref(),
            Some("2023-11-14T22:13:22+00:00")
        );
        assert_eq!(
            timestamps.accessed.as_deref(),
            Some("2023-11-14T22:13:23+00:00")
        );
        assert_eq!(timestamps.birth, timestamps.created);
    }
}
