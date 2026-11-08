// Copyright 2024 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::Ext4;
use crate::checksum::Checksum;
use crate::error::{CorruptKind, Ext4Error};
use crate::extent::Extent;
use crate::inode::{Inode, InodeIndex};
use crate::util::{read_u16le, read_u32le, u64_from_hilo, usize_from_u32};
use alloc::vec;
use alloc::vec::Vec;

/// Size of each entry within an extent node (including the header
/// entry).
const ENTRY_SIZE_IN_BYTES: usize = 12;

/// Header at the start of a node in an extent tree.
///
/// An extent tree is made up of nodes. Each node may be internal or
/// leaf. Leaf nodes contain `Extent`s. Internal nodes point at other
/// nodes.
///
/// Each node starts with a `NodeHeader` that includes the node's depth
/// (depth 0 is a leaf node) and the number of entries in the node.
struct NodeHeader {
    /// Number of entries in this node, not including the header.
    num_entries: u16,

    /// Maximum number of entries in this node, not including the header.
    max_entries: u16,

    /// Depth of this node in the tree. Zero means it's a leaf node. The
    /// maximum depth is five.
    depth: u16,
}

/// Returns `(n + 1) * ENTRY_SIZE_IN_BYTES`.
///
/// The maximum value this returns is 786432.
fn add_one_mul_entry_size(n: u16) -> usize {
    // OK to unwrap: the maximum value of `n` is `2^16-1`, so the
    // maximum value of this sum is `2^16`. That fits in a `u32`, and we
    // assume `usize` is at least as big as a `u32`.
    let n_plus_one = usize::from(n).checked_add(1).unwrap();
    // OK to unwrap: `n_plus_one` is at most `2^16` and
    // `ENTRY_SIZE_IN_BYTES` is 12, so the maximum product is 786432,
    // which fits in a `u32`. We assume `usize` is at least as big as a
    // `u32`.
    n_plus_one.checked_mul(ENTRY_SIZE_IN_BYTES).unwrap()
}

impl NodeHeader {
    /// Size of the node, including the header.
    fn node_size_in_bytes(&self) -> usize {
        add_one_mul_entry_size(self.num_entries)
    }

    /// Offset of the node's extent data.
    ///
    /// Per `add_one_mul_entry_size`, the maximum value this returns is
    /// 786432.
    fn checksum_offset(&self) -> usize {
        add_one_mul_entry_size(self.max_entries)
    }
}

impl NodeHeader {
    /// Read a `NodeHeader` from a byte slice.
    fn from_bytes(data: &[u8], inode: InodeIndex) -> Result<Self, Ext4Error> {
        if data.len() < ENTRY_SIZE_IN_BYTES {
            return Err(CorruptKind::ExtentNotEnoughData(inode).into());
        }

        let eh_magic = read_u16le(data, 0);
        let eh_entries = read_u16le(data, 2);
        let eh_max = read_u16le(data, 4);
        let eh_depth = read_u16le(data, 6);

        if eh_magic != 0xf30a {
            return Err(CorruptKind::ExtentMagic(inode).into());
        }

        if eh_entries > eh_max {
            return Err(CorruptKind::ExtentNotEnoughData(inode).into());
        }
        if eh_depth > 5 {
            return Err(CorruptKind::ExtentDepth(inode).into());
        }

        Ok(Self {
            depth: eh_depth,
            num_entries: eh_entries,
            max_entries: eh_max,
        })
    }
}

#[derive(Clone)]
struct ToVisitItem {
    // Full node data. This starts with the node header, then some
    // number of entries. See [`NodeHeader`] for more on the structure
    // of extent tree data.
    node: Vec<u8>,

    // Current index within the node. 0 is the node header, 1 is the
    // first entry, etc.
    entry: u32,

    // Node depth, copied from the node header.
    depth: u16,
}

impl ToVisitItem {
    fn new(mut node: Vec<u8>, inode: InodeIndex) -> Result<Self, Ext4Error> {
        let header = NodeHeader::from_bytes(&node, inode)?;

        // The node data must be large enough to contain the number of
        // entries specified in the header.
        if node.len() < header.node_size_in_bytes() {
            return Err(CorruptKind::ExtentNotEnoughData(inode).into());
        }

        // Remove unused data at the end (e.g. checksum data).
        node.truncate(header.node_size_in_bytes());

        Ok(Self {
            node,
            entry: 0,
            depth: header.depth,
        })
    }

    fn entry(&self) -> Option<&[u8]> {
        // OK to unwrap: `self.entry` is a `u16` and
        // `ENTRY_SIZE_IN_BYTES` is `12`, so the maximum value of this
        // product is `(2^16-1)*12 = 786420`, which fits in a `u32. We
        // assume `usize` is at least as big as a `u32`.
        let start = usize_from_u32(self.entry)
            .checked_mul(ENTRY_SIZE_IN_BYTES)
            .unwrap();
        // OK to unwrap: `start` is at most 786420, so this sum is at
        // most `786420+12 = 786432`, which fits in a `u32`. We assume
        // `usize` is at least as big as a `u32`.
        let end = start.checked_add(ENTRY_SIZE_IN_BYTES).unwrap();
        self.node.get(start..end)
    }
}

