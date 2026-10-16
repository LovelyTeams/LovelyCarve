//! Read-only exFAT volume adapter built on the vendored `exfat` crate.
//!
//! The `exfat::Root` is opened once in [`ExfatVolume::open`] — which loads the
//! whole FAT region — and reused for every directory listing via the vendored
//! `Root::items()` borrow plus `Directory::open(&self)`. A whole-volume
//! snapshot walk would otherwise re-open the root (re-reading the entire FAT)
//! per directory; caching the `ExfatVolume` in
//! [`crate::core::filesystem::tree::fs_tree::FsSession`] then keeps it to one FAT load per
//! walk. File *content* reads still open a fresh root (they need an owned,
//! seekable `File` and aren't on the walk hot path). All access is `&self`, so
//! no per-volume lock is needed.
//!
//! Node IDs encode the absolute UTF-8 path inside the volume as hex bytes:
//! `exfat:part:{N}:{hex(/path/to/entry)}`. The empty-hex form denotes the
//! volume root and is never emitted directly — the partition node itself
//! stands in for the root.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use chrono::{Duration, NaiveDate, TimeZone, Utc};

mod read;
pub use read::{ExfatFileMetadata, ExfatVolumeMetadata};
#[cfg(test)]
mod fixture_tests;
use exfat::timestamp::{Timestamp as ExfatTimestamp, Timestamps as ExfatTimestamps};
use exfat::{Item, Root};

use crate::core::evidence::image::find_partition;
use crate::core::evidence::image::{DiskImage, Partition};
use crate::core::filesystem::tree::fs_tree::{FsNode, Mactb, NodeKind};

#[derive(Debug, Clone, Copy)]
pub enum ExfatSource {
    Partition(u8),
}

/// Parse an `exfat:part:{N}:{hex(path)}` ID. Returns the source plus the
/// decoded UTF-8 path (with a leading `/`, or empty for the volume root).
pub fn parse_exfat_id(id: &str) -> Option<(ExfatSource, String)> {
    let mut parts = id.splitn(4, ':');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("exfat"), Some("part"), Some(part_id), Some(hex_path)) => {
            let part_id = part_id.parse().ok()?;
            let path = if hex_path.is_empty() {
                String::new()
            } else {
                let bytes = hex::decode(hex_path).ok()?;
                String::from_utf8(bytes).ok()?
            };
            Some((ExfatSource::Partition(part_id), path))
        }
        _ => None,
    }
}

pub fn open_exfat_source(image: &Arc<DiskImage>, source: ExfatSource) -> Result<ExfatVolume> {
    match source {
        ExfatSource::Partition(part_id) => {
            let partition = find_partition(image, &part_id.to_string())?;
            ExfatVolume::open(Arc::clone(image), partition)
        }
    }
}

pub struct ExfatVolume {
    image: Arc<DiskImage>,
    partition_id: u8,
    base_offset: u64,
    length_bytes: u64,
    boot_sector: [u8; 512],
    /// Root opened once (its FAT loaded once) and reused for every directory
    /// listing — `children` walks this via `Root::items()` instead of re-
    /// opening per call. Held by `&self`; no lock needed.
    root: Root<PartitionPart>,
}

impl ExfatVolume {
    pub fn open(image: Arc<DiskImage>, partition: &Partition) -> Result<Self> {
        let sector_size = image.sector_size as u64;
        let base_offset = partition
            .start_lba
            .checked_mul(sector_size)
            .ok_or_else(|| anyhow!("exFAT partition offset overflow"))?;
        let length_bytes = partition
            .length_lba
            .checked_mul(sector_size)
            .ok_or_else(|| anyhow!("exFAT partition length overflow"))?;
        base_offset
            .checked_add(length_bytes)
            .ok_or_else(|| anyhow!("exFAT partition end overflow"))?;
        let boot = super::exfat_recover::read_effective_boot_sector(&image, base_offset)?;
        if !(9..=12).contains(&boot[108]) {
            anyhow::bail!("exFAT invalid boot sector");
        }
        let declared_sectors = u64::from_le_bytes(boot[72..80].try_into()?);
        if declared_sectors
            .checked_mul(1u64 << boot[108])
            .is_none_or(|size| size > length_bytes)
        {
            anyhow::bail!("exFAT declared volume extends outside partition");
        }
        let part = PartitionPart {
            image: Arc::clone(&image),
            base_offset,
            length_bytes,
            boot_sector: boot,
        };
        let root = Root::open(part).map_err(|err| anyhow!("exFAT open: {err:?}"))?;
        Ok(Self {
            image,
            partition_id: partition.id,
            base_offset,
            length_bytes,
            boot_sector: boot,
            root,
        })
    }

