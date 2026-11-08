// Copyright 2024 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::inode::Inode;
use crate::iters::file_blocks::FsBlockIndex;
use crate::util::read_u32le;
use crate::{Ext4, Ext4Error};
use alloc::vec;
use alloc::vec::Vec;

/// Block map iterator.
///
/// Block maps are how file data was stored prior to extents. Unlike
/// extents, each block of the file is stored. This makes block maps
/// much less storage efficient.
///
/// The root of the block map structure is stored directly in the
/// inode. It consists of 15 block indices. Here, as in the rest of the
/// block map structure, a block index is a `u32` containing the
/// absolute index of a block in the filesystem.
///
/// Indices `0..=11` in the root node are direct. That is, they point
/// directly to a block of file data.
///
/// Index 12 points to an indirect block. This block contains an array
/// of direct block indices. The size of the array depends on the block
/// size; a 1KiB block can store 256 indices (1024÷4).
///
/// Index 13 points to a doubly-indirect block. This block contains an
/// array of indirect block indices. Each of those indices points to a
/// block containing direct indices.
///
/// Index 14 points to a triply-indirect block. This block contains an
/// array of indirect block indices, each of which points to a block
/// that also contains indirect indices, each of which points to a block
/// containing direct indices.
///
/// Indices are only initialized up to the size of the file.
///
/// A block index of zero indicates a hole.
pub(super) struct BlockMap {
    fs: Ext4,

    /// Root of the block map. This is copied directly from the inode.
    level_0: [u32; 15],

    /// Index within `level_0`.
    level_0_index: usize,

    /// Number of blocks the iterator has yielded so far.
    num_blocks_yielded: u32,

    /// Total number of blocks in the file.
    num_blocks_total: u32,

    /// Iterators through the deeper levels of the block map.
    level_1: Option<IndirectBlockIter>,
    level_2: Option<DoubleIndirectBlockIter>,
    level_3: Option<TripleIndirectBlockIter>,

    is_done: bool,
}

impl BlockMap {
    const NUM_ENTRIES: usize = 15;

    pub(super) fn new(fs: Ext4, inode: &Inode) -> Self {
        let mut level_0 = [0; Self::NUM_ENTRIES];
        for (i, dst) in level_0.iter_mut().enumerate() {
            // OK to unwrap: `i` is at most 14, so the product is at
            // most `14*4=56`, which fits in a `usize`.
            let src_offset: usize = i.checked_mul(size_of::<u32>()).unwrap();
            *dst = read_u32le(&inode.inline_data, src_offset);
        }

        Self {
            fs,
            level_0,
            num_blocks_yielded: 0,
            num_blocks_total: inode.file_size_in_blocks(),
            level_0_index: 0,
            level_1: None,
            level_2: None,
            level_3: None,
            is_done: false,
        }
    }

    #[track_caller]
    fn increment_num_blocks_yielded(&mut self) {
        // OK to unwrap: `num_blocks_yielded` is less than
        // `num_blocks_total` (checked at the beginning of `next_impl`),
        // so adding 1 cannot fail.
        self.num_blocks_yielded = self.num_blocks_yielded.checked_add(1).unwrap();
    }

    fn next_impl(&mut self) -> Result<Option<FsBlockIndex>, Ext4Error> {
        if self.num_blocks_yielded >= self.num_blocks_total {
            self.is_done = true;
            return Ok(None);
        }

        let Some(block_0) = self.level_0.get(self.level_0_index).copied() else {
            self.is_done = true;
            return Ok(None);
        };

        let ret: u32 = if self.level_0_index <= 11 {
            // OK to unwrap: `level_0_index` is at most `11`.
            self.level_0_index = self.level_0_index.checked_add(1).unwrap();
            self.increment_num_blocks_yielded();
            block_0
        } else if self.level_0_index == 12 {
            if let Some(level_1) = &mut self.level_1 {
                if let Some(block_index) = level_1.next() {
                    self.increment_num_blocks_yielded();
                    return Ok(Some(FsBlockIndex::from(block_index)));
                } else {
                    self.level_1 = None;
                    self.level_0_index = 13;
                    return Ok(None);
                }
            } else {
                self.level_1 = Some(IndirectBlockIter::new(self.fs.clone(), block_0)?);
                return Ok(None);
            }
        } else if self.level_0_index == 13 {
            if let Some(level_2) = &mut self.level_2 {
                if let Some(block_index) = level_2.next() {
                    let block_index = block_index?;
                    self.increment_num_blocks_yielded();
                    return Ok(Some(FsBlockIndex::from(block_index)));
                } else {
                    self.level_2 = None;
                    self.level_0_index = 14;
                    return Ok(None);
                }
            } else {
                self.level_2 = Some(DoubleIndirectBlockIter::new(self.fs.clone(), block_0)?);
                return Ok(None);
            }
        } else if self.level_0_index == 14 {
            if let Some(level_3) = &mut self.level_3 {
                if let Some(block_index) = level_3.next() {
                    let block_index = block_index?;
                    self.increment_num_blocks_yielded();
                    return Ok(Some(FsBlockIndex::from(block_index)));
                } else {
                    self.level_3 = None;
                    self.level_0_index = 15;
                    return Ok(None);
                }
            } else {
                self.level_3 = Some(TripleIndirectBlockIter::new(self.fs.clone(), block_0)?);
                return Ok(None);
            }
        } else {
            todo!();
        };

        Ok(Some(FsBlockIndex::from(ret)))
    }
}

