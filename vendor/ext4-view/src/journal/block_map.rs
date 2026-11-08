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
use crate::error::{CorruptKind, Ext4Error, IncompatibleKind};
use crate::inode::Inode;
use crate::iters::file_blocks::FileBlocks;
use crate::journal::block_header::{JournalBlockHeader, JournalBlockType};
use crate::journal::commit_block::validate_commit_block_checksum;
use crate::journal::descriptor_block::{
    DescriptorBlockTagIter, validate_descriptor_block_checksum,
};
use crate::journal::revocation_block::{
    read_revocation_block_table, validate_revocation_block_checksum,
};
use crate::journal::superblock::JournalSuperblock;
use crate::util::usize_from_u32;
use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;
use core::iter::Skip;

/// Map from a block somewhere in the filesystem to a block in the
/// journal. Both the key and value are absolute block indices.
pub(super) type BlockMap = BTreeMap<FsBlockIndex, FsBlockIndex>;

/// Read the block map from the journal.
pub(super) fn load_block_map(
    fs: &Ext4,
    superblock: &JournalSuperblock,
    journal_inode: &Inode,
) -> Result<BlockMap, Ext4Error> {
    let mut loader = BlockMapLoader::new(fs, superblock, journal_inode)?;

    while let Some(block_index) = loader.journal_block_iter.next() {
        loader.block_index = block_index?;

        if let Err(err) = loader.process_next() {
            if let Ext4Error::Corrupt(_) = err {
                // If a corruption error occurred, stop reading the
                // journal. Any uncommitted changes are discarded.
                break;
            } else {
                // Propagate any other type of error.
                return Err(err);
            }
        }

        // Stop reading if the end of the journal was reached.
        if loader.is_done {
            break;
        }
    }

    Ok(loader.block_map)
}

/// Private struct to help create a `BlockMap` from the journal. This is
/// essentially an iterator, although it does not implement the actual
/// iterator trait.
struct BlockMapLoader<'a> {
    fs: &'a Ext4,
    superblock: &'a JournalSuperblock,

    /// This block map is the output of the loader. When a commit block
    /// is reached, the `uncommitted_block_map` entries are moved to
    /// `block_map`.
    block_map: BlockMap,

    /// In-process updates to the block map. When a commit block is
    /// reached, the contents of this map are moved to `block_map`.
    uncommitted_block_map: BlockMap,

    /// Revoked blocks in the current transaction. When a commit block
    /// is reached, any keys in `uncommitted_block_map` that are in this
    /// revoked list will be deleted instead of committing them to
    /// `block_map`.
    revoked_blocks: Vec<FsBlockIndex>,

    /// Iterator over blocks in the journal inode. At construction, the
    /// iterator is advanced to the journal start block.
    journal_block_iter: Skip<FileBlocks>,

    /// Current block index.
    block_index: FsBlockIndex,

    /// Buffer to hold the current block's data.
    block: Vec<u8>,

    /// Buffer to hold a data block's data. This is separate from
    /// `block` because when processing a data block, we still need the
    /// associated descriptor block in memory, so the descriptor is
    /// stored in `block`.
    data_block: Vec<u8>,

    /// Current commit sequence number. The initial value comes from the
    /// superblock, and is incremented when a commit block is
    /// reached. Each journal block also contains the sequence number;
    /// it is checked against this value to make sure the journal is
    /// consistent.
    sequence: u32,

    /// If true, a block has been reached that doesn't start with the
    /// journal magic bytes, indicating the end of the journal has been
    /// reached.
    is_done: bool,
}