    pub fn root_children(&self) -> Result<Vec<FsNode>> {
        self.children("")
    }

    /// Lists `path`'s children including recovered deleted entry sets (P1-1).
    /// Live entries come from the vendored crate; deleted sets (type byte high
    /// bit cleared) are decoded from the directory's raw clusters via
    /// [`crate::core::filesystem::volumes::exfat_recover`] and merged with `deleted: true`.
    pub fn children_with_deleted(&self, path: &str) -> Result<Vec<FsNode>> {
        match self.children_with_deleted_strict(path) {
            Ok(nodes) => Ok(nodes),
            Err(error) => {
                tracing::debug!("exfat deleted listing failed: {error:#}");
                self.children(path)
            }
        }
    }

    /// Recovery scans must report missing deletion metadata instead of silently
    /// returning only live entries. Ordinary browsing retains its tolerant fallback.
    pub(crate) fn children_with_deleted_strict(&self, path: &str) -> Result<Vec<FsNode>> {
        Ok(self
            .recovery_children(path)?
            .into_iter()
            .map(|(node, _)| node)
            .collect())
    }

    /// Strict recovery listing with the initialized length of each deleted file.
    /// Live entries use their native logical reader and need no override.
    pub(crate) fn recovery_children(&self, path: &str) -> Result<Vec<(FsNode, Option<u64>)>> {
        use crate::core::filesystem::volumes::exfat_recover as recover;
        let mut out = self
            .children(path)?
            .into_iter()
            .map(|node| (node, None))
            .collect::<Vec<_>>();
        let geom = recover::open_geom(&self.image, self.base_offset)?;
        let fat = recover::ExfatFat::load(&self.image, &geom)?;
        let bytes = recover::read_directory_path(&self.image, &geom, &fat, path)?;
        let parent_id = self.node_id(path);
        for entry in recover::scan_dir_deleted(&bytes, &geom, Some(&fat)) {
            let initialized = entry.valid_data_length;
            out.push((
                recovered_node(self.partition_id, &parent_id, entry),
                Some(initialized),
            ));
        }
        Ok(out)
    }

    /// Lists the direct children of `path` inside the volume. `path` uses
    /// Unix-style separators with a leading `/`, or is empty for the volume
    /// root. Served from the held root — no FAT reload per directory.
    pub fn children(&self, path: &str) -> Result<Vec<FsNode>> {
        let segments = path_segments(path);
        if segments.is_empty() {
            self.nodes_from_items(path, self.root.items().iter())
        } else {
            let items = self.subdir_items(&segments)?;
            self.nodes_from_items(path, items.iter())
        }
    }

