//! Pure Rust, read-only ReFS adapter over partition-relative evidence reads.
//! Node IDs remain path based; the legacy DLL is available only to oracle probes.
#[cfg(test)]
#[path = "refs_tests.rs"]
mod tests;
pub use super::refs_backing::BitlockerBacking;
use super::refs_backing::ImageBacking;
pub use super::refs_walk::{WalkEntry, WalkResult, WalkStream, REFS_ROOT_OBJECT_ID};
use crate::evidence::image::{find_partition, DiskImage, Partition};
use crate::filesystem::tree::fs_tree::{
    detect_mime_bytes, shannon_entropy, FsDataRun, FsNode, FsStat, Mactb, NodeKind,
};
use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use refs_reader::{FileEntry, FileReader, ReadAt};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy)]
pub enum RefsSource {
    Partition(u8),
}

/// Parse a `refs:part:{N}:{hex(path)}` ID. Returns the source plus the
/// decoded UTF-8 path (with a leading `/`, or empty for the volume root).
pub fn parse_refs_id(id: &str) -> Option<(RefsSource, String)> {
    let mut parts = id.splitn(4, ':');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("refs"), Some("part"), Some(part_id), Some(hex_path)) => {
            let part_id = part_id.parse().ok()?;
            let path = if hex_path.is_empty() {
                String::new()
            } else {
                let bytes = hex::decode(hex_path).ok()?;
                String::from_utf8(bytes).ok()?
            };
            Some((RefsSource::Partition(part_id), path))
        }
        _ => None,
    }
}

/// Parse a `refs-snap:part:{N}:{hex(path)}:{hex(stream_name)}` ID — the
/// pseudo-file id minted for a ReFS file-snapshot stream surfaced as an
/// independent file row. Returns the source, the decoded owning-file path
/// (leading `/`), and the decoded stream name. Both the path and the stream
/// name are hex-encoded so arbitrary bytes / `:` in either are safe.
pub fn parse_refs_snap_id(id: &str) -> Option<(RefsSource, String, String)> {
    let mut parts = id.splitn(5, ':');
    match (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) {
        (Some("refs-snap"), Some("part"), Some(part_id), Some(hex_path), Some(hex_stream)) => {
            let part_id = part_id.parse().ok()?;
            let path = String::from_utf8(hex::decode(hex_path).ok()?).ok()?;
            let stream = String::from_utf8(hex::decode(hex_stream).ok()?).ok()?;
            if path.is_empty() || stream.is_empty() {
                return None;
            }
            Some((RefsSource::Partition(part_id), path, stream))
        }
        _ => None,
    }
}

pub fn open_refs_source(image: &Arc<DiskImage>, source: RefsSource) -> Result<RefsVolume> {
    match source {
        RefsSource::Partition(part_id) => {
            let partition = find_partition(image, &part_id.to_string())?;
            RefsVolume::open(Arc::clone(image), partition)
        }
    }
}

pub struct RefsVolume {
    partition_id: u8,
    length_bytes: u64,
    base_offset: u64,
    reader: refs_reader::Volume,
}

impl RefsVolume {
    pub fn open(image: Arc<DiskImage>, partition: &Partition) -> Result<Self> {
        Self::open_checked(image, partition, &|| Ok(()))
    }

