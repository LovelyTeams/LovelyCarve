//! Pure Rust VHD/VHDX readers sharing the decoded-image random-read interface.
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::Context;

enum Reader {
    Vhd(vhd::VhdReader),
    Vhdx(vhdx::VhdxReader),
}

pub struct VhdHandle {
    reader: Reader,
    media_size: u64,
    bytes_per_sector: usize,
}

impl std::fmt::Debug for VhdHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VhdHandle")
            .field("media_size", &self.media_size)
            .field("bytes_per_sector", &self.bytes_per_sector)
            .finish_non_exhaustive()
    }
}

impl VhdHandle {
    pub fn open_vhd(path: &Path) -> anyhow::Result<Self> {
        let reader = vhd::VhdReader::open(path)
            .with_context(|| format!("failed to open VHD image: {}", path.display()))?;
        Ok(Self {
            media_size: reader.virtual_disk_size(),
            bytes_per_sector: 512,
            reader: Reader::Vhd(reader),
        })
    }

    pub fn open_vhdx(path: &Path) -> anyhow::Result<Self> {
        let reader = vhdx::VhdxReader::open(path)
            .with_context(|| format!("failed to open VHDX image: {}", path.display()))?;
        Ok(Self {
            media_size: reader.virtual_disk_size(),
            bytes_per_sector: reader.logical_sector_size() as usize,
            reader: Reader::Vhdx(reader),
        })
    }

    pub fn media_size(&self) -> u64 {
        self.media_size
    }
    pub fn bytes_per_sector(&self) -> usize {
        self.bytes_per_sector
    }

    pub fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> anyhow::Result<usize> {
        let length = self
            .media_size
            .saturating_sub(offset)
            .min(buffer.len() as u64) as usize;
        if length == 0 {
            return Ok(0);
        }
        match &mut self.reader {
            Reader::Vhd(reader) => {
                reader.seek(SeekFrom::Start(offset))?;
                reader.read_exact(&mut buffer[..length])?;
            }
            Reader::Vhdx(reader) => {
                reader.seek(SeekFrom::Start(offset))?;
                reader.read_exact(&mut buffer[..length])?;
            }
        }
        Ok(length)
    }
}

#[cfg(test)]
#[path = "vhd_image_tests.rs"]
mod tests;