    /// Builds the sorted child `FsNode`s for `parent_path` from a borrowed
    /// sequence of directory items (the held root's, or an opened subdir's).
    fn nodes_from_items<'i, I>(&self, parent_path: &str, items: I) -> Result<Vec<FsNode>>
    where
        I: Iterator<Item = &'i Item<PartitionPart>>,
    {
        let parent_id = self.node_id(parent_path);
        let mut out = Vec::new();
        for item in items {
            match item {
                Item::Directory(dir) => {
                    let name = dir.name().to_string();
                    if name.is_empty() || name == "." || name == ".." {
                        continue;
                    }
                    let child_path = join_path(parent_path, &name);
                    out.push(FsNode {
                        id: self.node_id(&child_path),
                        name,
                        parent_id: Some(parent_id.clone()),
                        kind: NodeKind::Dir,
                        size: 0,
                        deleted: false,
                        inode: Some(self.base_offset + dir.entry_offset()),
                        timestamps: timestamps_to_mactb(dir.timestamps()),
                        link_target: None,
                        has_snapshot: false,
                    });
                }
                Item::File(file) => {
                    let name = file.name().to_string();
                    if name.is_empty() {
                        continue;
                    }
                    let child_path = join_path(parent_path, &name);
                    out.push(FsNode {
                        id: self.node_id(&child_path),
                        name,
                        parent_id: Some(parent_id.clone()),
                        kind: NodeKind::File,
                        size: file.len(),
                        deleted: false,
                        inode: Some(self.base_offset + file.entry_offset()),
                        timestamps: timestamps_to_mactb(file.timestamps()),
                        link_target: None,
                        has_snapshot: false,
                    });
                }
            }
        }
        out.sort_by(|a, b| {
            let ak = u8::from(a.kind != NodeKind::Dir);
            let bk = u8::from(b.kind != NodeKind::Dir);
            ak.cmp(&bk)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(out)
    }

    /// Items of the directory named by `segments`, walked from the held root.
    /// The first level borrows the held root's items (its FAT already loaded);
    /// each deeper level is the owned `Vec` from `Directory::open` (also
    /// `&self`), so no listing re-reads the FAT.
    fn subdir_items(&self, segments: &[&str]) -> Result<Vec<Item<PartitionPart>>> {
        let (first, rest) = segments
            .split_first()
            .ok_or_else(|| anyhow!("exFAT subdir walk needs at least one segment"))?;
        let dir = self
            .root
            .items()
            .iter()
            .find_map(|item| match item {
                Item::Directory(dir) if dir.name() == *first => Some(dir),
                _ => None,
            })
            .ok_or_else(|| anyhow!("exFAT directory not found: {first}"))?;
        let mut items = dir
            .open()
            .map_err(|err| anyhow!("exFAT open dir {first}: {err}"))?;
        for segment in rest {
            let dir = items
                .into_iter()
                .find_map(|item| match item {
                    Item::Directory(dir) if dir.name() == *segment => Some(dir),
                    _ => None,
                })
                .ok_or_else(|| anyhow!("exFAT directory not found: {segment}"))?;
            items = dir
                .open()
                .map_err(|err| anyhow!("exFAT open dir {segment}: {err}"))?;
        }
        Ok(items)
    }

    fn node_id(&self, path: &str) -> String {
        if path.is_empty() {
            format!("part:{}", self.partition_id)
        } else {
            format!(
                "exfat:part:{}:{}",
                self.partition_id,
                hex::encode(path.as_bytes())
            )
        }
    }

    fn open_root(&self) -> Result<Root<PartitionPart>> {
        let partition = PartitionPart {
            image: Arc::clone(&self.image),
            base_offset: self.base_offset,
            length_bytes: self.length_bytes,
            boot_sector: self.boot_sector,
        };
        Root::open(partition).map_err(|err| anyhow!("exFAT open: {err:?}"))
    }

    /// Resolve `path` (a leading-slash absolute path inside the volume) to an
    /// `exfat::file::File`. Walks the directory chain by name on each call so
    /// the file's lifetime is not coupled to a held-open `Root` (the exfat
    /// crate's directory iterator consumes the parent).
    fn open_file(&self, path: &str) -> Result<exfat::file::File<PartitionPart>> {
        let segments = path_segments(path);
        let (file_name, dir_segments) = segments
            .split_last()
            .ok_or_else(|| anyhow!("exFAT path has no leaf: {path}"))?;
        let file_name = (*file_name).to_string();

        let root = self.open_root()?;
        let items = if dir_segments.is_empty() {
            root.into_iter().collect::<Vec<_>>()
        } else {
            navigate_to_dir(root, dir_segments)?
        };

        for item in items {
            if let Item::File(file) = item {
                if file.name().eq_ignore_ascii_case(&file_name) {
                    return Ok(file);
                }
            }
        }
        Err(anyhow!("exFAT file not found: {path}"))
    }
}

/// Walks `dir_segments` from a freshly-opened root, returning the items of the
/// final directory. Each segment lookup consumes the parent's iterator since
/// `Directory::open` returns an owned `Vec<Item>`.
fn navigate_to_dir(
    root: Root<PartitionPart>,
    dir_segments: &[&str],
) -> Result<Vec<Item<PartitionPart>>> {
    let mut items: Vec<Item<PartitionPart>> = root.into_iter().collect();
    for segment in dir_segments {
        let mut next = None;
        for item in items.into_iter() {
            if let Item::Directory(dir) = item {
                if dir.name() == *segment {
                    next = Some(dir);
                    break;
                }
            }
        }
        let dir = next.ok_or_else(|| anyhow!("exFAT directory not found: {segment}"))?;
        items = dir
            .open()
            .map_err(|err| anyhow!("exFAT open dir {segment}: {err}"))?;
    }
    Ok(items)
}

fn path_segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|seg| !seg.is_empty()).collect()
}

