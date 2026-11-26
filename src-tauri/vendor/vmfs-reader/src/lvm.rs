//! Storage-member directories use VMware's LVM extent map, separately from the filesystem.
use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};

use crate::{Error, Result, le32, le64, name, read_exact_at};

const HEADER_OFFSET: u64 = 0x100000;
const MAGIC: u32 = 0xc001d00d;
const VOLUME_TABLE_AREA: u64 = 512 * 1024;
const EXTENTS_PER_MAP: u32 = 8192;
const EXTENT_RECORD_SIZE: usize = 128;
const MAX_MAPS_PER_REGION: u32 = 8;

#[derive(Debug, Clone)]
pub struct MemberInfo {
    pub version: u32,
    pub member_uuid: [u8; 16],
    pub volume_uuid: [u8; 16],
    pub snapshot_id: u32,
    pub volume_generation: u64,
    pub member_count: u32,
    pub volume_size: u64,
    pub label: String,
    pub first_extent: u64,
    pub last_extent: u64,
}

pub fn probe_member<R: Read + Seek>(reader: &mut R) -> Result<Option<MemberInfo>> {
    Ok(Member::read(reader)?.map(|member| member.info))
}

pub(crate) struct Member {
    pub info: MemberInfo,
    length: u64,
    alignment: u64,
    data_start: u64,
    extent_count: u32,
    volume_slot: u32,
    regions: Vec<MapRegion>,
}

struct MapRegion {
    header: u64,
    data_start: u64,
    maps: u32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Extent {
    pub logical: u64,
    pub physical: u64,
    pub length: u64,
    pub source: usize,
}

impl Member {
    pub(crate) fn read<R: Read + Seek>(reader: &mut R) -> Result<Option<Self>> {
        let source_length = reader.seek(SeekFrom::End(0))?;
        if source_length < HEADER_OFFSET + 512 {
            return Ok(None);
        }
        let mut header = [0; 512];
        read_exact_at(reader, HEADER_OFFSET, &mut header)?;
        if le32(&header, 0) != MAGIC {
            return Ok(None);
        }
        let version = le32(&header, 4);
        let alignment = match version {
            5 => 512,
            6 => u64::from(le32(&header, 0xca)),
            _ => return Err(Error::Unsupported("VMware LVM version")),
        };
        let sector_size = u64::from(le32(&header, 0x5a));
        let length = le64(&header, 0x5e);
        let data_start = le64(&header, 0x7a);
        if !(512..=65536).contains(&alignment)
            || !alignment.is_power_of_two()
            || !matches!(sector_size, 512 | 4096)
            || length > source_length
            || data_start >= length
            || !data_start.is_multiple_of(sector_size)
        {
            return Err(Error::Corrupt("invalid member geometry"));
        }
        if le32(&header, 0x66) != 1 {
            return Err(Error::Unsupported("expected one logical volume per member"));
        }
        let mut table = [0; 512];
        read_exact_at(reader, HEADER_OFFSET + alignment, &mut table)?;
        let volume_size = le64(&table, 0);
        let extent_count = le32(&header, 0x6a);
        let member_count = le32(&table, 0x90);
        let first_extent = le64(&table, 0x78);
        let last_extent = le64(&table, 0x80);
        let volume_uuid = table[0x54..0x64].try_into().unwrap();
        let member_uuid = header[0x82..0x92].try_into().unwrap();
        if volume_size == 0
            || !volume_size.is_multiple_of(sector_size)
            || !matches!(member_count, 1..=32)
            || extent_count == 0
            || last_extent < first_extent
            || member_uuid == [0; 16]
            || volume_uuid == [0; 16]
        {
            return Err(Error::Corrupt("invalid volume descriptor"));
        }
        if le32(&table, 0x10) != 1 {
            return Err(Error::Unsupported(
                "logical volume is outside its normal state",
            ));
        }
        let mut regions = vec![MapRegion {
            header: HEADER_OFFSET,
            data_start,
            maps: le32(&header, 0xbe).max(1),
        }];
        let mut reached = HashSet::from([HEADER_OFFSET]);
        let mut next = le64(&header, 0xc2);
        while next != 0 {
            if !reached.insert(next) || regions.len() == 256 {
                return Err(Error::Corrupt("cyclic or excessive VMware LVM metadata"));
            }
            let mut extended = [0; 24];
            if next
                .checked_add(extended.len() as u64)
                .is_none_or(|end| end > length)
            {
                return Err(Error::Corrupt("extended metadata is outside member"));
            }
            read_exact_at(reader, next, &mut extended)?;
            if le32(&extended, 0) != MAGIC {
                return Err(Error::Corrupt("extended metadata signature"));
            }
            regions.push(MapRegion {
                header: next,
                maps: le32(&extended, 4),
                data_start: le64(&extended, 8),
            });
            next = le64(&extended, 16);
        }
        if regions.iter().any(|region| {
            !(1..=MAX_MAPS_PER_REGION).contains(&region.maps) || region.data_start >= length
        }) || regions.iter().map(|region| region.maps).sum::<u32>() * EXTENTS_PER_MAP
            < extent_count
        {
            return Err(Error::Corrupt("invalid extent table count"));
        }
        Ok(Some(Self {
            info: MemberInfo {
                version,
                member_uuid,
                volume_uuid,
                snapshot_id: le32(&table, 0x64),
                volume_generation: le64(&table, 8),
                member_count,
                volume_size,
                label: name(&table[0x14..0x54]),
                first_extent,
                last_extent,
            },
            length,
            alignment,
            data_start,
            extent_count,
            volume_slot: le32(&table, 0x70),
            regions,
        }))
    }

