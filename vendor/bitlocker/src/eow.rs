//! Used-space-only Encrypt-on-Write maps. Bitmap bits describe relocation
//! blocks, not filesystem allocation; zero bits must bypass decryption.
use std::io::{Read, Seek, SeekFrom};

use crate::bytes::{le_u16, le_u32, le_u64};
use crate::{BdeError, BdeVariant, Result, VolumeHeader};

#[derive(Default)]
pub(crate) struct EowMap {
    metadata: Vec<(u64, u64)>,
    regions: Vec<Bitmap>,
}

struct Bitmap {
    offset: u64,
    size: u64,
    bytes_per_bit: u64,
    bits: Vec<u8>,
}

impl EowMap {
    pub fn is_metadata(&self, offset: u64) -> bool {
        self.metadata
            .iter()
            .any(|&(start, end)| offset >= start && offset < end)
    }

    pub fn is_unencrypted(&self, offset: u64) -> bool {
        self.regions.iter().any(|region| {
            if offset < region.offset || offset - region.offset >= region.size {
                return false;
            }
            let bit = (offset - region.offset) / region.bytes_per_bit;
            region
                .bits
                .get((bit / 8) as usize)
                .is_some_and(|byte| byte & (1 << (bit % 8)) == 0)
        })
    }

    pub fn read<R: Read + Seek>(reader: &mut R, header: &VolumeHeader, total: u64) -> Result<Self> {
        if header.variant != BdeVariant::UsedDiskSpaceOnly {
            return Ok(Self::default());
        }
        let mut last_error =
            BdeError::InvalidData("used-space-only volume has no valid EOW descriptor");
        for offset in header.eow_offsets {
            if offset == 0 {
                continue;
            }
            match Self::read_descriptor(reader, offset, header, total) {
                Ok(map) => return Ok(map),
                Err(error) => last_error = error,
            }
        }
        Err(last_error)
    }

    fn read_descriptor<R: Read + Seek>(
        reader: &mut R,
        offset: u64,
        header: &VolumeHeader,
        total: u64,
    ) -> Result<Self> {
        let descriptor = read_bounded(reader, offset, 512, total)?;
        if descriptor.get(..8) != Some(b"FVE-EOW\0") || le_u16(&descriptor, 8) != 56 {
            return Err(BdeError::InvalidData("invalid EOW descriptor"));
        }
        let physical = u64::from(le_u32(&descriptor, 16));
        let bytes_per_bit = u64::from(le_u32(&descriptor, 20));
        let count = le_u32(&descriptor, 32) as usize;
        if le_u32(&descriptor, 12) != u32::from(header.bytes_per_sector)
            || !matches!(physical, 512 | 1024 | 2048 | 4096)
            || bytes_per_bit == 0
            || bytes_per_bit % u64::from(header.bytes_per_sector) != 0
            || count == 0
            || count > 57
        {
            return Err(BdeError::InvalidData("invalid EOW geometry"));
        }
        let mut output = Self::default();
        for descriptor_offset in header.eow_offsets {
            if descriptor_offset != 0 {
                output.add_metadata(descriptor_offset, 4096, total)?;
            }
        }
        let mut bitmap_bytes = 0usize;
        for index in 0..count {
            let map_offset = le_u64(&descriptor, 56 + index * 8);
            let map = read_bounded(reader, map_offset, 64, total)?;
            if map.get(..10) != Some(b"FVE-EOWBM\0") || le_u16(&map, 10) != 64 {
                return Err(BdeError::InvalidData("invalid EOW block map"));
            }
            let map_size = u64::from(le_u32(&map, 12));
            let region_offset = le_u64(&map, 20);
            let region_size = le_u64(&map, 28);
            if region_size == 0
                || region_offset
                    .checked_add(region_size)
                    .is_none_or(|end| end > total)
                || region_offset % u64::from(header.bytes_per_sector) != 0
                || region_size % u64::from(header.bytes_per_sector) != 0
            {
                return Err(BdeError::InvalidData("invalid EOW volume region"));
            }
            if output.regions.iter().any(|region| {
                region_offset < region.offset + region.size
                    && region.offset < region_offset + region_size
            }) {
                return Err(BdeError::InvalidData("overlapping EOW volume regions"));
            }
            let record_size = le_u32(&map, 52) as usize;
            if !(36..=16 * 1024 * 1024).contains(&record_size)
                || map_size < physical + 2 * record_size as u64
            {
                return Err(BdeError::InvalidData("invalid EOW block record size"));
            }
            bitmap_bytes = bitmap_bytes
                .checked_add(record_size * 2)
                .ok_or(BdeError::InvalidData("EOW allocation overflow"))?;
            if bitmap_bytes > 64 * 1024 * 1024 {
                return Err(BdeError::InvalidData("EOW bitmap resource limit exceeded"));
            }
            output.add_metadata(
                map_offset,
                map_size
                    .checked_add(4095)
                    .ok_or(BdeError::InvalidData("EOW size overflow"))?
                    / 4096
                    * 4096,
                total,
            )?;
            let log_offset = le_u64(&map, 36);
            if log_offset != 0 {
                let size = u64::from(le_u32(&descriptor, 24)).div_ceil(4096) * 4096;
                output.add_metadata(log_offset, size, total)?;
            }
            // libbde reads the two records immediately after the physical map sector.
            let first_offset = map_offset
                .checked_add(physical)
                .ok_or(BdeError::InvalidData("EOW offset overflow"))?;
            let second_offset = first_offset
                .checked_add(record_size as u64)
                .ok_or(BdeError::InvalidData("EOW offset overflow"))?;
            let first = read_bounded(reader, first_offset, record_size, total)?;
            let second = read_bounded(reader, second_offset, record_size, total)?;
            for record in [&first, &second] {
                if record.get(..10) != Some(b"FVE-EOWBR\0") || le_u16(record, 10) != 36 {
                    return Err(BdeError::InvalidData("invalid EOW block record"));
                }
            }
            let record = match le_u32(&first, 20).cmp(&le_u32(&second, 20)) {
                std::cmp::Ordering::Greater => &first,
                std::cmp::Ordering::Less => &second,
                std::cmp::Ordering::Equal => {
                    return Err(BdeError::InvalidData("ambiguous EOW record sequence"))
                }
            };
            let bit_count = u64::from(le_u32(record, 16));
            let bitmap_size = bit_count.div_ceil(8) as usize;
            if bit_count < region_size.div_ceil(bytes_per_bit) || bitmap_size > record.len() - 36 {
                return Err(BdeError::InvalidData("truncated EOW bitmap"));
            }
            output.regions.push(Bitmap {
                offset: region_offset,
                size: region_size,
                bytes_per_bit,
                bits: record[36..36 + bitmap_size].to_vec(),
            });
        }
        Ok(output)
    }

    fn add_metadata(&mut self, offset: u64, size: u64, total: u64) -> Result<()> {
        let end = offset
            .checked_add(size)
            .filter(|end| *end <= total)
            .ok_or(BdeError::InvalidData("EOW metadata outside volume"))?;
        self.metadata.push((offset, end));
        Ok(())
    }
}

fn read_bounded<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    size: usize,
    total: u64,
) -> Result<Vec<u8>> {
    if offset
        .checked_add(size as u64)
        .is_none_or(|end| end > total)
    {
        return Err(BdeError::InvalidData("EOW read outside volume"));
    }
    let mut data = vec![0; size];
    reader.seek(SeekFrom::Start(offset))?;
    reader.read_exact(&mut data)?;
    Ok(data)
}
