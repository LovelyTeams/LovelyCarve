use byteorder::{BigEndian, ReadBytesExt};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};

use crate::btree::{self, BTreeHeaderRecord};
use crate::error::{HfsPlusError, Result};
use crate::volume::{ExtentDescriptor, ForkData, VolumeHeader};

/// A reader that presents a file's data fork as a contiguous `Read + Seek` stream.
/// Translates logical file offsets to physical disk offsets through the extent map.
pub struct ForkReader<'a, R: Read + Seek> {
    reader: &'a mut R,
    logical_size: u64,
    /// Flattened list of (logical_start_byte, physical_start_byte, length_bytes)
    extent_map: Vec<(u64, u64, u64)>,
    position: u64,
}

impl<'a, R: Read + Seek> ForkReader<'a, R> {
    /// Construct a reader over an already validated, complete allocation map.
    pub fn from_extents(
        reader: &'a mut R,
        fork: &ForkData,
        block_size: u32,
        extents: &[ExtentDescriptor],
    ) -> Result<Self> {
        if block_size == 0 {
            return Err(HfsPlusError::CorruptedData(
                "zero allocation block size".into(),
            ));
        }
        let mut extent_map = Vec::new();
        let mut logical_offset = 0u64;
        for extent in extents {
            if extent.block_count == 0 {
                continue;
            }
            let physical_start = u64::from(extent.start_block) * u64::from(block_size);
            let length = u64::from(extent.block_count) * u64::from(block_size);
            extent_map.push((logical_offset, physical_start, length));
            logical_offset = logical_offset
                .checked_add(length)
                .ok_or_else(|| HfsPlusError::CorruptedData("extent capacity overflow".into()))?;
        }
        Ok(Self {
            reader,
            logical_size: fork.logical_size,
            extent_map,
            position: 0,
        })
    }

    /// Translate a logical offset to a physical offset
    fn logical_to_physical(&self, logical_offset: u64) -> Option<u64> {
        for &(log_start, phys_start, length) in &self.extent_map {
            if logical_offset >= log_start && logical_offset < log_start + length {
                return Some(phys_start + (logical_offset - log_start));
            }
        }
        None
    }
}

impl<R: Read + Seek> Read for ForkReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.position >= self.logical_size {
            return Ok(0);
        }

        let remaining = usize::try_from(self.logical_size - self.position).unwrap_or(usize::MAX);
        let to_read = buf.len().min(remaining);
        if to_read == 0 {
            return Ok(0);
        }

        let mut total_read = 0;
        while total_read < to_read {
            let logical_pos = self.position + total_read as u64;

            // Find which extent this position falls in
            let physical_pos = self.logical_to_physical(logical_pos).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "logical offset beyond extent map",
                )
            })?;

            // Calculate how many contiguous bytes we can read from this extent
            let mut extent_remaining = 0u64;
            for &(log_start, _, length) in &self.extent_map {
                if logical_pos >= log_start && logical_pos < log_start + length {
                    extent_remaining = (log_start + length) - logical_pos;
                    break;
                }
            }

            let chunk_size = ((to_read - total_read) as u64).min(extent_remaining) as usize;

            self.reader.seek(SeekFrom::Start(physical_pos))?;
            self.reader
                .read_exact(&mut buf[total_read..total_read + chunk_size])?;

            total_read += chunk_size;
        }

        self.position += total_read as u64;
        Ok(total_read)
    }
}

impl<R: Read + Seek> Seek for ForkReader<'_, R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let new_pos = match pos {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::Current(delta) => self.position.checked_add_signed(delta),
            SeekFrom::End(delta) => self.logical_size.checked_add_signed(delta),
        }
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek outside addressable range",
            )
        })?;
        self.position = new_pos;
        Ok(self.position)
    }
}

/// Fork types
pub const FORK_TYPE_DATA: u8 = 0x00;
pub const FORK_TYPE_RESOURCE: u8 = 0xFF;

/// Validate and collect a complete fork allocation map, including overflow records.
pub fn collect_extents<R: Read + Seek>(
    reader: &mut R,
    vol: &VolumeHeader,
    extents_btree: &BTreeHeaderRecord,
    fork: &ForkData,
    file_id: u32,
    fork_type: u8,
) -> Result<Vec<ExtentDescriptor>> {
    let mut result = Vec::new();
    let mut allocated = 0u32;
    append_extents(&mut result, &mut allocated, &fork.extents, vol, fork)?;
    while allocated < fork.total_blocks {
        let mut tree = extents_btree.clone();
        if file_id == 3 {
            tree.extents = result.clone();
        }
        let next = lookup_overflow_extents(reader, &tree, file_id, fork_type, allocated)?;
        let previous = allocated;
        append_extents(&mut result, &mut allocated, &next, vol, fork)?;
        if allocated == previous {
            return Err(HfsPlusError::CorruptedData(format!(
                "file {file_id} fork {fork_type:#x}: missing overflow extent at {allocated}"
            )));
        }
    }
    if fork.logical_size > u64::from(allocated) * u64::from(vol.block_size) {
        return Err(HfsPlusError::CorruptedData(format!(
            "file {file_id} declares {} bytes beyond its allocation",
            fork.logical_size
        )));
    }
    Ok(result)
}

