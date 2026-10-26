//! Persistent allocator layout shared by DAT and ifile; entry numbers are
//! group-relative before selecting an entry block (record sizes need not divide).
use crate::require;
use lamfold::{FoldError, Result};

pub(crate) struct Position {
    pub descriptor: u64,
    pub descriptor_offset: usize,
    pub bitmap: u64,
    pub bit: usize,
    pub entry: u64,
    pub entry_offset: usize,
}
impl Position {
    pub fn of(number: u64, block_size: u32, entry_size: usize) -> Result<Self> {
        let per_group = block_size as u64 * 8;
        let per_block = block_size as u64 / entry_size as u64;
        require(per_block != 0, "nilfs2: allocator entry size")?;
        let per_descriptor = block_size as u64 / 4;
        let blocks_per_group = per_group.div_ceil(per_block) + 1;
        let blocks_per_descriptor = per_descriptor * blocks_per_group + 1;
        let group = number / per_group;
        let local = number % per_group;
        let descriptor = (group / per_descriptor)
            .checked_mul(blocks_per_descriptor)
            .ok_or(FoldError::Corrupt("nilfs2: allocator descriptor overflow"))?;
        let bitmap = descriptor
            .checked_add(1 + (group % per_descriptor) * blocks_per_group)
            .ok_or(FoldError::Corrupt("nilfs2: allocator bitmap overflow"))?;
        let entry = bitmap
            .checked_add(1 + local / per_block)
            .ok_or(FoldError::Corrupt("nilfs2: allocator entry overflow"))?;
        Ok(Self {
            descriptor,
            descriptor_offset: (group % per_descriptor) as usize * 4,
            bitmap,
            bit: local as usize,
            entry,
            entry_offset: (local % per_block) as usize * entry_size,
        })
    }
}
