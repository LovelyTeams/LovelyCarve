// Copyright 2024 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::Ext4;
use crate::block_index::{FileBlockIndex, FsBlockIndex};
use crate::dir_block::DirBlock;
use crate::dir_entry::{DirEntry, DirEntryName};
use crate::dir_entry_hash::HashAlg;
use crate::error::{CorruptKind, Ext4Error};
use crate::extent::Extent;
use crate::inode::{Inode, InodeFlags, InodeIndex};
use crate::iters::extents::Extents;
use crate::iters::file_blocks::FileBlocks;
use crate::path::PathBuf;
use crate::util::{read_u16le, read_u32le, usize_from_u32};
use alloc::rc::Rc;
use alloc::vec;

type DirHash = u32;

// Internal node of an htree.
//
// This stores a reference to the raw bytes of entries in an internal
// node (including the root node) of an htree.
//
// Each entry is eight bytes long.
//
// The first entry is a header with three fields:
// * limit (u16): the number of entries that could be present (including
//   the header entry). In other words, space has been allocated for
//   this many entries.
// * count (u16): the actual number of entries (including the header
//   entry).
// * zero_block (u32): the child block index to use when looking up
//   hashes that compare less-than the first "normal" entry's hash key.
//
// The remaining entries each contain two fields:
// * hash: the minimum hash for this block. All directory entries in
//   this block (or children of this block) have a hash greater than or
//   equal to the hash key.
// * block (u32): the child block index.
//
// The entries after the header are sorted by hash, allowing for
// efficient hash lookup with a binary search.
//
// Note that all block indices mentioned above are relative to the file,
// not the file system. E.g. index zero is the file's first block, not
// the first block in the filesystem.
//
// Example of entries in an internal node:
// 0:  122, 15, 1      (limit, count, zero_block)
// 1:  0x0d69cdd8, 15  (hash, block)
// 2:  0x1eb8a274, 7   (hash, block)
// 3:  0x31df5aa2, 12  (hash, block)
// 4:  0x418c4380, 3   (hash, block)
// [...]
// 14: 0xec5cb0ca, 10  (hash, block)
#[derive(Debug)]
struct InternalNode<'a> {
    /// Raw entry data. The header entry is included. Entries that are
    /// not in use are excluded (in other words, this includes entries
    /// up to `count`, not `limit`).
    entries: &'a [u8],
}

impl<'a> InternalNode<'a> {
    const ENTRY_SIZE: usize = 8;

    /// Create an `InternalNode` from a root directory block.
    fn from_root_block(block: &'a [u8], inode: InodeIndex) -> Result<Self, Ext4Error> {
        Self::new(&block[0x20..], inode)
    }

    /// Create an `InternalNode` from a non-root directory block.
    fn from_non_root_block(block: &'a [u8], inode: InodeIndex) -> Result<Self, Ext4Error> {
        Self::new(&block[0x8..], inode)
    }

    /// Create an `InternalNode` from raw bytes. These bytes come from a
    /// directory block, see [`from_root_block`] and [`from_non_root_block`].
    fn new(mut bytes: &'a [u8], inode: InodeIndex) -> Result<Self, Ext4Error> {
        // At least the header entry must be present.
        if bytes.len() < Self::ENTRY_SIZE {
            return Err(CorruptKind::HtreeInternalNodeMissingHeader {
                inode,
                num_bytes: bytes.len(),
            }
            .into());
        }

        // Get number of in-use entries from the header.
        let count = usize::from(read_u16le(bytes, 2));

        // OK to unwrap: `ENTRY_SIZE` is 8 and `count` is at most
        // 2^16-1, so the result is at most 524,280. That fits in a
        // `u32`, and we assume that `usize` is at least that large.
        let end_byte: usize = Self::ENTRY_SIZE.checked_mul(count).unwrap();

        // Shrink raw data to exactly the valid length, or return an
        // error if not enough data.
        bytes = bytes
            .get(..end_byte)
            .ok_or(CorruptKind::HtreeInternalNodeCountTooLarge {
                inode,
                count,
                num_bytes: bytes.len(),
            })?;

        Ok(Self { entries: bytes })
    }

    /// Look up the entry at `index`. Returns `(hash, block)`.
    /// Panics if `index` is out of range.
    ///
    /// For `index` zero, the `hash` key is implicitly zero.
    fn get_entry(&self, index: usize) -> (DirHash, FileBlockIndex) {
        // OK to unwrap: `ENTRY_SIZE` is 8 and `index` is at most
        // 2^16-1, so the result is at most 524,280. That fits in a `u32`,
        // and we assume that `usize` is at least that large.
        let offset: usize = Self::ENTRY_SIZE.checked_mul(index).unwrap();

        // OK to unwrap: `offset` is at most 2^19, so the result still
        // fits in a `u32` and we assume that `usize` is at least that
        // large.
        let block_offset: usize = offset.checked_add(4).unwrap();

        let block = read_u32le(self.entries, block_offset);

        let hash = if index == 0 {
            0
        } else {
            read_u32le(self.entries, offset)
        };

        (hash, block)
    }

