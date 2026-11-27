//! A single validated extent plan for complete, streamed and seekable reads.
use crate::catalog::FileExtentRecord;
use crate::error::{ApfsError, Result};
use std::io::{Read, Seek, SeekFrom, Write};

use crate::crypto::{CRYPTO_SECTOR, VolumeCipher};

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
    extent_map: Vec<Extent>,
    cipher: Option<&'a VolumeCipher>,
    position: u64,
}

struct Extent {
    logical: u64,
    length: u64,
    allocation: Option<Allocation>,
}

#[derive(Clone, Copy)]
struct Allocation {
    offset: u64,
    first_sector: u64,
}

impl<'a, R: Read + Seek> ApfsForkReader<'a, R> {
    pub fn new(
        reader: &'a mut R,
        block_size: u32,
        extents: Vec<FileExtentRecord>,
        logical_size: u64,
    ) -> Result<Self> {
        let source_length = reader.seek(SeekFrom::End(0))?;
        Self::new_checked(
            reader,
            block_size,
            extents,
            logical_size,
            source_length,
            None,
            None,
        )
    }

    fn new_checked(
        reader: &'a mut R,
        block_size: u32,
        mut extents: Vec<FileExtentRecord>,
        logical_size: u64,
        main: u64,
        tier2: Option<u64>,
        cipher: Option<&'a VolumeCipher>,
    ) -> Result<Self> {
        if block_size == 0 {
            return Err(ApfsError::CorruptedData("zero block size".into()));
        }
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
            let flags = record.value.flags_and_length >> 56;
            // FEXT_CRYPTO_ID_IS_TWEAK marks the on-disk tweak used by ONEKEY volumes.
            if flags & !1 != 0 || flags == 1 && cipher.is_none() {
                return Err(ApfsError::Unsupported("file extent flags".into()));
            }
            let allocation = if record.value.phys_block_num == 0 {
                None
            } else {
                let start = record
                    .value
                    .phys_block_num
                    .checked_mul(u64::from(block_size))
                    .ok_or_else(|| {
                        ApfsError::CorruptedData("extent physical offset overflow".into())
                    })?;
                let (read_length, first_sector) = if cipher.is_some() {
                    let sector_size = CRYPTO_SECTOR as u64;
                    let aligned =
                        length
                            .checked_next_multiple_of(sector_size)
                            .ok_or_else(|| {
                                ApfsError::CorruptedData(
                                    "APFS encrypted extent size overflow".into(),
                                )
                            })?;
                    let sector = record
                        .value
                        .crypto_id
                        .checked_mul(u64::from(block_size) / sector_size)
                        .filter(|sector| sector.checked_add(aligned / sector_size - 1).is_some())
                        .ok_or_else(|| {
                            ApfsError::CorruptedData("APFS extent crypto ID overflow".into())
                        })?;
                    (aligned, sector)
                } else {
                    (length, 0)
                };
                if !crate::backing::contains(start, read_length, main, tier2) {
                    return Err(ApfsError::CorruptedData(
                        "allocated extent exceeds source".into(),
                    ));
                }
                Some(Allocation {
                    offset: start,
                    first_sector,
                })
            };
            extent_map.push(Extent {
                logical: record.logical_addr,
                length,
                allocation,
            });
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
            cipher,
            position: 0,
        })
    }
}

impl<'a, R: Read + Seek> ApfsForkReader<'a, crate::backing::ContainerReader<R>> {
    pub(crate) fn from_container(
        reader: &'a mut crate::backing::ContainerReader<R>,
        block_size: u32,
        extents: Vec<FileExtentRecord>,
        logical_size: u64,
        cipher: Option<&'a VolumeCipher>,
    ) -> Result<Self> {
        let (main, tier2) = reader.range_limits();
        Self::new_checked(
            reader,
            block_size,
            extents,
            logical_size,
            main,
            tier2,
            cipher,
        )
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
                .partition_point(|extent| extent.logical <= offset);
            let mapping = index.checked_sub(1).and_then(|i| self.extent_map.get(i));
            let (allocation, within, available) = match mapping {
                Some(extent) if offset < extent.logical + extent.length => {
                    let within = offset - extent.logical;
                    (extent.allocation, within, extent.length - within)
                }
                _ => (
                    None,
                    0,
                    self.extent_map
                        .get(index)
                        .map_or(self.logical_size, |entry| entry.logical)
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
            let output = &mut buffer[copied..copied + count];
            match (allocation, self.cipher) {
                (Some(source), Some(cipher)) => {
                    read_encrypted(self.reader, source, within, output, cipher)?
                }
                (Some(source), None) => {
                    self.reader.seek(SeekFrom::Start(source.offset + within))?;
                    self.reader.read_exact(output)?;
                }
                (None, _) => output.fill(0),
            }
            copied += count;
        }
        self.position += copied as u64;
        Ok(copied)
    }
}

fn read_encrypted<R: Read + Seek>(
    reader: &mut R,
    source: Allocation,
    within: u64,
    mut output: &mut [u8],
    cipher: &VolumeCipher,
) -> std::io::Result<()> {
    let sector_size = CRYPTO_SECTOR as u64;
    let mut sector = source.first_sector + within / sector_size;
    let mut skip = (within % sector_size) as usize;
    reader.seek(SeekFrom::Start(source.offset + within - skip as u64))?;
    let mut scratch = [0; CRYPTO_SECTOR];
    while !output.is_empty() {
        let length = if skip == 0 && output.len() >= CRYPTO_SECTOR {
            let length = output.len() / CRYPTO_SECTOR * CRYPTO_SECTOR;
            reader.read_exact(&mut output[..length])?;
            cipher.decrypt(&mut output[..length], sector);
            length
        } else {
            let length = (CRYPTO_SECTOR - skip).min(output.len());
            reader.read_exact(&mut scratch)?;
            cipher.decrypt(&mut scratch, sector);
            output[..length].copy_from_slice(&scratch[skip..skip + length]);
            length
        };
        output = &mut output[length..];
        if !output.is_empty() {
            sector += (skip + length).div_ceil(CRYPTO_SECTOR) as u64;
        }
        skip = 0;
    }
    Ok(())
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
