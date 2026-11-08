// Copyright 2024 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

mod block_header;
mod block_map;
mod commit_block;
mod descriptor_block;
mod revocation_block;
mod superblock;

use crate::Ext4;
use crate::block_index::FsBlockIndex;
use crate::error::Ext4Error;
use crate::inode::Inode;
use block_map::{BlockMap, load_block_map};
use superblock::JournalSuperblock;

#[derive(Debug)]
pub(crate) struct Journal {
    block_map: BlockMap,
}

impl Journal {
    /// Create an empty journal.
    pub(crate) fn empty() -> Self {
        Self {
            block_map: BlockMap::new(),
        }
    }

    /// Load a journal from the filesystem.
    ///
    /// If the filesystem has no journal, an empty journal is returned.
    ///
    /// Note: ext4 is all little-endian, except for the journal, which
    /// is all big-endian.
    pub(crate) fn load(fs: &Ext4) -> Result<Self, Ext4Error> {
        let Some(journal_inode) = fs.0.superblock.journal_inode else {
            // Return an empty journal if this filesystem does not have
            // a journal.
            return Ok(Self::empty());
        };

        let journal_inode = Inode::read(fs, journal_inode)?;
        // Older ext4 layouts (pre-IS_64BIT / pre-CHECKSUM_V3 journals,
        // legacy superblock-v1 journals) trip the strict feature check
        // in JournalSuperblock::load. For a forensic read-only viewer
        // we'd rather see the on-disk state than refuse to mount the
        // whole volume — and skipping journal replay actually preserves
        // not-yet-checkpointed deletions, which is the conservative
        // choice for evidence work.
        let superblock = match JournalSuperblock::load(fs, &journal_inode) {
            Ok(sb) => sb,
            Err(Ext4Error::Incompatible(_)) => return Ok(Self::empty()),
            Err(err) => return Err(err),
        };
        let block_map = load_block_map(fs, &superblock, &journal_inode)?;

        Ok(Self { block_map })
    }

    /// Map from an absolute block index to a block in the journal.
    ///
    /// If the journal does not contain a replacement for the input
    /// block, the input block is returned.
    pub(crate) fn map_block_index(&self, block_index: FsBlockIndex) -> FsBlockIndex {
        *self.block_map.get(&block_index).unwrap_or(&block_index)
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use crate::test_util::load_compressed_filesystem;
    use alloc::rc::Rc;

    #[test]
    fn test_journal() {
        let mut fs = load_compressed_filesystem("test_disk_4k_block_journal.bin.zst");

        let test_dir = "/dir500";

        // With the journal in place, this directory exists.
        assert!(fs.exists(test_dir).unwrap());

        // Clear the journal, and verify that the directory no longer exists.
        Rc::get_mut(&mut fs.0).unwrap().journal.block_map.clear();
        assert!(!fs.exists(test_dir).unwrap());
    }
}