    pub(crate) fn extents<R: Read + Seek>(
        &self,
        reader: &mut R,
        source: usize,
    ) -> Result<Vec<Extent>> {
        let bitmap_size = self.alignment.max(u64::from(EXTENTS_PER_MAP / 8));
        let map_size = bitmap_size + u64::from(EXTENTS_PER_MAP) * EXTENT_RECORD_SIZE as u64;
        let mut remaining = self.extent_count;
        let mut extents = Vec::new();
        let mut bitmap = vec![0; bitmap_size as usize];
        for region in &self.regions {
            for map in 0..region.maps {
                let count = remaining.min(EXTENTS_PER_MAP);
                if count == 0 {
                    break;
                }
                let offset = region
                    .header
                    .checked_add(self.alignment + VOLUME_TABLE_AREA + u64::from(map) * map_size)
                    .ok_or(Error::Corrupt("extent table offset overflow"))?;
                let table_offset = offset
                    .checked_add(bitmap_size)
                    .ok_or(Error::Corrupt("extent table offset overflow"))?;
                let table_length = count as usize * EXTENT_RECORD_SIZE;
                if table_offset
                    .checked_add(table_length as u64)
                    .is_none_or(|end| end > self.length)
                {
                    return Err(Error::Corrupt("extent table exceeds member"));
                }
                read_exact_at(reader, offset, &mut bitmap)?;
                let mut table = vec![0; table_length];
                reader.read_exact(&mut table)?;
                for (index, record) in table.as_chunks::<EXTENT_RECORD_SIZE>().0.iter().enumerate()
                {
                    let allocated = bitmap[index / 8] & (1 << (index % 8)) != 0;
                    if record[0] != u8::from(allocated) {
                        return Err(Error::Corrupt(
                            "extent table disagrees with its allocation bitmap",
                        ));
                    }
                    if !allocated || le32(record, 5) != self.volume_slot {
                        continue;
                    }
                    let physical = le64(record, 9);
                    let logical = le64(record, 17);
                    let length = le64(record, 25);
                    if physical < self.data_start
                        || length == 0
                        || physical
                            .checked_add(length)
                            .is_none_or(|end| end > self.length)
                        || logical
                            .checked_add(length)
                            .is_none_or(|end| end > self.info.volume_size)
                        || ![physical, logical, length]
                            .iter()
                            .all(|value| value.is_multiple_of(512))
                        || le32(record, 33) != 1
                    {
                        return Err(Error::Corrupt("invalid logical extent"));
                    }
                    extents.push(Extent {
                        logical,
                        physical,
                        length,
                        source,
                    });
                }
                remaining -= count;
            }
        }
        let mut physical = extents.clone();
        physical.sort_by_key(|extent| extent.physical);
        if physical
            .windows(2)
            .any(|pair| pair[0].physical + pair[0].length > pair[1].physical)
        {
            return Err(Error::Corrupt("overlapping member extents"));
        }
        Ok(extents)
    }
}
