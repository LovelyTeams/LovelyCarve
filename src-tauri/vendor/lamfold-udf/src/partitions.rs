//! Partition-map identities and UDF 2.50/2.60 metadata address translation.
use crate::{descriptors::*, udf::Udf};
use alloc::vec::Vec;
use lamfold::{BlockSource, FoldError, Result};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Address {
    pub(crate) reference: u16,
    pub(crate) block: u32,
}

#[derive(Clone, Copy)]
pub(crate) struct MetadataGeometry {
    pub(crate) allocation_unit: u32,
    pub(crate) alignment_unit: u32,
}

#[derive(Clone, Copy)]
pub(crate) struct MetadataPartition {
    reference: u16,
    pub(crate) main: u32,
    pub(crate) mirror: u32,
    pub(crate) geometry: MetadataGeometry,
}

#[derive(Clone, Copy)]
pub(crate) struct SparablePartition {
    pub(crate) packet_blocks: u16,
    pub(crate) table_size: u32,
    pub(crate) table_blocks: [u32; 4],
    pub(crate) table_count: u8,
}

#[derive(Default)]
pub(crate) struct PartitionMaps {
    pub(crate) physical_ref: u16,
    pub(crate) sparable: Option<SparablePartition>,
    pub(crate) metadata: Option<MetadataPartition>,
    pub(crate) virtual_map: Option<(u16, u16)>,
}

impl PartitionMaps {
    pub(crate) fn parse(logical: &[u8], partition: u16) -> Result<Self> {
        let revision = domain(&logical[216..248])?;
        let map_size = u32_at(logical, 264)? as usize;
        let map_count = u32_at(logical, 268)?;
        if !matches!((map_count, map_size), (1, 6 | 64) | (2, 70 | 128)) {
            return Err(FoldError::Unsupported(
                "udf: unsupported partition map layout",
            ));
        }
        descriptor(logical, u32_at(logical, 12)?, 440 + map_size)?;
        let mut remaining = &logical[440..440 + map_size];
        let mut physical = None;
        let mut maps = Self::default();
        for reference in 0..map_count as u16 {
            match remaining {
                [1, 6, ..] if remaining.len() >= 6 && physical.is_none() => {
                    require(
                        u16_at(remaining, 2)? == 1 && u16_at(remaining, 4)? == partition,
                        "udf: physical partition map identity",
                    )?;
                    physical = Some(reference);
                    remaining = &remaining[6..];
                }
                [2, 64, 0, 0, 0, ..] if remaining.len() >= 64 => {
                    let map = &remaining[..64];
                    require(
                        u16_at(map, 36)? == 1 && u16_at(map, 38)? == partition,
                        "udf: type-2 partition map identity",
                    )?;
                    let map_revision = u16_at(map, 28)?;
                    match &map[5..28] {
                        b"*UDF Metadata Partition" if maps.metadata.is_none() => {
                            require(
                                revision >= 0x0250 && matches!(map_revision, 0x0250 | 0x0260),
                                "udf: metadata partition revision",
                            )?;
                            maps.metadata = Some(parse_metadata_map(map, reference)?);
                        }
                        b"*UDF Sparable Partition" if physical.is_none() => {
                            require(revision >= 0x0150, "udf: sparable partition revision")?;
                            maps.sparable = Some(parse_sparable_map(map)?);
                            physical = Some(reference);
                        }
                        b"*UDF Virtual Partition\0" if maps.virtual_map.is_none() => {
                            require(
                                revision >= 0x0150
                                    && matches!(
                                        map_revision,
                                        0x0150 | 0x0200 | 0x0201 | 0x0250 | 0x0260
                                    ),
                                "udf: virtual partition revision",
                            )?;
                            maps.virtual_map = Some((reference, map_revision));
                        }
                        _ => {
                            return Err(FoldError::Unsupported(
                                "udf: unsupported partition map type",
                            ))
                        }
                    }
                    remaining = &remaining[64..];
                }
                _ => {
                    return Err(FoldError::Unsupported(
                        "udf: unsupported partition map type",
                    ))
                }
            }
        }
        maps.physical_ref = physical.ok_or(FoldError::Unsupported(
            "udf: missing physical or sparable partition map",
        ))?;
        require(
            remaining.is_empty()
                && !(maps.virtual_map.is_some()
                    && (maps.metadata.is_some() || maps.sparable.is_some())),
            "udf: incompatible or trailing partition maps",
        )?;
        Ok(maps)
    }

    pub(crate) fn tree_ref(&self) -> u16 {
        if let Some(metadata) = self.metadata {
            metadata.reference
        } else if let Some((reference, _)) = self.virtual_map {
            reference
        } else {
            self.physical_ref
        }
    }

    pub(crate) fn tree_address(&self, block: u32) -> Address {
        Address {
            reference: self.tree_ref(),
            block,
        }
    }
}

fn parse_metadata_map(map: &[u8], reference: u16) -> Result<MetadataPartition> {
    let geometry = MetadataGeometry {
        allocation_unit: u32_at(map, 52)?,
        alignment_unit: u32::from(u16_at(map, 56)?),
    };
    let main = u32_at(map, 40)?;
    let mirror = u32_at(map, 44)?;
    require(
        geometry.alignment_unit != 0
            && geometry.allocation_unit != 0
            && map[58] & !1 == 0
            && map[59..64] == [0; 5]
            && main != mirror,
        "udf: invalid metadata map geometry or flags",
    )?;
    Ok(MetadataPartition {
        reference,
        main,
        mirror,
        geometry,
    })
}

