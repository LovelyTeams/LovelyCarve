//! Read-only CD-ROM MODE1/2352 projection. The evidence keeps its original
//! physical bytes; only CDFS consumers see the 2048-byte sector payloads.
use anyhow::{ensure, Context, Result};

use crate::core::evidence::partitions::SectorSource;

pub const RAW_SECTOR_BYTES: usize = 2352;
pub const DATA_BYTES: usize = 2048;
const DATA_OFFSET: usize = 16;
const SYNC: [u8; 12] = [0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0];

#[derive(Debug, Clone, Copy)]
pub struct Mode1Layout {
    physical_length: u64,
    logical_length: u64,
    first_frame: u32,
}

impl Mode1Layout {
    /// Recognize a single raw Mode 1 data track by its sector headers and a
    /// bounded ISO 9660 primary descriptor. Does not change the source view.
    pub fn probe(source: &dyn SectorSource) -> Result<Option<Self>> {
        if source.size() < DATA_OFFSET as u64 {
            return Ok(None);
        }
        let head = read_exact(source, 0, DATA_OFFSET)?;
        if head[..12] != SYNC {
            return Ok(None);
        }
        ensure!(
            head[15] == 1,
            "unsupported raw CD sector mode {}; expected MODE1/2352",
            head[15]
        );
        ensure!(
            source.size().is_multiple_of(RAW_SECTOR_BYTES as u64),
            "truncated MODE1/2352 physical sector"
        );
        let layout = Self {
            physical_length: source.size(),
            logical_length: source.size() / RAW_SECTOR_BYTES as u64 * DATA_BYTES as u64,
            first_frame: frame_number(&head)?,
        };
        ensure!(
            layout.logical_length() >= 18 * DATA_BYTES as u64,
            "raw CD track is too short for ISO 9660 descriptors"
        );
        layout.read(source, DATA_BYTES as u64, 1)?;
        for number in 16..48 {
            let offset = number * DATA_BYTES as u64;
            if offset + DATA_BYTES as u64 > layout.logical_length() {
                break;
            }
            let descriptor = layout.read(source, offset, DATA_BYTES)?;
            if &descriptor[1..6] != b"CD001" || descriptor[6] != 1 {
                return Ok(None);
            }
            match descriptor[0] {
                1 => {
                    let blocks = u32::from_le_bytes(descriptor[80..84].try_into()?);
                    let blocks_be = u32::from_be_bytes(descriptor[84..88].try_into()?);
                    let block_size = u16::from_le_bytes(descriptor[128..130].try_into()?);
                    let block_size_be = u16::from_be_bytes(descriptor[130..132].try_into()?);
                    ensure!(
                        blocks == blocks_be
                            && blocks > number as u32
                            && block_size == DATA_BYTES as u16
                            && block_size_be == block_size,
                        "invalid raw CD ISO 9660 volume geometry"
                    );
                    ensure!(
                        u64::from(blocks) * DATA_BYTES as u64 <= layout.logical_length(),
                        "ISO 9660 volume exceeds raw CD track"
                    );
                    let root = &descriptor[156..190];
                    let extent = u32::from_le_bytes(root[2..6].try_into()?);
                    let extent_be = u32::from_be_bytes(root[6..10].try_into()?);
                    let size = u32::from_le_bytes(root[10..14].try_into()?);
                    let size_be = u32::from_be_bytes(root[14..18].try_into()?);
                    ensure!(
                        root[0] >= 34
                            && root[25] & 2 != 0
                            && extent == extent_be
                            && size == size_be
                            && size > 0
                            && u64::from(extent) * DATA_BYTES as u64 + u64::from(size)
                                <= u64::from(blocks) * DATA_BYTES as u64,
                        "invalid raw CD ISO 9660 root directory bounds"
                    );
                    return Ok(Some(Self {
                        logical_length: u64::from(blocks) * DATA_BYTES as u64,
                        ..layout
                    }));
                }
                255 => return Ok(None),
                0 | 2 | 3 => {}
                _ => return Ok(None),
            }
        }
        Ok(None)
    }

    pub fn logical_length(self) -> u64 {
        self.logical_length
    }

    pub fn physical_length(self) -> u64 {
        self.physical_length
    }

    /// Read logical bytes, validating every visited physical header. EDC/ECC
    /// decoding and correction are deliberately outside this projection.
    pub fn read(self, source: &dyn SectorSource, offset: u64, length: usize) -> Result<Vec<u8>> {
        let length = (length as u64).min(self.logical_length().saturating_sub(offset)) as usize;
        let mut output = Vec::with_capacity(length);
        while output.len() < length {
            let logical = offset
                .checked_add(output.len() as u64)
                .context("CD logical offset overflow")?;
            let sector = logical / DATA_BYTES as u64;
            let within = (logical % DATA_BYTES as u64) as usize;
            let physical = sector
                .checked_mul(RAW_SECTOR_BYTES as u64)
                .context("CD physical offset overflow")?;
            let frame = read_exact(source, physical, RAW_SECTOR_BYTES)?;
            ensure!(
                frame[..12] == SYNC && frame[15] == 1,
                "invalid MODE1/2352 header at sector {sector}"
            );
            let expected = u64::from(self.first_frame)
                .checked_add(sector)
                .context("CD frame number overflow")?;
            ensure!(
                u64::from(frame_number(&frame)?) == expected,
                "raw CD address mismatch at sector {sector}"
            );
            let count = (length - output.len()).min(DATA_BYTES - within);
            output.extend_from_slice(&frame[DATA_OFFSET + within..DATA_OFFSET + within + count]);
        }
        Ok(output)
    }
}

fn frame_number(header: &[u8]) -> Result<u32> {
    let mut address = [0u32; 3];
    for (index, value) in header[12..15].iter().copied().enumerate() {
        ensure!(
            value & 15 <= 9 && value >> 4 <= 9,
            "invalid raw CD BCD address"
        );
        address[index] = u32::from(value >> 4) * 10 + u32::from(value & 15);
    }
    ensure!(
        address[1] < 60 && address[2] < 75,
        "invalid raw CD MSF address"
    );
    Ok((address[0] * 60 + address[1]) * 75 + address[2])
}

fn read_exact(source: &dyn SectorSource, offset: u64, length: usize) -> Result<Vec<u8>> {
    ensure!(
        offset
            .checked_add(length as u64)
            .is_some_and(|end| end <= source.size()),
        "raw CD read outside source"
    );
    let bytes = source.read(offset, length)?;
    ensure!(
        bytes.len() == length,
        "raw CD source short read at {offset:#x}"
    );
    Ok(bytes)
}

#[cfg(test)]
#[path = "optical_disc_tests.rs"]
mod tests;
