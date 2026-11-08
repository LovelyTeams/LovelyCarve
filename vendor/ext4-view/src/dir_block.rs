// Copyright 2024 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::Ext4;
use crate::block_index::FsBlockIndex;
use crate::checksum::Checksum;
use crate::error::{CorruptKind, Ext4Error};
use crate::inode::InodeIndex;
use crate::util::{read_u16le, read_u32le};

#[derive(Debug, Eq, PartialEq)]
enum DirBlockType {
    /// Root node of an htree.
    Root,

    /// Non-root internal node of an htree.
    Internal,

    /// Leaf node of an htree, also the format used for all blocks in a
    /// directory without an htree.
    Leaf,
}

/// Struct for reading and validating a directory block.
#[derive(Clone)]
pub(crate) struct DirBlock<'a> {
    pub(crate) fs: &'a Ext4,

    /// Absolute index of the block within the filesystem.
    pub(crate) block_index: FsBlockIndex,

    /// Whether this is the first block of the file.
    pub(crate) is_first: bool,

    /// Directory inode index.
    pub(crate) dir_inode: InodeIndex,

    /// Whether the directory has an htree.
    pub(crate) has_htree: bool,

    /// Checksum base copied from the dir inode.
    pub(crate) checksum_base: Checksum,
}

impl DirBlock<'_> {
    /// Read the directory block's contents into `block`.
    ///
    /// If checksums are enabled for the filesystem, the directory
    /// block's checksum will be verified.
    pub(crate) fn read(&self, block: &mut [u8]) -> Result<(), Ext4Error> {
        let block_size = self.fs.0.superblock.block_size;
        assert_eq!(block.len(), block_size);

        self.fs.read_from_block(self.block_index, 0, block)?;

        if !self.fs.has_metadata_checksums() {
            return Ok(());
        }

        let block_type = self.get_block_type(block);

        let expected_checksum = self.read_expected_checksum(block);
        let actual_checksum = if block_type == DirBlockType::Leaf {
            self.calc_leaf_checksum(block)
        } else {
            self.calc_internal_checksum(block, block_type)?
        };

        if actual_checksum.finalize() == expected_checksum {
            Ok(())
        } else {
            Err(CorruptKind::DirBlockChecksum(self.dir_inode).into())
        }
    }

    /// Get the stored checksum from the last four bytes of the block.
    fn read_expected_checksum(&self, block: &[u8]) -> u32 {
        // OK to unwrap: minimum block size is 1024.
        let offset = block.len().checked_sub(4).unwrap();

        read_u32le(block, offset)
    }

    /// Calculate the checksum of a leaf block.
    fn calc_leaf_checksum(&self, block: &[u8]) -> Checksum {
        let tail_entry_size = 12;

        // OK to unwrap: minimum block size is 1024.
        let tail_entry_offset = block.len().checked_sub(tail_entry_size).unwrap();

        let mut checksum = self.checksum_base.clone();
        checksum.update(&block[..tail_entry_offset]);

        checksum
    }

    /// Calculate the checksum of a non-leaf block.
    fn calc_internal_checksum(
        &self,
        block: &[u8],
        block_type: DirBlockType,
    ) -> Result<Checksum, Ext4Error> {
        let tail_entry_size = 8;

        // OK to unwrap: minimum block size is 1024.
        let tail_entry_offset = block.len().checked_sub(tail_entry_size).unwrap();

        let limit_offset: usize = if block_type == DirBlockType::Root {
            0x20
        } else {
            0x8
        };

        // OK to unwrap: `limit_offset` is at most 0x20.
        let count_offset = limit_offset.checked_add(2).unwrap();

        let count = read_u16le(block, count_offset);

        // OK to unwrap: `count` is at most 2^16-1, `limit_offset` is
        // at most 0x20, so the maximum result is 524,312. This fits in
        // a `u32`, and we assume that `usize` is at least that large.
        let num_bytes = limit_offset
            .checked_add(usize::from(count).checked_mul(8).unwrap())
            .unwrap();

        let mut checksum = self.checksum_base.clone();
        let bytes = block
            .get(..num_bytes)
            .filter(|_| num_bytes <= tail_entry_offset)
            .ok_or(CorruptKind::DirBlockChecksum(self.dir_inode))?;
        checksum.update(bytes);
        checksum.update_u32_le(read_u32le(block, tail_entry_offset));
        checksum.update_u32_le(0);

        Ok(checksum)
    }

    fn get_block_type(&self, block: &[u8]) -> DirBlockType {
        // Non-htree directories use the same format as leaf nodes in an htree.
        if !self.has_htree {
            return DirBlockType::Leaf;
        }

        // The first block of an htree is the root node.
        if self.is_first {
            return DirBlockType::Root;
        }

        // Other internal nodes are identified by the first record
        // having a length equal to the whole block.
        let first_rec_len = read_u16le(block, 4);
        if first_rec_len == self.fs.0.superblock.block_size
            || (self.fs.block_size() == 65536 && matches!(first_rec_len, 0 | 65535))
        {
            DirBlockType::Internal
        } else {
            DirBlockType::Leaf
        }
    }
}
