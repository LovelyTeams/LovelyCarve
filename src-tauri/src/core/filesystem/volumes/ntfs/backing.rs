//! Bounded read and seek adapters for NTFS partitions.
use super::*;
use crate::core::common::error::Failure;

/// Backing source for an NTFS volume: a partition slice of a `DiskImage`, or
/// an LVM2 logical volume mapped over one. An unlocked encrypted volume is a
/// partition of its plaintext image.
#[derive(Clone)]
pub(super) struct PartitionBacking<'a> {
    pub(super) image: &'a DiskImage,
    pub(super) base_offset: u64,
    pub(super) length_bytes: u64,
    pub(super) layout: Option<crate::core::evidence::partitions::lvm2::layout::LinearLayout>,
}

impl<'a> PartitionBacking<'a> {
    pub(super) fn length(&self) -> u64 {
        self.length_bytes
    }

    pub(super) fn physical_ranges(&self, position: u64, length: u64) -> Result<Vec<(u64, u64)>> {
        match &self.layout {
            Some(layout) => Ok(layout
                .ranges(position, length)?
                .into_iter()
                .map(|range| (range.physical_offset, range.size))
                .collect()),
            None => Ok(vec![(
                self.base_offset
                    .checked_add(position)
                    .ok_or_else(|| anyhow!(Failure::corrupt("NTFS source offset overflow")))?,
                length,
            )]),
        }
    }

    pub(super) fn read_into(&self, position: u64, buf: &mut [u8]) -> io::Result<usize> {
        let bytes = crate::core::filesystem::volumes::lvm::read_backing_range(
            self.image,
            self.layout.as_ref(),
            self.base_offset + position,
            buf.len(),
        )
        .map_err(|err| io::Error::other(err.to_string()))?;
        let copied = bytes.len();
        buf[..copied].copy_from_slice(&bytes);
        Ok(copied)
    }

    pub(super) fn reader(&self) -> PartitionReader<'a> {
        PartitionReader {
            backing: self.clone(),
            position: 0,
        }
    }
}

pub(super) struct PartitionReader<'a> {
    pub(super) backing: PartitionBacking<'a>,
    pub(super) position: u64,
}

impl Read for PartitionReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let length = self.backing.length();
        if self.position >= length || buf.is_empty() {
            return Ok(0);
        }
        let remaining = length - self.position;
        let want = (buf.len() as u64).min(remaining) as usize;
        let copied = self.backing.read_into(self.position, &mut buf[..want])?;
        self.position += copied as u64;
        Ok(copied)
    }
}

impl Seek for PartitionReader<'_> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let length = self.backing.length();
        let target = match pos {
            SeekFrom::Start(offset) => offset as i128,
            SeekFrom::Current(delta) => self.position as i128 + delta as i128,
            SeekFrom::End(delta) => length as i128 + delta as i128,
        };
        if target < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "NTFS partition seek before start",
            ));
        }
        self.position = target as u64;
        Ok(self.position)
    }
}