    /// Get the number of entries (this is based on the `count` field,
    /// not the `limit` field).
    fn num_entries(&self) -> usize {
        self.entries.len() / Self::ENTRY_SIZE
    }

    /// Perform a binary search to find the child block index for the
    /// `lookup_hash`.
    fn lookup_block_by_hash(&self, lookup_hash: DirHash) -> Option<FileBlockIndex> {
        // Left/right entry index.
        let mut left = 0;
        let mut right = self.num_entries().checked_sub(1)?;

        while left <= right {
            let mid = left.checked_add(right)? / 2;
            let mid_hash = self.get_entry(mid).0;
            if mid_hash <= lookup_hash {
                left = mid.checked_add(1)?;
            } else {
                right = mid.checked_sub(1)?;
            }
        }

        let index = left.checked_sub(1)?;
        Some(self.get_entry(index).1)
    }
}

/// Read the block containing the root node of an htree into
/// `block`. This is always the first block of the file.
fn read_root_block(fs: &Ext4, inode: &Inode, block: &mut [u8]) -> Result<(), Ext4Error> {
    let mut file_blocks = FileBlocks::new(fs.clone(), inode)?;

    // Get the first block.
    let block_index = file_blocks
        .next()
        .ok_or(CorruptKind::DirEntry(inode.index))??;

    // Read the first block of the extent.
    let dir_block = DirBlock {
        fs,
        dir_inode: inode.index,
        block_index,
        is_first: true,
        has_htree: true,
        checksum_base: inode.checksum_base.clone(),
    };
    dir_block.read(block)
}

/// Check if name is "." or ".." and return the corresponding entry if
/// so. These entries exist at hardcoded offsets within the root block
/// of the htree.
///
/// `block` is the raw block data of the first directory block.
///
/// If name is neither "." nor "..", returns `None`.
fn read_dot_or_dotdot(
    fs: Ext4,
    inode: &Inode,
    name: DirEntryName<'_>,
    block: &[u8],
) -> Result<Option<DirEntry>, Ext4Error> {
    let corrupt = || CorruptKind::DirEntry(inode.index).into();

    let offset = if name == "." {
        0
    } else if name == ".." {
        12
    } else {
        return Ok(None);
    };

    let (entry, _size) =
        DirEntry::from_bytes(fs, &block[offset..], inode.index, Rc::new(PathBuf::empty()))?;
    let entry = entry.ok_or_else(corrupt)?;
    if entry.file_name() == name {
        Ok(Some(entry))
    } else {
        Err(corrupt())
    }
}

/// Find the extent within a file that includes the given child `block`.
fn find_extent_for_block(
    fs: &Ext4,
    inode: &Inode,
    block: FileBlockIndex,
) -> Result<Extent, Ext4Error> {
    for extent in Extents::new(fs.clone(), inode)? {
        let extent = extent?;

        let start = extent.block_within_file;
        let end = start
            .checked_add(u32::from(extent.num_blocks))
            .ok_or(CorruptKind::DirEntry(inode.index))?;
        if block >= start && block < end {
            return Ok(extent);
        }
    }

    Err(CorruptKind::DirEntry(inode.index).into())
}

/// Convert from a block offset within a file to an absolute block index.
fn block_from_file_block(
    fs: &Ext4,
    inode: &Inode,
    relative_block: FileBlockIndex,
) -> Result<FsBlockIndex, Ext4Error> {
    if inode.flags.contains(InodeFlags::EXTENTS) {
        let extent = find_extent_for_block(fs, inode, relative_block)?;
        let block_within_extent = relative_block
            .checked_sub(extent.block_within_file)
            .ok_or(CorruptKind::DirEntry(inode.index))?;
        let absolute_block = extent
            .start_block
            .checked_add(u64::from(block_within_extent))
            .ok_or(CorruptKind::DirEntry(inode.index))?;
        Ok(absolute_block)
    } else {
        let mut block_map = FileBlocks::new(fs.clone(), inode)?;
        block_map
            .nth(usize_from_u32(relative_block))
            .ok_or(CorruptKind::DirEntry(inode.index))?
    }
}