impl<'a> BlockMapLoader<'a> {
    fn new(
        fs: &'a Ext4,
        superblock: &'a JournalSuperblock,
        journal_inode: &Inode,
    ) -> Result<Self, Ext4Error> {
        // Get an iterator over the journal's block indices.
        let journal_block_iter = FileBlocks::new(fs.clone(), journal_inode)?;

        // Skip forward to the start block.
        let journal_block_iter = journal_block_iter.skip(usize_from_u32(superblock.start_block));

        Ok(Self {
            fs,
            superblock,
            block_map: BlockMap::new(),
            uncommitted_block_map: BlockMap::new(),
            revoked_blocks: Vec::new(),
            journal_block_iter,
            block_index: 0,
            block: vec![0; fs.0.superblock.block_size.to_usize()],
            data_block: vec![0; fs.0.superblock.block_size.to_usize()],
            sequence: superblock.sequence,
            is_done: false,
        })
    }

    /// Process the next block.
    ///
    /// Note that depending on the block type, multiple blocks may be
    /// processed.
    fn process_next(&mut self) -> Result<(), Ext4Error> {
        self.fs
            .read_from_block(self.block_index, 0, &mut self.block)?;

        let Some(header) = JournalBlockHeader::read_bytes(&self.block) else {
            // Journal block magic is not present, so we've reached
            // the end of the journal.
            self.is_done = true;
            return Ok(());
        };

        if header.sequence != self.sequence {
            return Err(CorruptKind::JournalSequence.into());
        }

        if header.block_type == JournalBlockType::DESCRIPTOR {
            self.process_descriptor_block()?;
        } else if header.block_type == JournalBlockType::REVOCATION {
            self.process_revocation_block()?;
        } else if header.block_type == JournalBlockType::COMMIT {
            self.process_commit_block()?;
        } else {
            return Err(IncompatibleKind::JournalBlockType(header.block_type.0).into());
        }
        Ok(())
    }

    /// Process a descriptor block.
    ///
    /// Each descriptor block contains an array of tags, one for each
    /// data block following the descriptor block. Each data block will
    /// replace a block within the ext4 filesystem.
    ///
    /// Note that this will skip the `journal_block_iter` past the data
    /// blocks that follow the descriptor block.
    fn process_descriptor_block(&mut self) -> Result<(), Ext4Error> {
        validate_descriptor_block_checksum(self.superblock, &self.block)?;

        let tags = DescriptorBlockTagIter::new(&self.block[JournalBlockHeader::SIZE..]);

        for tag in tags {
            let tag = tag?;

            let block_index = self
                .journal_block_iter
                .next()
                .ok_or(CorruptKind::JournalTruncated)??;

            // Check the data block checksum.
            let mut checksum = Checksum::new();
            checksum.update(self.superblock.uuid.as_bytes());
            checksum.update_u32_be(self.sequence);
            self.fs
                .read_from_block(block_index, 0, &mut self.data_block)?;
            checksum.update(&self.data_block);
            if checksum.finalize() != tag.checksum {
                return Err(CorruptKind::JournalDescriptorTagChecksum.into());
            }

            self.uncommitted_block_map
                .insert(tag.block_index, block_index);
        }

        Ok(())
    }

    fn process_revocation_block(&mut self) -> Result<(), Ext4Error> {
        validate_revocation_block_checksum(self.superblock, &self.block)?;
        read_revocation_block_table(&self.block, &mut self.revoked_blocks)
    }

    /// Process a commit block.
    ///
    /// This indicates that a group of descriptor blocks have been
    /// successfully processed. The entries in `uncommitted_block_map`
    /// are moved to `block_map`, and the sequence number is
    /// incremented.
    fn process_commit_block(&mut self) -> Result<(), Ext4Error> {
        validate_commit_block_checksum(self.superblock, &self.block)?;

        // Remove any revoked blocks from uncommitted blocks.
        for block_index in &self.revoked_blocks {
            // Don't check the `remove` return value, as a revoked block
            // wasn't necessarily reused later in the transaction.
            self.uncommitted_block_map.remove(block_index);
        }
        self.revoked_blocks.clear();

        // Commit the block map entries.
        self.block_map.extend(self.uncommitted_block_map.iter());
        self.uncommitted_block_map.clear();

        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or(CorruptKind::JournalSequenceOverflow)?;

        Ok(())
    }
}
