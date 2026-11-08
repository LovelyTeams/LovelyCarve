// Copyright 2025 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::block_index::FsBlockIndex;
use crate::block_size::BlockSize;
use crate::error::CorruptKind;
use crate::error::Ext4Error;
use crate::util::usize_from_u32;
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec;

/// Entry for a single block in the cache.
#[derive(Clone)]
struct CacheEntry {
    /// Absolute block index within the filesystem.
    block_index: FsBlockIndex,

    /// Block data. The length is always equal to the filesystem block size.
    data: Box<[u8]>,
}

/// LRU block cache.
///
/// This is a fairly simple cache that holds a fixed number of blocks in
/// a deque. The front of the deque is for most-recently accessed
/// blocks, the back for least-recently accessed.
///
/// When a block in the cache is accessed, it's moved to the front of
/// the cache, and new blocks are also added directly to the front.
///
/// When new blocks are added, an equal number of blocks are popped off
/// the back. At the end of insertion, the total number of cache entries
/// remains unchanged. The block allocations within each entry are
/// reused, so allocation only occurs when initializing the cache.
///
/// Blocks are read in a group. Depending on the underlying data source,
/// this can be much more efficient than reading one by one.
///
/// The number of entries in the cache, and the size of the read buffer,
/// are controlled by the block size. The intent is to strike a
/// reasonable balance between speed and memory usage.
pub(crate) struct BlockCache {
    /// Contiguous buffer of multiple blocks.
    ///
    /// Depending on the underlying data source, it can be much more
    /// efficient to do a single read of N blocks, instead of N reads
    /// each one block in length. And it's a good bet that if we read
    /// block X, we'll soon need blocks X+1, X+2, etc.
    ///
    /// Immediately after blocks are read into this buffer, they are individually
    /// copied to an entry in `entries`.
    read_buf: Box<[u8]>,

    /// Maximum number of blocks that can be read into `read_buf`. The
    /// length of `read_buf` is `max_blocks_per_read * block_size`.
    max_blocks_per_read: u32,

    /// Cache entries, sorted from most-recently-used to least.
    ///
    /// The entries are fully allocated when the cache is
    /// created. During regular operation no additional allocation or
    /// deallocation occurs, data is just copied around.
    entries: VecDeque<CacheEntry>,

    /// File system block size.
    block_size: BlockSize,

    /// Total number of blocks in the filesystem.
    ///
    /// This is used to ensure that when reading multiple blocks we
    /// don't go past the end of the filesystem.
    num_fs_blocks: u64,
}

impl BlockCache {
    /// Create a block cache with sensible defaults.
    pub(crate) fn new(block_size: BlockSize, num_fs_blocks: u64) -> Result<Self, Ext4Error> {
        Self::with_opts(CacheOpts::new(block_size), num_fs_blocks)
    }

    /// Create a block cache with control over the number of entries and
    /// the read size.
    ///
    /// # Preconditions
    ///
    /// `max_blocks_per_read` must be less than or equal to `num_entries`.
    fn with_opts(opts: CacheOpts, num_fs_blocks: u64) -> Result<Self, Ext4Error> {
        assert!(usize_from_u32(opts.max_blocks_per_read) <= opts.num_entries);

        let read_buf_len = opts.read_buf_size_in_bytes();

        let entries = vec![
            CacheEntry {
                block_index: 0,
                data: vec![0; opts.block_size.to_usize()].into_boxed_slice(),
            };
            opts.num_entries
        ];
        Ok(Self {
            entries: VecDeque::from(entries),
            max_blocks_per_read: opts.max_blocks_per_read,
            read_buf: vec![0; read_buf_len].into_boxed_slice(),
            block_size: opts.block_size,
            num_fs_blocks,
        })
    }

    /// Get the number of blocks to read.
    ///
    /// Normally this returns `max_blocks_per_read`. If reading that
    /// many blocks would go past the end of the filesystem, the number
    /// is clamped to avoid that.
    ///
    /// # Preconditions
    ///
    /// `block_index` must be less than `num_fs_blocks`.
    fn num_blocks_to_read(&self, block_index: FsBlockIndex) -> u32 {
        assert!(block_index < self.num_fs_blocks);

        // Get the index of the block right after the last block to read.
        let end_block = block_index
            .saturating_add(u64::from(self.max_blocks_per_read))
            .min(self.num_fs_blocks);

        // OK to unwrap: `end_block` can't be less than `block_index`.
        let num_blocks = end_block.checked_sub(block_index).unwrap();

        // OK to unwrap: the number is at most `max_blocks_per_read`,
        // which is a `u32`.
        u32::try_from(num_blocks).unwrap()
    }