    pub fn open_with_cancel(
        image: Arc<DiskImage>,
        partition: &Partition,
        cancel: &CancellationToken,
    ) -> Result<Self> {
        Self::open_checked(image, partition, &|| {
            if cancel.is_cancelled() {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "snapshot build cancelled",
                ))
            } else {
                Ok(())
            }
        })
    }

    fn open_checked(
        image: Arc<DiskImage>,
        partition: &Partition,
        check: &(dyn Fn() -> std::io::Result<()> + Sync),
    ) -> Result<Self> {
        let sector_size = u64::try_from(image.sector_size)?;
        let base_offset = partition
            .start_lba
            .checked_mul(sector_size)
            .context("ReFS partition offset overflow")?;
        let length_bytes = partition
            .length_lba
            .checked_mul(sector_size)
            .context("ReFS partition length overflow")?;
        base_offset
            .checked_add(length_bytes)
            .context("ReFS partition range overflow")?;
        let source: Arc<dyn ReadAt> = Arc::new(ImageBacking {
            image,
            base_offset,
            length_bytes,
        });
        let reader = refs_reader::Volume::open_checked(source, check)
            .with_context(|| format!("ReFS mount partition {}", partition.id))?;
        Ok(Self {
            partition_id: partition.id,
            length_bytes,
            base_offset,
            reader,
        })
    }

    pub fn root_children(&self) -> Result<Vec<FsNode>> {
        self.children("")
    }
    pub fn partition_id(&self) -> u8 {
        self.partition_id
    }
    pub fn partition_length(&self) -> u64 {
        self.length_bytes
    }
    pub fn walk_tree(&self, cancel: &CancellationToken) -> Result<WalkResult> {
        super::refs_walk::collect(&self.reader, cancel)
    }
    pub fn node_id_for_path(&self, path: &str) -> String {
        if path.is_empty() || path == "/" {
            format!("part:{}", self.partition_id)
        } else {
            format!(
                "refs:part:{}:{}",
                self.partition_id,
                hex::encode(path.as_bytes())
            )
        }
    }

    pub fn children(&self, path: &str) -> Result<Vec<FsNode>> {
        let object_id = if path.is_empty() || path == "/" {
            REFS_ROOT_OBJECT_ID
        } else {
            self.reader
                .lookup(path)?
                .directory_id
                .context("ReFS listing path is not a directory")?
        };
        let parent_id = self.node_id_for_path(path);
        let mut output = self
            .reader
            .list_directory(object_id)?
            .into_iter()
            .map(|entry| {
                let child_path = format!("{}/{}", path.trim_end_matches('/'), entry.name);
                FsNode {
                    id: self.node_id_for_path(&child_path),
                    name: entry.name.clone(),
                    parent_id: Some(parent_id.clone()),
                    kind: if entry.is_directory() {
                        NodeKind::Dir
                    } else {
                        NodeKind::File
                    },
                    size: entry.size,
                    deleted: false,
                    inode: entry.inode,
                    timestamps: entry_timestamps(&entry),
                    link_target: entry.symlink_target,
                    has_snapshot: false,
                }
            })
            .collect::<Vec<_>>();
        output.sort_by(|left, right| {
            u8::from(left.kind != NodeKind::Dir)
                .cmp(&u8::from(right.kind != NodeKind::Dir))
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        Ok(output)
    }

    pub fn read_file(&self, path: &str, max_bytes: usize) -> Result<Vec<u8>> {
        self.read_range(path, 0, max_bytes)
    }
    pub fn read_range(&self, path: &str, offset: u64, length: usize) -> Result<Vec<u8>> {
        let file = self.reader.open_file(self.reader.lookup(path)?)?;
        read_reader_range(&file, offset, length)
    }
    pub fn stream_file<F>(&self, path: &str, chunk_size: usize, visitor: F) -> Result<u64>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        let file = self.reader.open_file(self.reader.lookup(path)?)?;
        stream_reader(&file, chunk_size, visitor)
    }
    pub fn list_streams(&self, path: &str) -> Result<Vec<super::ntfs::NtfsDataStream>> {
        if path.is_empty() || path == "/" {
            return Ok(Vec::new());
        }
        let entry = self.reader.lookup(path)?;
        Ok(self
            .reader
            .list_streams(&entry)?
            .into_iter()
            .map(|stream| super::ntfs::NtfsDataStream {
                name: stream.name,
                size: stream.size,
                resident: stream.resident,
            })
            .collect())
    }
    pub fn read_named_stream(&self, path: &str, name: &str, max_bytes: usize) -> Result<Vec<u8>> {
        self.read_named_stream_range(path, name, 0, max_bytes)
    }
    pub fn read_named_stream_range(
        &self,
        path: &str,
        name: &str,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>> {
        let file = self
            .reader
            .open_named_stream(self.reader.lookup(path)?, name)?;
        read_reader_range(&file, offset, length)
    }
    pub fn stream_named_stream<F>(
        &self,
        path: &str,
        name: &str,
        chunk_size: usize,
        visitor: F,
    ) -> Result<u64>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        let file = self
            .reader
            .open_named_stream(self.reader.lookup(path)?, name)?;
        stream_reader(&file, chunk_size, visitor)
    }
    pub fn file_stat(&self, path: &str, node_id: &str) -> Result<FsStat> {
        let entry = self.reader.lookup(path)?;
        if entry.is_directory() {
            let mut stat = stat_from_sample(node_id, entry.size, Vec::new(), Vec::new());
            stat.inode = entry.inode;
            stat.mode = Some(0o40777);
            return Ok(stat);
        }
        let file = self.reader.open_file(entry)?;
        self.reader_stat(&file, node_id)
    }
    pub fn named_stream_stat(&self, path: &str, name: &str, node_id: &str) -> Result<FsStat> {
        let file = self
            .reader
            .open_named_stream(self.reader.lookup(path)?, name)?;
        self.reader_stat(&file, node_id)
    }
    fn reader_stat(&self, file: &FileReader, node_id: &str) -> Result<FsStat> {
        let sample = read_reader_range(file, 0, 4096)?;
        let runs = file
            .extents()
            .iter()
            .filter(|extent| extent.logical < file.entry().size)
            .filter_map(|extent| extent.physical.map(|physical| (extent, physical)))
            .map(|(extent, physical)| {
                Ok(FsDataRun {
                    logical_offset: extent.logical,
                    physical_offset: self
                        .base_offset
                        .checked_add(physical)
                        .context("ReFS data run offset overflow")?,
                    length: extent
                        .length
                        .min(file.entry().size.saturating_sub(extent.logical)),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mut stat = stat_from_sample(node_id, file.entry().size, sample, runs);
        stat.inode = file.entry().inode;
        stat.mode = Some(if file.entry().symlink_target.is_some() {
            0o120777
        } else {
            0o100777
        });
        Ok(stat)
    }
}

fn read_reader_range(file: &FileReader, offset: u64, length: usize) -> Result<Vec<u8>> {
    let count = file.entry().size.saturating_sub(offset).min(length as u64) as usize;
    let mut bytes = vec![0; count];
    let read = file.read_range(offset, &mut bytes)?;
    anyhow::ensure!(read == count, "short ReFS read: {read} of {count} bytes");
    Ok(bytes)
}

fn stream_reader<F>(file: &FileReader, chunk_size: usize, mut visitor: F) -> Result<u64>
where
    F: FnMut(&[u8]) -> Result<()>,
{
    let chunk_size = chunk_size.clamp(1, 16 * 1024 * 1024);
    let mut buffer =
        vec![0; chunk_size.min(usize::try_from(file.entry().size).unwrap_or(usize::MAX))];
    let mut offset = 0;
    while offset < file.entry().size {
        let count = file.read_range(offset, &mut buffer)?;
        anyhow::ensure!(
            count > 0,
            "short ReFS stream at {offset} of {} bytes",
            file.entry().size
        );
        visitor(&buffer[..count])?;
        offset = offset
            .checked_add(count as u64)
            .context("ReFS stream offset overflow")?;
    }
    anyhow::ensure!(offset == file.entry().size, "ReFS stream size mismatch");
    Ok(offset)
}

fn stat_from_sample(
    node_id: &str,
    size: u64,
    sample: Vec<u8>,
    data_runs: Vec<FsDataRun>,
) -> FsStat {
    FsStat {
        node_id: node_id.to_string(),
        size,
        inode: None,
        mode: None,
        mime: detect_mime_bytes(&sample).to_string(),
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
    }
}

pub(super) fn entry_timestamps(entry: &FileEntry) -> Mactb {
    Mactb {
        created: filetime(entry.created),
        modified: filetime(entry.modified),
        accessed: filetime(entry.accessed),
        mft_changed: filetime(entry.changed),
        birth: filetime(entry.created),
    }
}

pub fn walk_entry_timestamps(entry: &WalkEntry) -> Mactb {
    entry.timestamps.clone()
}

fn filetime(ticks: u64) -> Option<String> {
    if ticks == 0 {
        return None;
    }
    let ticks = i128::from(ticks) - 116_444_736_000_000_000;
    let seconds = i64::try_from(ticks.div_euclid(10_000_000)).ok()?;
    let nanos = u32::try_from(ticks.rem_euclid(10_000_000) * 100).ok()?;
    Utc.timestamp_opt(seconds, nanos)
        .single()
        .map(|time| time.to_rfc3339())
}