/// Build an `FsNode` for a recovered deleted exFAT entry set. When a single
/// contiguous run is reconstructed it is embedded in a `deleted:run:` id so
/// reads land directly on the image; otherwise a non-readable `deleted:` id.
fn recovered_node(
    partition_id: u8,
    parent_id: &str,
    entry: crate::core::filesystem::volumes::exfat_recover::RecoveredEntry,
) -> FsNode {
    let id = match entry.runs.first() {
        Some(run)
            if !entry.is_directory
                && entry.runs.len() == 1
                && run.length == entry.size
                && run.length > 0 =>
        {
            format!(
                "deleted:run:part:{partition_id}:exfat:{}:{}:0x{:x}:0x{:x}",
                hex::encode(parent_id),
                entry.slot_offset,
                run.physical_offset,
                run.length
            )
        }
        _ => format!(
            "deleted:exfat:part:{partition_id}:{}:{}",
            hex::encode(parent_id),
            entry.slot_offset
        ),
    };
    FsNode {
        id,
        name: entry.name,
        parent_id: Some(parent_id.to_string()),
        kind: if entry.is_directory {
            NodeKind::Dir
        } else {
            NodeKind::File
        },
        size: if entry.is_directory { 0 } else { entry.size },
        deleted: true,
        inode: None,
        timestamps: entry.timestamps,
        link_target: None,
        has_snapshot: false,
    }
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() || parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

fn timestamps_to_mactb(ts: &ExfatTimestamps) -> Mactb {
    Mactb {
        modified: exfat_ts_to_rfc3339(ts.modified()),
        accessed: exfat_ts_to_rfc3339(ts.accessed()),
        created: exfat_ts_to_rfc3339(ts.created()),
        mft_changed: None,
        birth: exfat_ts_to_rfc3339(ts.created()),
    }
}

pub(super) fn exfat_ts_to_rfc3339(ts: &ExfatTimestamp) -> Option<String> {
    if !ts.is_valid() {
        return None;
    }
    let date = ts.date();
    let time = ts.time();
    let naive = NaiveDate::from_ymd_opt(
        i32::from(date.year),
        u32::from(date.month),
        u32::from(date.day),
    )?
    .and_hms_milli_opt(
        u32::from(time.hour),
        u32::from(time.minute),
        u32::from(time.second),
        u32::from(ts.milliseconds()),
    )?;
    match ts.offset() {
        Some(offset) => Some(
            (Utc.from_utc_datetime(&naive) - Duration::minutes(15 * i64::from(offset)))
                .to_rfc3339(),
        ),
        None => Some(naive.format("%Y-%m-%dT%H:%M:%S%.3f").to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{exfat_ts_to_rfc3339, ExfatTimestamp};

    fn encoded_timestamp(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> u32 {
        (u32::from(year - 1980) << 25)
            | (u32::from(month) << 21)
            | (u32::from(day) << 16)
            | (u32::from(hour) << 11)
            | (u32::from(minute) << 5)
            | u32::from(second / 2)
    }

    #[test]
    fn exfat_timestamp_applies_positive_utc_offset() {
        let timestamp = ExfatTimestamp::new(encoded_timestamp(2024, 1, 2, 3, 4, 6), 0, 32);

        assert_eq!(
            exfat_ts_to_rfc3339(&timestamp).as_deref(),
            Some("2024-01-01T19:04:06+00:00")
        );
    }
}

/// `DiskPartition` view onto a partition slice of the underlying `DiskImage`.
/// Cloning is cheap — only an `Arc` bump — so each operation hands a fresh
/// instance to `Root::open` without aliasing concerns.
#[derive(Clone)]
struct PartitionPart {
    image: Arc<DiskImage>,
    base_offset: u64,
    length_bytes: u64,
    boot_sector: [u8; 512],
}

impl exfat::DiskPartition for PartitionPart {
    type Err = std::io::Error;

    fn read(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        if offset >= self.length_bytes || buf.is_empty() {
            return Ok(0);
        }
        let remaining = self.length_bytes - offset;
        let want = (buf.len() as u64).min(remaining) as usize;
        let bytes = self
            .image
            .read_range(self.base_offset + offset, want)
            .map_err(|err| std::io::Error::other(err.to_string()))?;
        let copied = bytes.len();
        buf[..copied].copy_from_slice(&bytes);
        if offset < 512 {
            let overlap = copied.min(512 - offset as usize);
            buf[..overlap]
                .copy_from_slice(&self.boot_sector[offset as usize..offset as usize + overlap]);
        }
        Ok(copied)
    }
}
