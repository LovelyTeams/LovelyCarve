//! Bounded, seekable read-only partition access over any evidence byte source.

use std::io::{self, Read, Seek, SeekFrom};

use anyhow::{ensure, Context};

use super::{MbrPartition, SectorSource};

/// Independent cursor; no global state or host-file writes. A read that crosses
/// the partition end is clamped; a short backing read before EOF is an error.
pub struct MbrPartitionReader<'a> {
    source: &'a dyn SectorSource,
    volume_offset: u64,
    length: u64,
    position: u64,
}

impl<'a> MbrPartitionReader<'a> {
    pub fn new(
        source: &'a dyn SectorSource,
        partition: &MbrPartition,
        sector_size: usize,
    ) -> anyhow::Result<Self> {
        ensure!(
            matches!(sector_size, 512 | 1024 | 2048 | 4096),
            "unsupported MBR sector size: {sector_size}"
        );
        let volume_offset = partition
            .partition
            .start_lba
            .checked_mul(sector_size as u64)
            .context("MBR partition offset overflow")?;
        let length = partition
            .partition
            .length_lba
            .checked_mul(sector_size as u64)
            .context("MBR partition size overflow")?;
        let end = volume_offset
            .checked_add(length)
            .context("MBR partition end overflow")?;
        ensure!(
            end <= source.size(),
            "MBR partition exceeds evidence source"
        );
        Ok(Self {
            source,
            volume_offset,
            length,
            position: 0,
        })
    }

    pub fn volume_offset(&self) -> u64 {
        self.volume_offset
    }
    pub fn size(&self) -> u64 {
        self.length
    }

    /// Positioned read leaves this cursor unchanged, enabling independent
    /// readers to share the same immutable source safely.
    pub fn read_at(&self, offset: u64, length: usize) -> anyhow::Result<Vec<u8>> {
        let length = (length as u64).min(self.length.saturating_sub(offset)) as usize;
        if length == 0 {
            return Ok(Vec::new());
        }
        let bytes = self.source.read(self.volume_offset + offset, length)?;
        ensure!(
            bytes.len() == length,
            "short MBR partition read at {offset}: expected {length}, got {}",
            bytes.len()
        );
        Ok(bytes)
    }
}

impl Read for MbrPartitionReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let bytes = self
            .read_at(self.position, buffer.len())
            .map_err(io::Error::other)?;
        buffer[..bytes.len()].copy_from_slice(&bytes);
        self.position += bytes.len() as u64;
        Ok(bytes.len())
    }
}

impl Seek for MbrPartitionReader<'_> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(position) => i128::from(position),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
            SeekFrom::End(delta) => i128::from(self.length) + i128::from(delta),
        };
        self.position = u64::try_from(position).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "MBR seek offset out of bounds")
        })?;
        Ok(self.position)
    }
}