impl_result_iter!(BlockMap, FsBlockIndex);

struct IndirectBlockIter {
    /// Indirect block data. The block contains an array of `u32`, each
    /// of which is a block number.
    block: Vec<u8>,

    /// Current index within the block. This is a byte index, so each
    /// iterator step moves it forward by four (the size of a `u32`).
    index_within_block: usize,
}

impl IndirectBlockIter {
    fn new(fs: Ext4, block_index: u32) -> Result<Self, Ext4Error> {
        let mut block = vec![0u8; fs.0.superblock.block_size.to_usize()];
        if block_index != 0 {
            fs.read_from_block(FsBlockIndex::from(block_index), 0, &mut block)?;
        }

        Ok(Self {
            block,
            index_within_block: 0,
        })
    }
}

impl Iterator for IndirectBlockIter {
    /// Absolute block index.
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        if self.index_within_block >= self.block.len() {
            return None;
        }

        let block_index = read_u32le(&self.block, self.index_within_block);

        // OK to unwrap: `index_within_block` is less than `block.len()`
        // (checked above). The index is always incremented by 4, and
        // the `BlockSize` is guaranteed to be an even multiple of 4, so
        // the index is at most `block.len() - 4` at this point.
        self.index_within_block = self
            .index_within_block
            .checked_add(size_of::<u32>())
            .unwrap();

        Some(block_index)
    }
}

struct DoubleIndirectBlockIter {
    fs: Ext4,
    indirect_0: IndirectBlockIter,
    indirect_1: Option<IndirectBlockIter>,
    is_done: bool,
}

impl DoubleIndirectBlockIter {
    fn new(fs: Ext4, block_index: u32) -> Result<Self, Ext4Error> {
        Ok(Self {
            indirect_0: IndirectBlockIter::new(fs.clone(), block_index)?,
            indirect_1: None,
            fs,
            is_done: false,
        })
    }

    fn next_impl(&mut self) -> Result<Option<u32>, Ext4Error> {
        if let Some(indirect_1) = &mut self.indirect_1 {
            if let Some(block_index) = indirect_1.next() {
                Ok(Some(block_index))
            } else {
                self.indirect_1 = None;
                Ok(None)
            }
        } else if let Some(block_index) = self.indirect_0.next() {
            self.indirect_1 = Some(IndirectBlockIter::new(self.fs.clone(), block_index)?);
            Ok(None)
        } else {
            self.is_done = true;
            Ok(None)
        }
    }
}

impl_result_iter!(DoubleIndirectBlockIter, u32);

struct TripleIndirectBlockIter {
    fs: Ext4,
    indirect_0: IndirectBlockIter,
    indirect_1: Option<DoubleIndirectBlockIter>,
    is_done: bool,
}

impl TripleIndirectBlockIter {
    fn new(fs: Ext4, block_index: u32) -> Result<Self, Ext4Error> {
        Ok(Self {
            indirect_0: IndirectBlockIter::new(fs.clone(), block_index)?,
            indirect_1: None,
            fs,
            is_done: false,
        })
    }

    fn next_impl(&mut self) -> Result<Option<u32>, Ext4Error> {
        if let Some(indirect_1) = &mut self.indirect_1 {
            if let Some(block_index) = indirect_1.next() {
                let block_index = block_index?;
                Ok(Some(block_index))
            } else {
                self.indirect_1 = None;
                Ok(None)
            }
        } else if let Some(block_index) = self.indirect_0.next() {
            self.indirect_1 = Some(DoubleIndirectBlockIter::new(self.fs.clone(), block_index)?);
            Ok(None)
        } else {
            self.is_done = true;
            Ok(None)
        }
    }
}

impl_result_iter!(TripleIndirectBlockIter, u32);
