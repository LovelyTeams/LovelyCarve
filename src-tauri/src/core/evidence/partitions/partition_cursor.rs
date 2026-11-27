//! Bounded seekable access to a disk-image partition.

use super::{Partition, SectorSource};
use crate::core::common::error::Failure;
use crate::core::evidence::image::DiskImage;
use anyhow::{ensure, Context, Result};
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct PartitionCursor {
    image: Arc<DiskImage>,
    pub base_offset: u64,
    pub length_bytes: u64,
    position: u64,
}

impl PartitionCursor {
    pub(crate) fn new(image: Arc<DiskImage>, partition: &Partition) -> Result<Self> {
        let sector_size = image.sector_size as u64;
        let base_offset = partition
            .start_lba
            .checked_mul(sector_size)
            .context(Failure::corrupt("partition offset overflow"))?;
        let length_bytes = partition
            .length_lba
            .checked_mul(sector_size)
            .context(Failure::corrupt("partition length overflow"))?;
        ensure!(
            base_offset
                .checked_add(length_bytes)
                .is_some_and(|end| end <= image.len()),
            Failure::corrupt("partition exceeds source")
        );
        Ok(Self {
            image,
            base_offset,
            length_bytes,
            position: 0,
        })
    }
}

impl Read for PartitionCursor {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let want = self
            .length_bytes
            .saturating_sub(self.position)
            .min(buf.len() as u64) as usize;
        if want == 0 {
            return Ok(0);
        }
        let bytes = self
            .image
            .read_range(self.base_offset + self.position, want)
            .map_err(io::Error::other)?;
        let copied = bytes.len();
        buf[..copied].copy_from_slice(&bytes);
        self.position += copied as u64;
        Ok(copied)
    }
}

impl Seek for PartitionCursor {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
            SeekFrom::End(delta) => i128::from(self.length_bytes) + i128::from(delta),
        };
        self.position = u64::try_from(target).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "partition seek out of range")
        })?;
        Ok(self.position)
    }
}

/// Seekable reads over a borrowed byte source, for header probes that run
/// while partitions are detected, before a disk image exists. Reads end at
/// the end of the source.
pub(crate) struct SourceCursor<'a> {
    source: &'a dyn SectorSource,
    position: u64,
}

impl<'a> SourceCursor<'a> {
    pub(crate) fn new(source: &'a dyn SectorSource) -> Self {
        Self {
            source,
            position: 0,
        }
    }
}

impl Read for SourceCursor<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let want = self
            .source
            .size()
            .saturating_sub(self.position)
            .min(buf.len() as u64) as usize;
        if want == 0 {
            return Ok(0);
        }
        let bytes = self
            .source
            .read(self.position, want)
            .map_err(io::Error::other)?;
        let copied = bytes.len().min(want);
        buf[..copied].copy_from_slice(&bytes[..copied]);
        self.position += copied as u64;
        Ok(copied)
    }
}

impl Seek for SourceCursor<'_> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
            SeekFrom::End(delta) => i128::from(self.source.size()) + i128::from(delta),
        };
        self.position = u64::try_from(target)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "source seek out of range"))?;
        Ok(self.position)
    }
}
