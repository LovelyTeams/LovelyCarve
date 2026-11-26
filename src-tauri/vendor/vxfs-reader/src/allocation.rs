//! Decode direct and typed extent records into bounded logical file spans.

use std::collections::HashSet;

use lamfold::{BlockSource, FoldError, Result};

use crate::{Inode, ORG_DIRECT, ORG_IMMEDIATE, ORG_NONE, ORG_TYPED, Vxfs, require};

const MAX_INDIRECT_LEVELS: usize = 8;
const MAX_MAPPING_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EXTENTS: usize = 1_048_576;
const TYPED_OFFSET_MASK: u64 = (1 << 56) - 1;

#[derive(Clone, Copy)]
struct Extent {
    logical: u64,
    physical: u64,
    blocks: u64,
}

impl Extent {
    fn end(self) -> u64 {
        self.logical + self.blocks
    }
}

pub(crate) struct Allocation {
    extents: Vec<Extent>,
    indirect_bytes: u64,
    indirect_blocks: HashSet<u64>,
}

impl Allocation {
    pub(crate) fn read(
        &self,
        source: &mut impl BlockSource,
        inode: &Inode,
        block_size: u64,
        mut offset: u64,
        mut bytes: &mut [u8],
        allow_holes: bool,
    ) -> Result<()> {
        require(
            offset
                .checked_add(bytes.len() as u64)
                .is_some_and(|end| end <= inode.size),
            "VxFS read exceeds logical inode size",
        )?;
        if inode.organization == ORG_IMMEDIATE {
            let start = offset as usize;
            bytes.copy_from_slice(&inode.data[start..start + bytes.len()]);
            return Ok(());
        }
        let mut index = self
            .extents
            .partition_point(|extent| extent.end() <= offset / block_size);
        while !bytes.is_empty() {
            let next = self.extents.get(index);
            let logical = offset / block_size;
            let (count, physical) = match next {
                Some(extent) if extent.logical <= logical => {
                    let within = offset - extent.logical * block_size;
                    let count =
                        (extent.blocks * block_size - within).min(bytes.len() as u64) as usize;
                    (count, Some(extent.physical * block_size + within))
                }
                Some(extent) => (
                    (extent.logical * block_size - offset).min(bytes.len() as u64) as usize,
                    None,
                ),
                None => (bytes.len(), None),
            };
            let (part, remaining) = bytes.split_at_mut(count);
            if let Some(physical) = physical {
                source.read_at(physical, part)?;
            } else {
                require(allow_holes, "VxFS unallocated structural metadata")?;
                part.fill(0);
            }
            bytes = remaining;
            offset += count as u64;
            if next.is_some_and(|extent| offset / block_size >= extent.end()) {
                index += 1;
            }
        }
        Ok(())
    }
}

impl<S: BlockSource> Vxfs<S> {
    pub(crate) fn allocation(&mut self, inode: &Inode) -> Result<Allocation> {
        let mut result = Allocation {
            extents: Vec::new(),
            indirect_bytes: 0,
            indirect_blocks: HashSet::new(),
        };
        match inode.organization {
            ORG_NONE if inode.size == 0 => {}
            ORG_IMMEDIATE => {}
            ORG_DIRECT => self.direct_extents(&inode.data, &mut result)?,
            ORG_TYPED => self.typed_extents(&inode.data, 0, 0, &mut result)?,
            _ => return Err(FoldError::Unsupported("VxFS inode data organization")),
        }
        result.extents.sort_unstable_by_key(|extent| extent.logical);
        require(
            result
                .extents
                .windows(2)
                .all(|pair| pair[0].end() <= pair[1].logical),
            "VxFS overlapping logical extents",
        )?;
        Ok(result)
    }

    fn insert_extent(
        &self,
        output: &mut Allocation,
        logical: u64,
        physical: u64,
        blocks: u64,
    ) -> Result<()> {
        if blocks == 0 {
            return Ok(());
        }
        require(
            logical
                .checked_add(blocks)
                .is_some_and(|end| end <= u64::MAX / self.header.block_size),
            "VxFS logical extent overflow",
        )?;
        if physical == 0 {
            return Ok(());
        }
        self.check_blocks(physical, blocks)?;
        require(
            output.extents.len() < MAX_EXTENTS,
            "VxFS extent count limit",
        )?;
        output.extents.push(Extent {
            logical,
            physical,
            blocks,
        });
        Ok(())
    }

    fn direct_extents(&mut self, bytes: &[u8; 96], output: &mut Allocation) -> Result<()> {
        let endian = self.header.endian;
        let mut logical = 0;
        for item in bytes[16..].as_chunks::<8>().0 {
            let physical = u64::from(endian.u32(item, 0));
            let blocks = u64::from(endian.u32(item, 4));
            self.insert_extent(output, logical, physical, blocks)?;
            logical += blocks;
        }
        if endian.u32(bytes, 12) != 0 {
            return Err(FoldError::Unsupported("VxFS second legacy indirect table"));
        }
        let physical = u64::from(endian.u32(bytes, 8));
        if physical == 0 {
            return Ok(());
        }
        let blocks_per_entry = u64::from(endian.u32(bytes, 4));
        require(
            blocks_per_entry != 0,
            "VxFS zero-length indirect extent unit",
        )?;
        let table = self.read_blocks(physical, 1)?;
        for item in table.as_chunks::<4>().0 {
            let data = u64::from(endian.u32(item, 0));
            self.insert_extent(output, logical, data, blocks_per_entry)?;
            logical += blocks_per_entry;
        }
        Ok(())
    }

    fn typed_extents(
        &mut self,
        bytes: &[u8],
        base: u64,
        depth: usize,
        output: &mut Allocation,
    ) -> Result<()> {
        require(
            depth <= MAX_INDIRECT_LEVELS,
            "VxFS indirect extent depth limit",
        )?;
        let endian = self.header.endian;
        for item in bytes.as_chunks::<16>().0 {
            let head = endian.u64(item, 0);
            let physical = u64::from(endian.u32(item, 8));
            let blocks = u64::from(endian.u32(item, 12));
            if head == 0 && physical == 0 && blocks == 0 {
                continue;
            }
            let logical = base
                .checked_add(head & TYPED_OFFSET_MASK)
                .ok_or(FoldError::Corrupt("VxFS typed logical offset overflow"))?;
            match head >> 56 {
                1 => {
                    self.check_blocks(physical, blocks)?;
                    require(
                        blocks != 0 && physical != 0 && output.indirect_blocks.insert(physical),
                        "VxFS invalid or cyclic indirect extent",
                    )?;
                    let length = blocks * self.header.block_size;
                    output.indirect_bytes = output
                        .indirect_bytes
                        .checked_add(length)
                        .filter(|&total| total <= MAX_MAPPING_BYTES)
                        .ok_or(FoldError::Corrupt("VxFS indirect mapping work limit"))?;
                    // Read one block at a time, keeping recursion memory independent of extent length.
                    for block in physical..physical + blocks {
                        let child = self.read_blocks(block, 1)?;
                        self.typed_extents(&child, logical, depth + 1, output)?;
                    }
                }
                2 => {
                    require(
                        physical != 0 && blocks != 0,
                        "VxFS invalid allocated typed extent",
                    )?;
                    self.insert_extent(output, logical, physical, blocks)?;
                }
                3 | 4 => return Err(FoldError::Unsupported("VxFS external device extent")),
                _ => return Err(FoldError::Unsupported("VxFS unknown typed extent")),
            }
        }
        Ok(())
    }
}