/// Traverse the htree to find the leaf node that might contain `name`.
///
/// On success, `block` will contain the leaf node's directory block
/// data.
fn find_leaf_node(
    fs: &Ext4,
    inode: &Inode,
    name: DirEntryName<'_>,
    block: &mut [u8],
) -> Result<(), Ext4Error> {
    // Read the htree's hash type from the root block.
    let hash_alg = HashAlg::from_u8(block[0x1c])?;

    // Read the htree's depth from the root block. The depth is the
    // number of levels in the tree excluding the root and leaf
    // levels. So for example, a depth of one means there is a root
    // node, one level of internal nodes, and one level of leaf nodes.
    let depth = block[0x1e];

    // Get the node structure from the root block.
    let root_node = InternalNode::from_root_block(block, inode.index)?;

    let hash = hash_alg.hash(name, &fs.0.superblock.htree_hash_seed);
    let mut child_block_relative = root_node
        .lookup_block_by_hash(hash)
        .ok_or(CorruptKind::DirEntry(inode.index))?;

    // Descend through the tree one level at a time. The first iteration
    // of the loop goes from the root node to a child. The last
    // iteration (which may also be the first iteration) will read the
    // leaf node data into `block`.
    for level in 0..=depth {
        // Get the absolute block index and read the block's data.
        let block_index = block_from_file_block(fs, inode, child_block_relative)?;
        let dir_block = DirBlock {
            fs,
            dir_inode: inode.index,
            block_index,
            is_first: false,
            has_htree: true,
            checksum_base: inode.checksum_base.clone(),
        };
        dir_block.read(block)?;

        // If the block is an internal node, find the next child
        // block. Otherwise, we've reached a leaf node and there's
        // nothing more to do.
        if level != depth {
            let inner_node = InternalNode::from_non_root_block(block, inode.index)?;
            child_block_relative = inner_node
                .lookup_block_by_hash(hash)
                .ok_or(CorruptKind::DirEntry(inode.index))?;
        }
    }

    Ok(())
}