fn parse_sparable_map(map: &[u8]) -> Result<SparablePartition> {
    let packet_blocks = u16_at(map, 40)?;
    let table_count = map[42];
    let table_size = u32_at(map, 44)?;
    require(
        packet_blocks.is_power_of_two()
            && (1..=4).contains(&table_count)
            && map[43] == 0
            && (56..=u16::MAX as u32 + 16).contains(&table_size),
        "udf: invalid sparable map geometry",
    )?;
    let mut table_blocks = [0; 4];
    for (index, block) in table_blocks[..usize::from(table_count)]
        .iter_mut()
        .enumerate()
    {
        *block = u32_at(map, 48 + 4 * index)?;
    }
    Ok(SparablePartition {
        packet_blocks,
        table_size,
        table_blocks,
        table_count,
    })
}

pub(crate) struct MetadataMap {
    extents: Vec<MetadataExtent>,
}

struct MetadataExtent {
    logical_start: u64,
    logical_end: u64,
    backing_start: Option<u64>,
}

impl MetadataMap {
    fn span(&self, position: u64) -> Result<(u64, u64)> {
        let index = self
            .extents
            .partition_point(|extent| extent.logical_end <= position);
        let extent = self
            .extents
            .get(index)
            .ok_or(FoldError::Corrupt("udf: extent outside metadata partition"))?;
        let start = extent.backing_start.ok_or(FoldError::Corrupt(
            "udf: reference to an unallocated metadata block",
        ))?;
        Ok((
            start + position - extent.logical_start,
            extent.logical_end - position,
        ))
    }
}

impl<S: BlockSource> Udf<S> {
    pub(crate) fn open_metadata(
        &mut self,
        block: u32,
        file_type: u8,
        geometry: MetadataGeometry,
    ) -> Result<MetadataMap> {
        let address = Address {
            block,
            reference: self.maps.physical_ref,
        };
        let mut logical_start = 0;
        let mut extents = Vec::new();
        for extent in self.metadata_extents(address, file_type, geometry)? {
            let length = u64::from(extent.length);
            let backing_start = extent
                .address
                .map(|address| u64::from(address.block) * u64::from(self.block_size));
            let logical_end = logical_start + length;
            extents.push(MetadataExtent {
                logical_start,
                logical_end,
                backing_start,
            });
            logical_start = logical_end;
        }
        require(
            logical_start / u64::from(self.block_size) <= u64::from(u32::MAX) + 1,
            "udf: metadata partition exceeds address space",
        )?;
        Ok(MetadataMap { extents })
    }

    pub(crate) fn check_reference(&self, reference: u16) -> Result<()> {
        require(
            reference == self.maps.physical_ref || reference == self.maps.tree_ref(),
            "udf: invalid partition map reference",
        )
    }

    fn partition_span(&self, address: Address, offset: u64) -> Result<(u64, u64)> {
        self.check_reference(address.reference)?;
        let block_size = u64::from(self.block_size);
        let position = (u64::from(address.block) * block_size)
            .checked_add(offset)
            .ok_or(FoldError::Corrupt("udf: partition offset overflow"))?;
        if address.reference == self.maps.physical_ref {
            return self.backing_span(position);
        }
        let (backing, available) = if self.maps.virtual_map.is_some() {
            self.virtual_map
                .as_ref()
                .ok_or(FoldError::Corrupt("udf: VAT unavailable"))?
                .span(position, self.block_size)?
        } else {
            self.metadata_map
                .as_ref()
                .ok_or(FoldError::Corrupt(
                    "udf: metadata partition map unavailable",
                ))?
                .span(position)?
        };
        let (absolute, packet_available) = self.backing_span(backing)?;
        Ok((absolute, available.min(packet_available)))
    }

    fn backing_span(&self, position: u64) -> Result<(u64, u64)> {
        let block_size = u64::from(self.block_size);
        let length = u64::from(self.partition_length) * block_size;
        require(position < length, "udf: extent outside physical partition")?;
        let (absolute, available) = match &self.sparing_table {
            Some(table) => table.span(position, self.partition_start, self.block_size),
            None => (
                u64::from(self.partition_start) * block_size + position,
                length - position,
            ),
        };
        Ok((absolute, available.min(length - position)))
    }

    pub(crate) fn check_partition(&self, address: Address, length: u64) -> Result<()> {
        let mut offset = 0;
        while offset < length {
            let (_, available) = self.partition_span(address, offset)?;
            offset += available.min(length - offset);
        }
        Ok(())
    }

    pub(crate) fn read_partition(
        &mut self,
        address: Address,
        offset: u64,
        mut bytes: &mut [u8],
    ) -> Result<()> {
        let mut offset = offset;
        while !bytes.is_empty() {
            let (absolute, available) = self.partition_span(address, offset)?;
            let count = available.min(bytes.len() as u64) as usize;
            let (chunk, rest) = bytes.split_at_mut(count);
            exact(&mut self.src, absolute, chunk)?;
            bytes = rest;
            offset += count as u64;
        }
        Ok(())
    }
}
