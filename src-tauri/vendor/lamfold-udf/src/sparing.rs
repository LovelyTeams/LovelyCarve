//! Defective-packet translation for a UDF sparable partition.
use crate::{descriptors::*, partitions::SparablePartition, udf::Udf};
use alloc::{collections::BTreeSet, vec, vec::Vec};
use lamfold::{BlockSource, FoldError, Result};

#[derive(PartialEq, Eq)]
pub(crate) struct SparingTable {
    packet_blocks: u32,
    packets: Vec<(u32, u32)>,
}

impl SparingTable {
    pub(crate) fn span(&self, position: u64, start: u32, block_size: u32) -> (u64, u64) {
        let packet_size = u64::from(self.packet_blocks) * u64::from(block_size);
        let packet = (position / packet_size) as u32 * self.packet_blocks;
        match self
            .packets
            .binary_search_by_key(&packet, |&(original, _)| original)
        {
            Ok(index) => {
                let within = position % packet_size;
                (
                    u64::from(self.packets[index].1) * u64::from(block_size) + within,
                    packet_size - within,
                )
            }
            Err(next) => {
                let limit = self.packets.get(next).map_or(u64::MAX, |&(block, _)| {
                    u64::from(block) * u64::from(block_size) - position
                });
                (u64::from(start) * u64::from(block_size) + position, limit)
            }
        }
    }
}

impl<S: BlockSource> Udf<S> {
    pub(crate) fn read_sparing_table(&mut self, map: SparablePartition) -> Result<SparingTable> {
        let packet = u32::from(map.packet_blocks);
        require(
            self.partition_start.is_multiple_of(packet)
                && self.partition_length.is_multiple_of(packet),
            "udf: sparable partition packet boundaries",
        )?;
        let mut best = None;
        let mut conflicting = false;
        let mut failure = FoldError::Corrupt("udf: no usable sparing table");
        let mut seen = BTreeSet::new();
        for &block in &map.table_blocks[..usize::from(map.table_count)] {
            require(
                seen.insert(block / packet),
                "udf: sparing copies share a packet",
            )?;
            match self.sparing_copy(block, map) {
                Ok((sequence, table)) => match &best {
                    Some((old, _)) if sequence < *old => {}
                    Some((old, previous)) if sequence == *old => conflicting |= previous != &table,
                    _ => {
                        best = Some((sequence, table));
                        conflicting = false;
                    }
                },
                Err(error) => failure = error,
            }
        }
        require(
            !conflicting,
            "udf: conflicting sparing tables at the same sequence",
        )?;
        best.map(|(_, table)| table).ok_or(failure)
    }

    fn sparing_copy(&mut self, block: u32, map: SparablePartition) -> Result<(u32, SparingTable)> {
        let packet_blocks = u32::from(map.packet_blocks);
        let block_size = u64::from(self.block_size);
        let mut bytes = vec![0; map.table_size as usize];
        exact(&mut self.src, u64::from(block) * block_size, &mut bytes)?;
        let entry_count = usize::from(u16_at(&bytes, 48)?);
        require(
            descriptor(&bytes, block, 56 + 8 * entry_count)? == 0
                && bytes.get(16..40) == Some(b"\0*UDF Sparing Table\0\0\0\0\0")
                && u16_at(&bytes, 50)? == 0,
            "udf: invalid sparing table descriptor",
        )?;
        let mut packets = Vec::new();
        let mut replacements = BTreeSet::new();
        let mut previous = None;
        for entry in bytes[56..56 + 8 * entry_count].as_chunks::<8>().0 {
            let original = u32_at(entry, 0)?;
            let replacement = u32_at(entry, 4)?;
            require(
                previous.is_none_or(|old| original >= old),
                "udf: unordered sparing table",
            )?;
            previous = Some(original);
            if original >= 0xffff_fff0 {
                continue;
            }
            require(
                original.is_multiple_of(packet_blocks)
                    && original <= self.partition_length - packet_blocks
                    && replacement.is_multiple_of(packet_blocks)
                    && (u64::from(replacement) + u64::from(packet_blocks)) * block_size
                        <= self.src.len()
                    && packets.last().is_none_or(|&(old, _)| old < original)
                    && replacements.insert(replacement),
                "udf: invalid sparing packet mapping",
            )?;
            packets.push((original, replacement));
        }
        Ok((
            u32_at(&bytes, 52)?,
            SparingTable {
                packet_blocks,
                packets,
            },
        ))
    }
}
