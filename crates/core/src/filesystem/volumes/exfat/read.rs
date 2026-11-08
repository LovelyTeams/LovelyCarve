//! exFAT file access using the reader's validated allocation map.

use std::io::{Read, Seek, SeekFrom};

use anyhow::{anyhow, bail, Result};

use super::{timestamps_to_mactb, ExfatVolume};
use crate::filesystem::fs_dispatch::build_stat;
use crate::filesystem::tree::fs_tree::{FsDataRun, FsStat, Mactb};

/// Native exFAT metadata; uninitialized bytes through `size` read as zero.
#[derive(Debug, Clone)]
pub struct ExfatFileMetadata {
    pub entry_offset: u64,
    pub name: String,
    pub attributes: u16,
    pub size: u64,
    pub valid_data_length: u64,
    pub timestamps: Mactb,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExfatVolumeMetadata {
    pub label: Option<String>,
    pub serial_number: u32,
    pub cluster_size: u64,
}

impl ExfatVolume {
    pub fn volume_metadata(&self) -> Result<ExfatVolumeMetadata> {
        let boot = self.boot_sector;
        Ok(ExfatVolumeMetadata {
            label: self.root.volume_label().map(str::to_string),
            serial_number: u32::from_le_bytes(boot[100..104].try_into()?),
            cluster_size: 1u64 << (boot[108] + boot[109]),
        })
    }

    pub fn file_metadata(&self, path: &str) -> Result<ExfatFileMetadata> {
        let file = self.open_file(path)?;
        Ok(ExfatFileMetadata {
            entry_offset: self
                .base_offset
                .checked_add(file.entry_offset())
                .ok_or_else(|| anyhow!("exFAT offset overflow"))?,
            name: file.name().to_string(),
            attributes: file.attributes().bits(),
            size: file.len(),
            valid_data_length: file.valid_data_length(),
            timestamps: timestamps_to_mactb(file.timestamps()),
        })
    }

    pub fn live_data_runs(&self, path: &str) -> Result<Vec<FsDataRun>> {
        self.file_runs(&self.open_file(path)?)
    }

    fn file_runs(&self, file: &exfat::file::File<super::PartitionPart>) -> Result<Vec<FsDataRun>> {
        let mut runs: Vec<FsDataRun> = Vec::new();
        let mut logical_offset = 0;
        for (offset, length) in file.extents() {
            if offset
                .checked_add(length)
                .is_none_or(|end| end > self.length_bytes)
            {
                bail!("exFAT extent outside partition");
            }
            let physical_offset = self
                .base_offset
                .checked_add(offset)
                .ok_or_else(|| anyhow!("exFAT extent offset overflow"))?;
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
        if logical_offset != file.len() {
            bail!("exFAT incomplete allocation map");
        }
        Ok(runs)
    }

    pub fn read_file(&self, path: &str, max_bytes: usize) -> Result<Vec<u8>> {
        self.read_file_range(path, 0, max_bytes)
    }

    pub fn read_file_range(&self, path: &str, offset: u64, length: usize) -> Result<Vec<u8>> {
        let mut file = self.open_file(path)?;
        self.file_runs(&file)?;
        if offset >= file.len() || length == 0 {
            return Ok(Vec::new());
        }
        let length = (file.len() - offset).min(length as u64) as usize;
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| anyhow!("exFAT seek {path}: {error}"))?;
        let mut bytes = vec![0; length];
        file.read_exact(&mut bytes)
            .map_err(|error| anyhow!("exFAT read {path}: {error}"))?;
        Ok(bytes)
    }

    pub fn stream_file<F>(&self, path: &str, chunk_size: usize, mut visitor: F) -> Result<u64>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        let mut file = self.open_file(path)?;
        self.file_runs(&file)?;
        let mut buffer = vec![0; chunk_size.max(64 * 1024)];
        let mut total = 0;
        while total < file.len() {
            let want = (file.len() - total).min(buffer.len() as u64) as usize;
            let count = file
                .read(&mut buffer[..want])
                .map_err(|error| anyhow!("exFAT stream {path}: {error}"))?;
            if count == 0 {
                bail!("exFAT truncated file {path}");
            }
            visitor(&buffer[..count])?;
            total += count as u64;
        }
        Ok(total)
    }

    pub fn file_stat(&self, path: &str, node_id: &str) -> Result<FsStat> {
        let mut file = self.open_file(path)?;
        let runs = self.file_runs(&file)?;
        let mut sample = vec![0; file.len().min(4096) as usize];
        file.read_exact(&mut sample)
            .map_err(|error| anyhow!("exFAT sample {path}: {error}"))?;
        Ok(build_stat(
            node_id,
            file.len(),
            Some(self.base_offset + file.entry_offset()),
            None,
            &sample,
            runs,
        ))
    }
}