/// Iterator of an inode's extent tree.
pub(crate) struct Extents {
    ext4: Ext4,
    inode: InodeIndex,
    to_visit: Vec<ToVisitItem>,
    checksum_base: Checksum,
    is_done: bool,
}

impl Extents {
    pub(crate) fn new(ext4: Ext4, inode: &Inode) -> Result<Self, Ext4Error> {
        Ok(Self {
            ext4,
            inode: inode.index,
            to_visit: vec![ToVisitItem::new(inode.inline_data.to_vec(), inode.index)?],
            checksum_base: inode.checksum_base.clone(),
            is_done: false,
        })
    }

    // Step to the next entry.
    //
    // This is factored out of `Iterator::next` for clarity and ease of
    // returning errors.
    //
    // # Returns
    //
    // * Returns `Ok(Some(_))` if the next entry is within a leaf node.
    // * Returns `Err` if a hard error occurs (this will be returned by
    //   the iterator, and iteration will be ended on the next
    //   iteration).
    // * Returns `Ok(None)` for other cases. This doesn't end iteration,
    //   just means the iterator is not in a leaf node. The outer loop
    //   in `Iterator::next` will call `next_impl` again as long as
    //   there are nodes left to process.
    fn next_impl(&mut self) -> Result<Option<Extent>, Ext4Error> {
        let Some(item) = self.to_visit.last_mut() else {
            self.is_done = true;
            return Ok(None);
        };

        // Increment at the start to ensure that early returns don't
        // accidentally skip the increment. Since entry 0 is the node
        // header, entry 1 is the first actual node entry.
        //
        // OK to unwrap: there are at most `2^16-1` entries, plus 1 for
        // the header. Adding 1 here brings the maximum value to
        // `2^16+1`. This fits in `item.entry` since it is a `u32`.
        item.entry = item.entry.checked_add(1).unwrap();

        let Some(entry) = item.entry() else {
            // Reached end of this node.
            self.to_visit.pop();
            return Ok(None);
        };

        if item.depth == 0 {
            let ee_block = read_u32le(entry, 0);
            let ee_len = read_u16le(entry, 4);
            if ee_len == 0 {
                return Err(CorruptKind::ExtentBlock(self.inode).into());
            }
            let ee_start_hi = read_u16le(entry, 6);
            let ee_start_low = read_u32le(entry, 8);

            let start_block = u64_from_hilo(u32::from(ee_start_hi), ee_start_low);

            return Ok(Some(Extent {
                block_within_file: ee_block,
                start_block,
                num_blocks: if ee_len > 0x8000 {
                    ee_len - 0x8000
                } else {
                    ee_len
                },
                unwritten: ee_len > 0x8000,
            }));
        } else {
            let ei_leaf_lo = read_u32le(entry, 4);
            let ei_leaf_hi = read_u16le(entry, 8);
            let child_block = u64_from_hilo(u32::from(ei_leaf_hi), ei_leaf_lo);

            // Read just the header of the child node. This is needed to
            // find out how much data is in the full child node.
            let mut child_header = [0; ENTRY_SIZE_IN_BYTES];
            self.ext4
                .read_from_block(child_block, 0, &mut child_header)?;
            let child_header = NodeHeader::from_bytes(&child_header, self.inode)?;

            // OK to unwrap: item.depth is greater than zero in this branch.
            let expected_child_depth = item.depth.checked_sub(1).unwrap();
            // Check that the child's depth is one less than the current
            // node. In combination with the fact that the depth cannot be less
            // than zero, this prevents a corrupt filesystem from causing an
            // infinite loop in this iterator.
            if child_header.depth != expected_child_depth {
                return Err(CorruptKind::ExtentDepth(self.inode).into());
            }

            // The checksum is written in the four bytes directly after
            // the node.
            let checksum_offset = child_header.checksum_offset();
            let checksum_size = if self.ext4.has_metadata_checksums() {
                4
            } else {
                0
            };

            // OK to unwrap: per `checksum_offset()` the maximum offset
            // is 786432, so the maximum sum here is 786436, which fits
            // in a `u32`. We assume `usize` is at least as big as a
            // `u32`.
            let child_node_size: usize = checksum_offset.checked_add(checksum_size).unwrap();
            // Extent nodes are not allowed to exceed the block size.
            if child_node_size > self.ext4.0.superblock.block_size {
                return Err(CorruptKind::ExtentNodeSize(self.inode).into());
            }
            let mut child_node = vec![0; child_node_size];
            self.ext4.read_from_block(child_block, 0, &mut child_node)?;

            // Validating the checksum here covers everything but the
            // root node. The root node is embedded within the inode,
            // which has its own checksum.
            if self.ext4.has_metadata_checksums() {
                let expected_checksum = read_u32le(&child_node, checksum_offset);

                let mut checksum = self.checksum_base.clone();
                checksum.update(&child_node[..checksum_offset]);
                let actual_checksum = checksum.finalize();
                if expected_checksum != actual_checksum {
                    return Err(CorruptKind::ExtentChecksum(self.inode).into());
                }
            }

            self.to_visit
                .push(ToVisitItem::new(child_node, self.inode)?);
        }

        // This does not indicate end of iteration, we just haven't
        // reached a leaf node yet.
        Ok(None)
    }
}

impl_result_iter!(Extents, Extent);