    /// Get the cache entry for `block_index`, reading and inserting
    /// blocks into the cache if not already present.
    ///
    /// If the entry is already present, it is moved to the front of the
    /// cache to indicate it was accessed most recently.
    ///
    /// Otherwise, `f` is called to read a contiguous group of
    /// blocks. Each block is inserted into the cache, with the
    /// requested `block_index` at the front of the cache. `f` is called
    /// only once.
    ///
    /// # Preconditions
    ///
    /// `block_index` must be less than `num_fs_blocks`.
    pub(crate) fn get_or_insert_blocks<F>(
        &mut self,
        block_index: FsBlockIndex,
        f: F,
    ) -> Result<&[u8], Ext4Error>
    where
        F: FnOnce(&mut [u8]) -> Result<(), Ext4Error>,
    {
        assert!(block_index < self.num_fs_blocks);

        // Check if the block is already cached.
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.block_index == block_index)
        {
            // Move the entry to the front of the cache if it's not
            // already there.
            if index != 0 {
                let entry = self.entries.remove(index).unwrap();
                self.entries.push_front(entry);
            }

            // Return the cached block data.
            return Ok(&*self.entries[0].data);
        }

        // Get the number of blocks/bytes to read.
        let num_blocks = self.num_blocks_to_read(block_index);
        let num_bytes = usize_from_u32(num_blocks)
            .checked_mul(self.block_size.to_usize())
            .ok_or(CorruptKind::BlockCacheReadTooLarge {
                num_blocks,
                block_size: self.block_size,
            })?;

        // Read blocks into the read buffer.
        f(&mut self.read_buf[..num_bytes])?;

        // Add blocks to the cache. Blocks are added to the front in
        // reverse order, so that the requested `block_index` is at the
        // very front of the cache.
        for i in (0..num_blocks).rev() {
            // OK to unwrap: function precondition requires that the
            // requested blocks are valid (i.e. within the filesystem),
            // Valid block indices fit in a `u64`, so this can't
            // overflow.
            let block_index = block_index.checked_add(u64::from(i)).unwrap();

            self.insert_block(block_index, i);
        }

        // Get the requested block data, which should be at the front of
        // the cache now.
        let entry = &self.entries[0];
        assert_eq!(entry.block_index, block_index);
        Ok(&*entry.data)
    }

    /// Add a block to the front of the cache. The block data is read
    /// from the `read_buf` at an offset of `block_within_read_buf *
    /// block_size`.
    ///
    /// # Preconditions
    ///
    /// `block_within_read_buf` must be a valid block index within the
    /// read buf.
    fn insert_block(&mut self, block_index: FsBlockIndex, block_within_read_buf: u32) {
        assert!(block_within_read_buf < self.max_blocks_per_read);

        // OK to unwrap: precondition says that `block_within_read_buf`
        // is valid.
        let start = usize_from_u32(block_within_read_buf)
            .checked_mul(self.block_size.to_usize())
            .unwrap();
        let end = start.checked_add(self.block_size.to_usize()).unwrap();
        let src = &self.read_buf[start..end];

        // Take an entry from the back of the cache. Note that although
        // this removes the entry from the deque, the entry is just
        // being moved, so the large block allocation within the entry
        // is not freed or reallocated.
        let mut entry = self.entries.pop_back().unwrap();

        entry.block_index = block_index;
        entry.data.copy_from_slice(src);

        // Move the entry to the front of the cache.
        self.entries.push_front(entry);
    }
}

#[derive(Debug, PartialEq)]
struct CacheOpts {
    block_size: BlockSize,
    max_blocks_per_read: u32,
    num_entries: usize,
}

impl CacheOpts {
    /// Create `CacheOpts` with sensible values based on the block size.
    fn new(block_size: BlockSize) -> Self {
        // On a typical 4K-blocksize filesystem, read 8 blocks at a
        // time.
        let max_bytes_per_read = 8 * 4096;
        // Ensure that at least one block is read at a time.
        let max_blocks_per_read = 1.max(max_bytes_per_read / block_size.to_nz_u32());

        // OK to unwrap: the smallest block size is 1024, so
        // `max_blocks_per_read` cannot exceed
        // ((8*4096)/1024)=32. `num_entries` is therefore at most
        // 32*8=256, which fits in `u32`.
        let num_entries: u32 = max_blocks_per_read.checked_mul(8).unwrap();

        Self {
            block_size,
            max_blocks_per_read,
            num_entries: usize_from_u32(num_entries),
        }
    }

