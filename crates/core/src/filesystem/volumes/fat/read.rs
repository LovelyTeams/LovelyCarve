//! Validated FAT file layouts shared by full, ranged, streamed and stat reads.

use std::collections::HashSet;

use anyhow::{anyhow, bail, Result};

use super::{entry_timestamps, leaf_name, parent_dir, FatVolume};
use crate::filesystem::fs_dispatch::build_stat;
use crate::filesystem::tree::fs_tree::{FsDataRun, FsStat, Mactb};

/// Source FAT metadata; attributes are FAT flags, not POSIX mode bits.
#[derive(Debug, Clone)]
pub struct FatFileMetadata {
    pub entry_offset: u64,
    pub name: String,
    pub short_name: String,
    pub attributes: u8,
    pub size: u64,
    pub timestamps: Mactb,
}

/// Volume identity as recorded by the BPB and root label entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FatVolumeMetadata {
    pub format: String,
    pub label: String,
    pub serial_number: u32,
    pub cluster_size: u32,
}

impl FatVolume {
    pub fn volume_metadata(&self) -> Result<FatVolumeMetadata> {
        self.with_fs(|fs| {
            let label = fs
                .read_volume_label_from_root_dir()
                .map_err(|error| anyhow!("FAT volume label: {error}"))?
                .unwrap_or_else(|| fs.volume_label());
            Ok(FatVolumeMetadata {
                format: match fs.fat_type() {
                    fatfs::FatType::Fat12 => "FAT12",
                    fatfs::FatType::Fat16 => "FAT16",
                    fatfs::FatType::Fat32 => "FAT32",
                }
                .to_string(),
                label,
                serial_number: fs.volume_id(),
                cluster_size: fs.cluster_size(),
            })
        })
    }

    pub fn file_metadata(&self, path: &str) -> Result<FatFileMetadata> {
        self.file_layout(path).map(|(metadata, _)| metadata)
    }

    fn file_layout(&self, path: &str) -> Result<(FatFileMetadata, Vec<FsDataRun>)> {
        let relative = path.trim_start_matches('/');
        let leaf = leaf_name(relative)?;
        let parent = parent_dir(relative);
        self.with_fs(|fs| {
            let directory = if parent.is_empty() {
                fs.root_dir()
            } else {
                fs.root_dir()
                    .open_dir(parent)
                    .map_err(|error| anyhow!("FAT directory {path}: {error}"))?
            };
            for entry in directory.iter() {
                let entry = entry.map_err(|error| anyhow!("FAT entry {path}: {error}"))?;
                if !entry.eq_name(leaf) {
                    continue;
                }
                let metadata = FatFileMetadata {
                    entry_offset: self
                        .base_offset
                        .checked_add(entry.entry_offset())
                        .ok_or_else(|| anyhow!("FAT entry offset overflow"))?,
                    name: entry.file_name(),
                    short_name: entry.short_file_name(),
                    attributes: entry.attributes().bits(),
                    size: entry.len(),
                    timestamps: entry_timestamps(&entry),
                };
                if entry.is_dir()
                    || entry
                        .attributes()
                        .contains(fatfs::FileAttributes::VOLUME_ID)
                {
                    bail!("FAT entry {path} is not a regular file");
                }
                let mut file = entry.to_file();
                let mut runs: Vec<FsDataRun> = Vec::new();
                let mut visited = HashSet::new();
                let mut logical_offset = 0;
                for extent in file.extents() {
                    let extent = extent.map_err(|error| anyhow!("FAT extents {path}: {error}"))?;
                    let length = u64::from(extent.size);
                    if length == 0 || !visited.insert(extent.offset) {
                        bail!("FAT invalid or cyclic cluster chain: {path}");
                    }
                    if extent
                        .offset
                        .checked_add(length)
                        .is_none_or(|end| end > self.length_bytes)
                    {
                        bail!("FAT extent outside partition: {path}");
                    }
                    let physical_offset = self
                        .base_offset
                        .checked_add(extent.offset)
                        .ok_or_else(|| anyhow!("FAT extent offset overflow: {path}"))?;
                    if let Some(previous) = runs
                        .last_mut()
                        .filter(|run| run.physical_offset + run.length == physical_offset)
                    {
                        previous.length += length;
                    } else {
                        runs.push(FsDataRun {
                            logical_offset,
                            physical_offset,
                            length,
                        });
                    }
                    logical_offset += length;
                }
                if logical_offset != metadata.size {
                    bail!(
                        "FAT truncated cluster chain {path}: {logical_offset}/{} bytes",
                        metadata.size
                    );
                }
                return Ok((metadata, runs));
            }
            bail!("FAT file not found: {path}")
        })
    }

    fn read_layout(
        &self,
        runs: &[FsDataRun],
        size: u64,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>> {
        let length = (length as u64).min(size.saturating_sub(offset)) as usize;
        let end = offset + length as u64;
        let mut bytes = Vec::with_capacity(length);
        for run in runs {
            let start = offset.max(run.logical_offset);
            let stop = end.min(run.logical_offset + run.length);
            if start >= stop {
                continue;
            }
            let want = (stop - start) as usize;
            let chunk = self
                .image
                .read_range(run.physical_offset + start - run.logical_offset, want)?;
            if chunk.len() != want {
                bail!("FAT data short read: {}/{want} bytes", chunk.len());
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.len() != length {
            bail!("FAT incomplete file range");
        }
        Ok(bytes)
    }

    pub fn read_file(&self, path: &str, max_bytes: usize) -> Result<Vec<u8>> {
        self.read_file_range(path, 0, max_bytes)
    }

    /// Reads only the requested physical extents; offset beyond EOF returns empty.
    pub fn read_file_range(&self, path: &str, offset: u64, length: usize) -> Result<Vec<u8>> {
        let (metadata, runs) = self.file_layout(path)?;
        if offset >= metadata.size || length == 0 {
            return Ok(Vec::new());
        }
        self.read_layout(&runs, metadata.size, offset, length)
    }

    pub fn stream_file<F>(&self, path: &str, chunk_size: usize, mut visitor: F) -> Result<u64>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        let (metadata, runs) = self.file_layout(path)?;
        let chunk_size = chunk_size.max(64 * 1024);
        for run in &runs {
            let mut position = 0;
            while position < run.length {
                let want = (run.length - position).min(chunk_size as u64) as usize;
                let bytes = self
                    .image
                    .read_range(run.physical_offset + position, want)?;
                if bytes.len() != want {
                    bail!("FAT data short read: {}/{want} bytes", bytes.len());
                }
                visitor(&bytes)?;
                position += want as u64;
            }
        }
        Ok(metadata.size)
    }

    pub fn data_runs(&self, path: &str) -> Result<Vec<FsDataRun>> {
        if path.is_empty() {
            return Ok(Vec::new());
        }
        self.file_layout(path).map(|(_, runs)| runs)
    }

    pub fn file_stat(&self, path: &str, node_id: &str) -> Result<FsStat> {
        let (metadata, runs) = self.file_layout(path)?;
        let sample = self.read_layout(&runs, metadata.size, 0, 4096)?;
        Ok(build_stat(
            node_id,
            metadata.size,
            Some(metadata.entry_offset),
            None,
            &sample,
            runs,
        ))
    }
}
