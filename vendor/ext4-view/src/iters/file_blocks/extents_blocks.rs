// Copyright 2024 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::Ext4;
use crate::block_index::{FileBlockIndex, FsBlockIndex};
use crate::error::{CorruptKind, Ext4Error};
use crate::extent::Extent;
use crate::inode::{Inode, InodeIndex};
use crate::iters::extents::Extents;

/// Iterator over blocks in a file that uses extents.
///
/// The iterator produces absolute block indices. A block index of zero
/// indicates a hole.
pub(super) struct ExtentsBlocks {
    /// Extent iterator.
    extents: Extents,

    /// Current extent, or `None` if a new extent needs to be fetched
    /// from the iterator.
    extent: Option<Extent>,

    /// Current block index within the extent.
    block_within_extent: u16,

    /// If in a hole, number of blocks remaining in that hole. If not in
    /// a hole, this field is zero.
    blocks_remaining_in_hole: u32,

    /// Current block within the file. This is relative to the file, not
    /// an absolute block index.
    block_within_file: FileBlockIndex,

    /// Total number of blocks in the file.
    num_blocks_total: u32,

    /// Whether the iterator is done (all calls to `next` will return `None`).
    is_done: bool,

    /// Just used for errors.
    inode: InodeIndex,
}

impl ExtentsBlocks {
    pub(super) fn new(fs: Ext4, inode: &Inode) -> Result<Self, Ext4Error> {
        let num_blocks_total = inode.file_size_in_blocks();

        Ok(Self {
            extents: Extents::new(fs, inode)?,
            extent: None,
            blocks_remaining_in_hole: 0,
            block_within_file: 0,
            num_blocks_total,
            block_within_extent: 0,
            is_done: false,
            inode: inode.index,
        })
    }

    fn next_impl(&mut self) -> Result<Option<FsBlockIndex>, Ext4Error> {
        if self.block_within_file >= self.num_blocks_total {
            self.is_done = true;
            return Ok(None);
        }

        // If in a hole, yield zero.
        if self.blocks_remaining_in_hole > 0 {
            // OK to unwrap: just checked that
            // `blocks_remaining_in_hole` is greater than zero.
            self.blocks_remaining_in_hole = self.blocks_remaining_in_hole.checked_sub(1).unwrap();
            // OK to unwrap: `block_within_file` is less than
            // `num_blocks_total` (checked at the beginning of this
            // function), so adding 1 cannot fail.
            self.block_within_file = self.block_within_file.checked_add(1).unwrap();
            return Ok(Some(0));
        }

        // Get the extent, or get the next one if not set.
        let extent = if let Some(extent) = &self.extent {
            extent
        } else {
            match self.extents.next() {
                Some(Ok(extent)) => {
                    // If there is a hole between the current block in
                    // the file and the start of this extent, get the
                    // size of that hole.
                    if extent.block_within_file > self.block_within_file {
                        // OK to unwrap: just checked that
                        // `block_within_file` is greater than
                        // `block_within_file`.
                        self.blocks_remaining_in_hole = extent
                            .block_within_file
                            .checked_sub(self.block_within_file)
                            .unwrap();
                    }

                    self.extent = Some(extent);
                    self.block_within_extent = 0;

                    // If there is a hole, return early so that the hole
                    // is processed before the extent.
                    if self.blocks_remaining_in_hole > 0 {
                        return Ok(None);
                    }

                    // OK to unwrap since we just set it.
                    self.extent.as_ref().unwrap()
                }
                Some(Err(err)) => return Err(err),
                None => {
                    // Get the number of blocks remaining in the file.
                    //
                    // OK to unwrap: `block_within_file` is less than
                    // `num_blocks_total` (checked at the beginning of
                    // this function).
                    let blocks_remaining = self
                        .num_blocks_total
                        .checked_sub(self.block_within_file)
                        .unwrap();

                    // If the final extent does not cover the end of the
                    // file, then the file ends in a hole. Return early
                    // to process the hole.
                    if blocks_remaining > 0 {
                        self.blocks_remaining_in_hole = blocks_remaining;
                        return Ok(None);
                    }

                    self.is_done = true;
                    return Ok(None);
                }
            }
        };

        // If all blocks in the extent have been processed, move to the
        // next extent on the next iteration.
        if self.block_within_extent == extent.num_blocks {
            self.extent = None;
            return Ok(None);
        }

        let block = extent
            .start_block
            .checked_add(u64::from(self.block_within_extent))
            .ok_or(CorruptKind::ExtentBlock(self.inode))?;

        // OK to unwrap: `block_within_extent` is less than `num_blocks`
        // (checked above) so adding `1` cannot fail.
        self.block_within_extent = self.block_within_extent.checked_add(1).unwrap();

        // OK to unwrap: `block_within_file` is less than
        // `num_blocks_total` (checked at the beginning of this
        // function), so adding 1 cannot fail.
        self.block_within_file = self.block_within_file.checked_add(1).unwrap();

        Ok(Some(if extent.unwritten { 0 } else { block }))
    }
}

// In pseudocode, here's what the iterator is doing:
//
// if hole before first extent {
//   yield 0 for each block in hole;
// }
//
// for extent in extents(inode) {
//   if hole between this extent and previous {
//     yield 0 for each block in hole;
//   }
//
//   for block in extent.blocks {
//     yield block;
//   }
// }
//
// if hole after last extent {
//   yield 0 for each block in hole;
// }
impl_result_iter!(ExtentsBlocks, FsBlockIndex);

#[cfg(feature = "std")]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::load_test_disk1;
    use crate::{FollowSymlinks, Path};

    /// Test that `ExtentsBlocks` yields zero for holes.
    ///
    /// This only checks hole vs not-hole, since the specific block
    /// indices will change if test data is regenerated.
    #[test]
    fn test_extents_blocks_with_hole() {
        let fs = load_test_disk1();

        let inode = fs
            .path_to_inode(Path::new("/holes"), FollowSymlinks::All)
            .unwrap();

        // This vec contains one boolean (hole vs not-hole) for each
        // block in the file.
        let is_hole: Vec<_> = ExtentsBlocks::new(fs, &inode)
            .unwrap()
            .map(|block_index| {
                let block_index = block_index.unwrap();
                block_index == 0
            })
            .collect();

        let expected_is_hole = [
            true, true, false, false, true, true, false, false, true, true,
        ];

        assert_eq!(is_hole, expected_is_hole);
    }
}