/// Find a directory entry via a directory htree. The htree is a tree of
/// nodes that use hashes for keys. The hash of `name` is used to
/// traverse this tree to a leaf node. The leaf node is an linear array
/// of directory entries; these are searched through in order to find
/// the one matching `name`.
///
/// Returns [`Ext4Error::NotFound`] if the entry doesn't exist.
///
/// Panics if the directory doesn't have an htree.
pub(crate) fn get_dir_entry_via_htree(
    fs: &Ext4,
    inode: &Inode,
    name: DirEntryName<'_>,
) -> Result<DirEntry, Ext4Error> {
    assert!(inode.flags.contains(InodeFlags::DIRECTORY_HTREE));

    let block_size = fs.0.superblock.block_size;
    let mut block = vec![0; block_size.to_usize()];

    // Read the first block of the file, which contains the root node of
    // the htree.
    read_root_block(fs, inode, &mut block)?;

    // Handle "." and ".." entries.
    if let Some(entry) = read_dot_or_dotdot(fs.clone(), inode, name, &block)? {
        return Ok(entry);
    }

    // Find the leaf node that might contain the entry. This will update
    // `block` to contain the leaf node's block data.
    find_leaf_node(fs, inode, name, &mut block)?;

    // The entry's `path()` method will not be called, so the value of
    // the base path does not matter.
    let path = Rc::new(PathBuf::empty());

    // Do a linear search through the leaf block for the right entry.
    let mut offset_within_block = 0;
    while offset_within_block < block.len() {
        let (dir_entry, entry_size) = DirEntry::from_bytes(
            fs.clone(),
            &block[offset_within_block..],
            inode.index,
            path.clone(),
        )?;
        offset_within_block = offset_within_block
            .checked_add(entry_size.get())
            .ok_or(CorruptKind::DirEntry(inode.index))?;
        let Some(dir_entry) = dir_entry else {
            continue;
        };

        if dir_entry.file_name() == name {
            return Ok(dir_entry);
        }
    }

    Err(Ext4Error::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "std")]
    use crate::{FollowSymlinks, Path, ReadDir};

    #[test]
    fn test_internal_node() {
        let inode = InodeIndex::new(1).unwrap();

        let mut bytes = Vec::new();
        let add_entry = |bytes: &mut Vec<u8>, hash: DirHash, block: FileBlockIndex| {
            bytes.extend(hash.to_le_bytes());
            bytes.extend(block.to_le_bytes());
        };
        bytes.extend(20u16.to_le_bytes()); // limit
        bytes.extend(11u16.to_le_bytes()); // count
        bytes.extend(100u32.to_le_bytes()); // block

        add_entry(&mut bytes, 2, 199);
        add_entry(&mut bytes, 4, 198);
        add_entry(&mut bytes, 6, 197);
        add_entry(&mut bytes, 8, 196);
        add_entry(&mut bytes, 10, 195);

        add_entry(&mut bytes, 12, 194);
        add_entry(&mut bytes, 14, 193);
        add_entry(&mut bytes, 16, 192);
        add_entry(&mut bytes, 18, 191);
        add_entry(&mut bytes, 20, 190);

        // Test search with an odd number of entries.
        let node = InternalNode::new(&bytes, inode).unwrap();
        assert_eq!(node.num_entries(), 11);
        assert_eq!(node.get_entry(0), (0, 100));
        assert_eq!(node.get_entry(10), (20, 190));
        assert_eq!(node.lookup_block_by_hash(0), Some(100));
        assert_eq!(node.lookup_block_by_hash(9), Some(196));
        assert_eq!(node.lookup_block_by_hash(10), Some(195));
        assert_eq!(node.lookup_block_by_hash(11), Some(195));
        assert_eq!(node.lookup_block_by_hash(12), Some(194));
        assert_eq!(node.lookup_block_by_hash(20), Some(190));
        assert_eq!(node.lookup_block_by_hash(30), Some(190));

        // Add one more entry.
        bytes[2..4].copy_from_slice(&12u16.to_le_bytes()); // count
        add_entry(&mut bytes, 30, 189);

        // Test search with an even number of entries.
        let node = InternalNode::new(&bytes, inode).unwrap();
        assert_eq!(node.num_entries(), 12);
        assert_eq!(node.lookup_block_by_hash(0), Some(100));
        assert_eq!(node.lookup_block_by_hash(9), Some(196));
        assert_eq!(node.lookup_block_by_hash(10), Some(195));
        assert_eq!(node.lookup_block_by_hash(11), Some(195));
        assert_eq!(node.lookup_block_by_hash(12), Some(194));
        assert_eq!(node.lookup_block_by_hash(20), Some(190));
        assert_eq!(node.lookup_block_by_hash(30), Some(189));
    }

    #[test]
    fn test_internal_node_errors() {
        let inode = InodeIndex::new(123).unwrap();
        assert_eq!(
            InternalNode::new(&[0; 7], inode).unwrap_err(),
            CorruptKind::HtreeInternalNodeMissingHeader {
                inode,
                num_bytes: 7,
            }
        );

        let mut bytes = Vec::new();
        bytes.extend(20u16.to_le_bytes()); // limit
        bytes.extend(2u16.to_le_bytes()); // count
        bytes.extend(456u32.to_le_bytes()); // zero block
        assert_eq!(
            InternalNode::new(&bytes, inode).unwrap_err(),
            CorruptKind::HtreeInternalNodeCountTooLarge {
                inode,
                num_bytes: 8,
                count: 2,
            }
        );
    }

    #[cfg(feature = "std")]
    #[test]
    fn test_read_dot_or_dotdot() {
        let fs = crate::test_util::load_test_disk1();

        let mut block = vec![0; fs.0.superblock.block_size.to_usize()];

        // Read the root block of an htree.
        let inode = fs
            .path_to_inode("/big_dir".try_into().unwrap(), FollowSymlinks::All)
            .unwrap();
        read_root_block(&fs, &inode, &mut block).unwrap();

        // Get the "." entry.
        let entry = read_dot_or_dotdot(fs.clone(), &inode, ".".try_into().unwrap(), &block)
            .unwrap()
            .unwrap();
        assert_eq!(entry.file_name(), ".");

        // Get the ".." entry.
        let entry = read_dot_or_dotdot(fs.clone(), &inode, "..".try_into().unwrap(), &block)
            .unwrap()
            .unwrap();
        assert_eq!(entry.file_name(), "..");

        // Check that an arbitrary name returns `None`.
        assert!(
            read_dot_or_dotdot(fs.clone(), &inode, "somename".try_into().unwrap(), &block)
                .unwrap()
                .is_none()
        );

        // Error: the first directory entry in the root node is not ".".
        assert_eq!(block[8], b'.');
        block[8] = b'x';
        assert_eq!(
            read_dot_or_dotdot(fs.clone(), &inode, ".".try_into().unwrap(), &block,).unwrap_err(),
            CorruptKind::DirEntry(inode.index)
        );

        // Error: invalid directory block.
        block.fill(0);
        assert_eq!(
            read_dot_or_dotdot(fs.clone(), &inode, ".".try_into().unwrap(), &block,).unwrap_err(),
            CorruptKind::DirEntryRecordTooSmall(inode.index, 0)
        );
    }

    /// Use ReadDir to iterate over all directory entries. Check that
    /// each entry can be looked up directly via the htree.
    ///
    /// Returns the number of entries.
    #[cfg(feature = "std")]
    #[track_caller]
    fn compare_all_entries(fs: &Ext4, dir: Path<'_>) -> usize {
        let dir_inode = fs.path_to_inode(dir, FollowSymlinks::All).unwrap();
        let iter = ReadDir::new(fs.clone(), &dir_inode, PathBuf::from(dir)).unwrap();
        let mut count = 0;
        for iter_entry in iter {
            let iter_entry = iter_entry.unwrap();
            let htree_entry =
                get_dir_entry_via_htree(fs, &dir_inode, iter_entry.file_name()).unwrap();
            assert_eq!(htree_entry.file_name(), iter_entry.file_name());
            assert_eq!(htree_entry.inode, iter_entry.inode);
            count += 1;
        }
        count
    }

    #[cfg(feature = "std")]
    #[test]
    fn test_get_dir_entry_via_htree() {
        let fs = crate::test_util::load_test_disk1();

        // Resolve paths in `/medium_dir` via htree.
        let medium_dir = Path::new("/medium_dir");
        assert_eq!(compare_all_entries(&fs, medium_dir), 1_002);

        // Resolve paths in `/big_dir` via htree.
        let big_dir = Path::new("/big_dir");
        assert_eq!(compare_all_entries(&fs, big_dir), 10_002);
    }

    /// Test `block_from_file_block` with a file that uses extents.
    #[cfg(feature = "std")]
    #[test]
    fn test_block_from_file_block() {
        let fs = crate::test_util::load_test_disk1();

        // Manually construct a simple extent tree containing two
        // extents.
        //
        // The test disk has experienced relatively few operations
        // compared to a real-world filesystem, so it doesn't have much
        // fragmentation. In particular, all of its directory tree
        // inodes currently have a single extent with a relative offset
        // of 0, which doesn't fully exercise
        // `block_from_file_block`. Create some slightly more
        // interesting extents to test here.
        let mut extents = Vec::new();
        // Node header:
        // Magic:
        extents.extend(&0xf30au16.to_le_bytes());
        // Num entries:
        extents.extend(&2u16.to_le_bytes());
        // Max entries:
        extents.extend(&2u16.to_le_bytes());
        // Depth (leaf):
        extents.extend(&0u16.to_le_bytes());
        // Padding:
        extents.extend(&0u32.to_le_bytes());
        // Extent 0:
        // Relative start block:
        extents.extend(&0u32.to_le_bytes());
        // Num blocks:
        extents.extend(&23u16.to_le_bytes());
        // Absolute start block (hi, lo):
        extents.extend(0u16.to_le_bytes());
        extents.extend(2543u32.to_le_bytes());
        // Extent 1:
        // Relative start block:
        extents.extend(&23u32.to_le_bytes());
        // Num blocks:
        extents.extend(&47u16.to_le_bytes());
        // Absolute start block (hi, lo):
        extents.extend(0u16.to_le_bytes());
        extents.extend(11u32.to_le_bytes());

        extents.resize(60usize, 0u8);

        // Grab a convenient inode and overwrite its inline data with
        // the new extent tree.
        let mut inode = fs
            .path_to_inode("/medium_dir".try_into().unwrap(), FollowSymlinks::All)
            .unwrap();
        inode.inline_data.copy_from_slice(&extents);

        // Verify the extents.
        let extents: Vec<_> = Extents::new(fs.clone(), &inode)
            .unwrap()
            .map(|e| e.unwrap())
            .collect();
        assert_eq!(
            extents,
            [
                Extent {
                    start_block: 2543,
                    unwritten: false,
                    num_blocks: 23,
                    block_within_file: 0,
                },
                Extent {
                    start_block: 11,
                    unwritten: false,
                    num_blocks: 47,
                    block_within_file: 23,
                }
            ]
        );

        // Blocks in extent 0.
        assert_eq!(block_from_file_block(&fs, &inode, 0).unwrap(), 2543);
        assert_eq!(block_from_file_block(&fs, &inode, 1).unwrap(), 2544);
        assert_eq!(block_from_file_block(&fs, &inode, 22).unwrap(), 2565);

        // Blocks in extent 1.
        assert_eq!(block_from_file_block(&fs, &inode, 23).unwrap(), 11);
        assert_eq!(block_from_file_block(&fs, &inode, 24).unwrap(), 12);
        assert_eq!(block_from_file_block(&fs, &inode, 69).unwrap(), 57);

        // Invalid block.
        assert!(block_from_file_block(&fs, &inode, 70).is_err());
    }
}