    fn read_buf_size_in_bytes(&self) -> usize {
        // OK to unwrap: outside of tests, `CacheOpts` is always created
        // by the new method. For any large block size,
        // `max_blocks_per_read` is capped to 1, so the multiplication
        // cannot cause overflow.
        usize_from_u32(self.max_blocks_per_read)
            .checked_mul(self.block_size.to_usize())
            .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Convert block size in bytes to a `BlockSize`.
    fn get_block_size(sz: u32) -> BlockSize {
        let bs = BlockSize::from_superblock_value(sz.ilog2() - 10).unwrap();
        assert_eq!(bs.to_u32(), sz);
        bs
    }

    #[test]
    fn test_cache_opts() {
        let block_size = get_block_size(1024);
        assert_eq!(
            CacheOpts::new(block_size),
            CacheOpts {
                block_size,
                max_blocks_per_read: 32,
                num_entries: 256,
            }
        );

        let block_size = get_block_size(4096);
        assert_eq!(
            CacheOpts::new(block_size),
            CacheOpts {
                block_size,
                max_blocks_per_read: 8,
                num_entries: 64,
            }
        );

        let block_size = get_block_size(65536);
        assert_eq!(
            CacheOpts::new(block_size),
            CacheOpts {
                block_size,
                max_blocks_per_read: 1,
                num_entries: 8,
            }
        );
    }

    #[test]
    fn test_num_blocks_to_read() {
        let num_fs_blocks = 8;
        let cache = BlockCache::with_opts(
            CacheOpts {
                block_size: get_block_size(1024),
                max_blocks_per_read: 4,
                num_entries: 4,
            },
            num_fs_blocks,
        )
        .unwrap();
        assert_eq!(cache.num_blocks_to_read(0), 4);
        assert_eq!(cache.num_blocks_to_read(4), 4);
        assert_eq!(cache.num_blocks_to_read(5), 3);
        assert_eq!(cache.num_blocks_to_read(7), 1);
    }

    #[test]
    fn test_insert_block() {
        let num_fs_blocks = 8;
        let mut cache = BlockCache::with_opts(
            CacheOpts {
                block_size: get_block_size(1024),
                max_blocks_per_read: 4,
                num_entries: 4,
            },
            num_fs_blocks,
        )
        .unwrap();

        cache.read_buf[0] = 6;
        cache.read_buf[1024] = 7;

        // Insert a block and check that it's in the front of the cache.
        cache.insert_block(123, 0);
        assert_eq!(cache.entries[0].block_index, 123);
        assert_eq!(cache.entries[0].data[0], 6);
        let block123_ptr = cache.entries[0].data.as_ptr();

        // Insert another block, which is now the front of the cache.
        cache.insert_block(456, 1);
        assert_eq!(cache.entries[0].block_index, 456);
        assert_eq!(cache.entries[0].data[0], 7);

        // Check that the previous front of the cache is now in the
        // second entry.
        assert_eq!(cache.entries[1].block_index, 123);
        assert_eq!(cache.entries[1].data[0], 6);
        // And verify that the underlying allocation hasn't changed.
        assert_eq!(cache.entries[1].data.as_ptr(), block123_ptr);
    }

    #[test]
    fn test_get_or_insert_blocks() {
        let num_fs_blocks = 8;
        let mut cache = BlockCache::with_opts(
            CacheOpts {
                block_size: get_block_size(1024),
                max_blocks_per_read: 2,
                num_entries: 4,
            },
            num_fs_blocks,
        )
        .unwrap();

        // Test that an error in the closure is propagated.
        assert_eq!(
            cache
                .get_or_insert_blocks(1, |_| { Err(CorruptKind::TooManyBlocksInFile.into()) })
                .unwrap_err(),
            CorruptKind::TooManyBlocksInFile
        );

        // Request block 1. This requires reading, so blocks 1 and 2 are
        // added to the cache.
        let data = cache
            .get_or_insert_blocks(1, |buf| {
                // Expecting two blocks due to `max_blocks_per_read=2`.
                assert_eq!(buf.len(), 1024 * 2);

                // Block 1:
                buf[0] = 3;
                // Block 2:
                buf[1024] = 4;

                Ok(())
            })
            .unwrap();

        // Check that block 1's data was returned.
        assert_eq!(data[0], 3);

        // Requested block should be at the front of the cache.
        assert_eq!(cache.entries[0].block_index, 1);
        assert_eq!(cache.entries[0].data[0], 3);
        // Followed by the other blocks read.
        assert_eq!(cache.entries[1].block_index, 2);
        assert_eq!(cache.entries[1].data[0], 4);

        // Request block 2. This is already in the cache, so no read
        // should occur.
        let data = cache
            .get_or_insert_blocks(2, |_| {
                panic!("read closure called unexpectedly");
            })
            .unwrap();

        // Check that block 2's data was returned.
        assert_eq!(data[0], 4);

        // The requested block should now be at the front of the cache.
        assert_eq!(cache.entries[0].block_index, 2);
        assert_eq!(cache.entries[1].block_index, 1);

        // Add blocks 3 and 4 to the cache.
        cache.get_or_insert_blocks(3, |_| Ok(())).unwrap();
        assert_eq!(cache.entries[0].block_index, 3);
        assert_eq!(cache.entries[1].block_index, 4);
        assert_eq!(cache.entries[2].block_index, 2);
        assert_eq!(cache.entries[3].block_index, 1);

        // Add blocks 5 and 6 to the cache. This causes blocks 1 and 2
        // to be evicted.
        cache.get_or_insert_blocks(5, |_| Ok(())).unwrap();
        assert_eq!(cache.entries[0].block_index, 5);
        assert_eq!(cache.entries[1].block_index, 6);
        assert_eq!(cache.entries[2].block_index, 3);
        assert_eq!(cache.entries[3].block_index, 4);
    }
}
