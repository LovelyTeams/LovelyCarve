// Adapted from KarpelesLab/fstool 0.5.0 (MIT); see ORIGIN.md.
//! F2FS NAT — Node Address Table.
//!
//! A NAT entry is `{ version: u8, ino: u32, block_addr: u32 }` (9 bytes).
//! 455 entries fit in one 4 KiB NAT page; the entry for nid `N` lives in
//! page `N / 455` at slot `N % 455`. Each logical page has two physical
//! copies for shadow paging, and they are interleaved *per segment*, not
//! split into two halves of the region: the logical page numbered
//! `q * blocks_per_seg + r` lives at offset `2 * q * blocks_per_seg + r`,
//! with its shadow one whole segment further on. Which of the pair is current is a per-page
//! bit in the checkpoint's NAT version bitmap. The function
//! [`current_nat_addr`] implements both rules exactly as the kernel
//! function of the same name does.
//!
//! Lookups consult the checkpoint's in-memory NAT journal first: any
//! `nid` present there is the freshest version regardless of what the
//! on-disk page says.
//!
//! Reference: kernel docs §"NAT" + FAST '15 §2.2.

use crate::block::BlockDevice;
use crate::Result;

use super::checkpoint::Checkpoint;
use super::constants::{F2FS_BLKSIZE, NAT_ENTRY_PER_BLOCK, NAT_ENTRY_SIZE};
use super::superblock::Superblock;

/// Resolved physical block address for a node id.
#[derive(Debug, Clone, Copy)]
pub struct NodeAddr {
    pub block: u32,
    pub version: u8,
    pub ino: u32,
}

/// Look up the on-disk block holding the node `nid`.
///
/// 1. Consult the checkpoint's NAT journal (newest in-memory state).
/// 2. Otherwise read the appropriate NAT page from the active pack and
///    decode the entry at `nid % 455`.
pub fn lookup_node(
    dev: &mut dyn BlockDevice,
    sb: &Superblock,
    cp: &Checkpoint,
    nid: u32,
) -> Result<NodeAddr> {
    if let Some(j) = cp.nat_journal_lookup(nid) {
        if j.block_addr == 0 {
            return Err(crate::invalid(format!(
                "f2fs: nid {nid} unallocated (journal block_addr=0)"
            )));
        }
        return Ok(NodeAddr {
            block: j.block_addr,
            version: j.version,
            ino: j.ino,
        });
    }

    let bs = sb.block_size() as u64;
    let slot = (nid as usize) % NAT_ENTRY_PER_BLOCK;
    let phys_page = current_nat_addr(sb, cp, nid)?;

    let mut page = vec![0u8; F2FS_BLKSIZE];
    dev.read_at(phys_page as u64 * bs, &mut page)?;

    let o = slot * NAT_ENTRY_SIZE;
    if o + NAT_ENTRY_SIZE > page.len() {
        return Err(crate::invalid("f2fs: NAT slot past end of page".into()));
    }
    let version = page[o];
    let ino = u32::from_le_bytes(page[o + 1..o + 5].try_into().unwrap());
    let block_addr = u32::from_le_bytes(page[o + 5..o + 9].try_into().unwrap());
    if block_addr == 0 {
        return Err(crate::invalid(format!(
            "f2fs: nid {nid} has block_addr=0 (unallocated)"
        )));
    }
    Ok(NodeAddr {
        block: block_addr,
        version,
        ino,
    })
}

/// Test the `block_off`-th bit of an f2fs version bitmap.
///
/// f2fs numbers bits inside a byte MSB-first (`f2fs_test_bit()` in
/// `fs/f2fs/f2fs.h` masks `BIT(7 - (nr & 7))`), which is the opposite of
/// the kernel's generic little-endian bitops. A bit past the end of the
/// bitmap reads as clear, the same as a freshly formatted volume.
fn f2fs_test_bit(block_off: u32, bitmap: &[u8]) -> bool {
    let byte = (block_off >> 3) as usize;
    match bitmap.get(byte) {
        Some(b) => b & (1 << (7 - (block_off & 7))) != 0,
        None => false,
    }
}

/// Physical block holding the current copy of the NAT page that owns
/// `nid` — the kernel's `current_nat_addr()`:
///
/// ```text
/// block_off = nid / NAT_ENTRY_PER_BLOCK
/// addr      = nat_blkaddr + (block_off << 1) - (block_off % blocks_per_seg)
/// if test_bit(block_off, nat_bitmap) { addr += blocks_per_seg }
/// ```
///
/// The subtraction is what makes the two copies interleave a segment at
/// a time instead of splitting the region into halves: logical page
/// `q * blocks_per_seg + r` lands in segment `2q` at offset `r`, its
/// shadow in segment `2q + 1`.
pub fn current_nat_addr(sb: &Superblock, cp: &Checkpoint, nid: u32) -> Result<u32> {
    let blocks_per_seg = sb.blocks_per_seg();
    let block_off = nid / NAT_ENTRY_PER_BLOCK as u32;
    // `segment_count_nat`, `blocks_per_seg` and `nat_blkaddr` are all
    // untrusted; use checked arithmetic so a crafted superblock can't
    // overflow into a bogus (but in-bounds-looking) physical page.
    let nat_total_blocks = sb
        .segment_count_nat
        .checked_mul(blocks_per_seg)
        .ok_or_else(|| crate::invalid("f2fs: NAT geometry overflow".into()))?;
    let mut nat_offset = block_off
        .checked_mul(2)
        .map(|x| x - (block_off & (blocks_per_seg - 1)))
        .ok_or_else(|| crate::invalid("f2fs: NAT page index overflow".into()))?;
    if f2fs_test_bit(block_off, &cp.nat_bitmap) {
        nat_offset = nat_offset
            .checked_add(blocks_per_seg)
            .ok_or_else(|| crate::invalid("f2fs: NAT page index overflow".into()))?;
    }
    if nat_offset >= nat_total_blocks {
        return Err(crate::invalid(format!("f2fs: nid {nid} out of NAT range")));
    }
    sb.nat_blkaddr
        .checked_add(nat_offset)
        .ok_or_else(|| crate::invalid("f2fs: NAT page address overflow".into()))
}