/// Validate a record before extending a fork map. Attribute overflow uses this too.
pub(crate) fn append_extents(
    result: &mut Vec<ExtentDescriptor>,
    allocated: &mut u32,
    record: &[ExtentDescriptor],
    vol: &VolumeHeader,
    fork: &ForkData,
) -> Result<()> {
    let mut terminated = false;
    for extent in record {
        if extent.block_count == 0 {
            if extent.start_block != 0 {
                return Err(HfsPlusError::CorruptedData("nonzero empty extent".into()));
            }
            terminated = true;
            continue;
        }
        let end = extent
            .start_block
            .checked_add(extent.block_count)
            .ok_or_else(|| HfsPlusError::CorruptedData("extent block overflow".into()))?;
        if terminated || end > vol.total_blocks {
            return Err(HfsPlusError::CorruptedData(
                "extent outside volume or after terminator".into(),
            ));
        }
        *allocated = allocated
            .checked_add(extent.block_count)
            .filter(|count| *count <= fork.total_blocks)
            .ok_or_else(|| {
                HfsPlusError::CorruptedData("extent allocation exceeds total_blocks".into())
            })?;
        result.push(*extent);
    }
    Ok(())
}

/// Stream a complete data fork. Missing mappings and short source reads are errors.
pub fn read_fork_data<R: Read + Seek, W: Write>(
    reader: &mut R,
    vol: &VolumeHeader,
    extents_btree: &BTreeHeaderRecord,
    fork: &ForkData,
    file_id: u32,
    writer: &mut W,
) -> Result<u64> {
    read_fork_data_typed(
        reader,
        vol,
        extents_btree,
        fork,
        file_id,
        FORK_TYPE_DATA,
        writer,
    )
}

/// Stream a complete data or resource fork using its distinct overflow key.
pub fn read_fork_data_typed<R: Read + Seek, W: Write>(
    reader: &mut R,
    vol: &VolumeHeader,
    extents_btree: &BTreeHeaderRecord,
    fork: &ForkData,
    file_id: u32,
    fork_type: u8,
    writer: &mut W,
) -> Result<u64> {
    let extents = collect_extents(reader, vol, extents_btree, fork, file_id, fork_type)?;
    let mut fork_reader = ForkReader::from_extents(reader, fork, vol.block_size, &extents)?;
    Ok(std::io::copy(&mut fork_reader, writer)?)
}

/// Look up overflow extent records from the Extents B-tree.
/// Returns up to 8 extent descriptors.
fn lookup_overflow_extents<R: Read + Seek>(
    reader: &mut R,
    extents_btree: &BTreeHeaderRecord,
    file_id: u32,
    fork_type: u8,
    start_block: u32,
) -> Result<Vec<ExtentDescriptor>> {
    // An undecodable key fails the lookup rather than ordering `Less`, which
    // would steer the descent past the damage and report extents that exist
    // as absent. PROVISIONAL(anomaly-channel): fail now, degrade to a
    // reported miss once there is somewhere to report to.
    let comparator = move |record_data: &[u8]| -> Result<std::cmp::Ordering> {
        if record_data.len() < 12 {
            return Err(HfsPlusError::InvalidBTree(format!(
                "extent key too short: {} bytes",
                record_data.len()
            )));
        }
        // Extent key: key_length(2) + fork_type(1) + pad(1) + file_id(4) + start_block(4)
        let _key_length = u16::from_be_bytes([record_data[0], record_data[1]]);
        let rec_fork_type = record_data[2];
        let rec_file_id = u32::from_be_bytes([
            record_data[4],
            record_data[5],
            record_data[6],
            record_data[7],
        ]);
        let rec_start_block = u32::from_be_bytes([
            record_data[8],
            record_data[9],
            record_data[10],
            record_data[11],
        ]);

        match rec_file_id.cmp(&file_id) {
            std::cmp::Ordering::Equal => {}
            ord => return Ok(ord),
        }
        match rec_fork_type.cmp(&fork_type) {
            std::cmp::Ordering::Equal => {}
            ord => return Ok(ord),
        }
        Ok(rec_start_block.cmp(&start_block))
    };

    match btree::search_btree(reader, extents_btree, &comparator)? {
        Some((node, record_idx)) => {
            let record_data = node.record_data(record_idx)?;
            // Key is 12 bytes (2 key_length + 1 fork_type + 1 pad + 4 file_id + 4 start_block)
            let key_length = u16::from_be_bytes([record_data[0], record_data[1]]) as usize;
            let data_start = 2 + key_length;
            if data_start + 64 > record_data.len() {
                return Err(HfsPlusError::InvalidBTree("extent record too short".into()));
            }

            let mut cursor = Cursor::new(&record_data[data_start..]);
            let mut extents = Vec::with_capacity(8);
            for _ in 0..8 {
                let start = cursor.read_u32::<BigEndian>()?;
                let count = cursor.read_u32::<BigEndian>()?;
                extents.push(ExtentDescriptor {
                    start_block: start,
                    block_count: count,
                });
            }
            Ok(extents)
        }
        None => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {}
