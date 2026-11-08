//! A single validated extent plan for complete, streamed and seekable reads.
use crate::catalog::FileExtentRecord;
use crate::error::{ApfsError, Result};
use std::io::{Read, Seek, SeekFrom, Write};

/// Read the complete logical fork, rejecting missing tails and source short reads.
pub fn read_file_data<R: Read + Seek, W: Write>(
    reader: &mut R,
    block_size: u32,
    extents: &[FileExtentRecord],
    logical_size: u64,
    writer: &mut W,
) -> Result<u64> {
    let mut fork = ApfsForkReader::new(reader, block_size, extents.to_vec(), logical_size)?;
    Ok(std::io::copy(&mut fork, writer)?)
}

/// A validated fork. Physical block zero denotes an explicit sparse extent.
pub struct ApfsForkReader<'a, R: Read + Seek> {
    reader: &'a mut R,
    logical_size: u64,
    // (logical start, optional physical byte offset, length)
    extent_map: Vec<(u64, Option<u64>, u64)>,
    position: u64,
}

impl<'a, R: Read + Seek> ApfsForkReader<'a, R> {
    pub fn new(
        reader: &'a mut R,
        block_size: u32,
        mut extents: Vec<FileExtentRecord>,
        logical_size: u64,
    ) -> Result<Self> {
        if block_size == 0 {
            return Err(ApfsError::CorruptedData("zero block size".into()));
        }
        let source_length = reader.seek(SeekFrom::End(0))?;
        extents.sort_by_key(|record| record.logical_addr);
        let mut extent_map = Vec::new();
        let mut previous_end = 0;
        for record in extents {
            let length = record.value.length();
            if length == 0 {
                return Err(ApfsError::CorruptedData("zero-length extent".into()));
            }
            let end = record
                .logical_addr
                .checked_add(length)
                .ok_or_else(|| ApfsError::CorruptedData("extent logical end overflow".into()))?;
            if record.logical_addr < previous_end {
                return Err(ApfsError::CorruptedData("overlapping file extents".into()));
            }
            if record.value.flags_and_length >> 56 != 0 {
                return Err(ApfsError::Unsupported("file extent flags".into()));
            }
            let physical = if record.value.phys_block_num == 0 {
                None
            } else {
                let start = record
                    .value
                    .phys_block_num
                    .checked_mul(u64::from(block_size))
                    .ok_or_else(|| {
                        ApfsError::CorruptedData("extent physical offset overflow".into())
                    })?;
                let physical_end = start.checked_add(length).ok_or_else(|| {
                    ApfsError::CorruptedData("extent physical end overflow".into())
                })?;
                if physical_end > source_length {
                    return Err(ApfsError::CorruptedData(
                        "allocated extent exceeds source".into(),
                    ));
                }
                Some(start)
            };
            extent_map.push((record.logical_addr, physical, length));
            previous_end = end;
        }
        // An explicit block-zero extent can represent a trailing/all-file hole.
        // Without it, absent tail records cannot be certified as authentic zeroes.
        if previous_end < logical_size {
            return Err(ApfsError::CorruptedData(
                "file logical size exceeds extent map".into(),
            ));
        }
        Ok(Self {
            reader,
            logical_size,
            extent_map,
            position: 0,
        })
    }
}

impl<R: Read + Seek> Read for ApfsForkReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.position >= self.logical_size {
            return Ok(0);
        }
        let wanted = (self.logical_size - self.position).min(buffer.len() as u64) as usize;
        let mut copied = 0;
        while copied < wanted {
            let offset = self.position + copied as u64;
            let index = self
                .extent_map
                .partition_point(|(start, _, _)| *start <= offset);
            let mapping = index.checked_sub(1).and_then(|i| self.extent_map.get(i));
            let (physical, available) = match mapping {
                Some(&(start, physical, length)) if offset < start + length => (
                    physical.map(|address| address + offset - start),
                    start + length - offset,
                ),
                _ => (
                    None,
                    self.extent_map
                        .get(index)
                        .map_or(self.logical_size, |entry| entry.0)
                        - offset,
                ),
            };
            let count = available.min((wanted - copied) as u64) as usize;
            if count == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "APFS extent map cannot advance",
                ));
            }
            if let Some(address) = physical {
                self.reader.seek(SeekFrom::Start(address))?;
                self.reader
                    .read_exact(&mut buffer[copied..copied + count])?;
            } else {
                buffer[copied..copied + count].fill(0);
            }
            copied += count;
        }
        self.position += copied as u64;
        Ok(copied)
    }
}
impl<R: Read + Seek> Seek for ApfsForkReader<'_, R> {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        let target = match position {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.logical_size) + i128::from(offset),
        };
        self.position = u64::try_from(target).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "APFS seek out of range")
        })?;
        Ok(self.position)
    }
}
#[cfg(test)]
mod tests;
