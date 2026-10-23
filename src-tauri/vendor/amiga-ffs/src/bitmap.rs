//! The allocation bitmap: which blocks the volume believes are in use.
//!
//! One bit per block, and four conventions that are each easy to get
//! backwards. All four were confirmed by dumping bitmaps out of images
//! xdftool built and checking that the blocks they mark allocated are the
//! blocks the filesystem actually wrote:
//!
//! 1. **A set bit means the block is *free*.** Not allocated. A freshly
//!    formatted volume is nearly all ones. Reading this the usual way
//!    round produces an allocator that hands out the blocks in use and
//!    refuses the empty ones.
//! 2. **Bits run LSB-first within each longword.** Bit 0 of the first
//!    longword after the checksum is the first block the bitmap covers;
//!    bit 31 is the thirty-second. Big-endian longwords, little-endian
//!    bits — the format is not being consistent, and a reader that
//!    assumes it is gets a scrambled but plausible-looking answer.
//! 3. **The first bit is block `reserved`**, normally 2. The two boot
//!    blocks are never in the bitmap at all: they are not allocatable, so
//!    they have no bit, and treating bit 0 as block 0 shifts every answer
//!    by two.
//! 4. **The checksum is longword 0**, not longword 5 like every header
//!    block ([`BITMAP_CHECKSUM_INDEX`]). A bitmap block has no type
//!    longword and no own-key; the checksum is the entire header.
//!
//! # Where the pages live
//!
//! The root block holds [`BITMAP_PAGES`] (25) pointers to bitmap blocks
//! at longwords −49..=−25, and longword −24 chains to a **bitmap
//! extension block** when 25 is not enough. At 512 bytes one page covers
//! 4064 blocks, so the root alone reaches 101 600 blocks — about 50 MB,
//! which is where the historical "FFS can't do more than 50-odd MB"
//! folklore comes from: old filesystems wrote a directory's protection
//! longword into the root, at longword −24, and clobbered the extension
//! pointer.
//!
//! An extension block is not like other blocks: no type, no own key, **no
//! checksum**, just [`bitmap_ext_pointers`] page pointers with the last
//! longword chaining to the next extension block. There is nothing in one
//! to verify, which is worth knowing before trying to.
//!
//! # `bitmap_flag`
//!
//! The root's longword −50 is −1 when the bitmap is valid and 0 when it
//! is not. AmigaDOS clears it while a volume is mounted-and-dirty and
//! sets it again on a clean unmount, so a 0 means the volume was not shut
//! down cleanly and the bitmap on disk is whatever it was mid-update.
//! [`Bitmap::valid`] carries that through rather than hiding it: the bits
//! are still read, because a recovery tool wants to see them, and they
//! are still marked untrustworthy, because an allocator must not use
//! them.

use alloc::collections::BTreeSet;
use alloc::vec;
use alloc::vec::Vec;

use crate::layout::*;
use crate::read::{Error, Volume};
use crate::{be32, checksum_ok, BlockSource};

/// A volume's allocation bitmap, read whole.
///
/// Held as the bits came off the disk — one bit per block, 1 meaning
/// free — with the accessors doing the inversion once so no caller has
/// to remember it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    reserved: u64,
    block_count: u64,
    valid: bool,
    /// Packed bits for blocks `reserved..block_count`, 1 = free, in the
    /// disk's LSB-first order.
    bits: Vec<u32>,
    pages: Vec<u64>,
    ext_blocks: Vec<u64>,
    /// Pages whose checksum failed, in page order: their LBA, kept for
    /// reporting.
    ///
    /// `bits` still has a placeholder word range for each — all zero, so
    /// they read as "allocated" and an allocator built on this bitmap
    /// never hands one out — but nothing here treats that placeholder as
    /// a fact about the disk. [`Bitmap::covers`] excludes their block
    /// range, so every accessor built on it (`is_free`, `covered`,
    /// `covered_count`, `allocated_count`, `free_count`) answers "cannot
    /// assess" for those blocks rather than a wrong yes or no.
    bad_pages: Vec<u64>,
    /// The block ranges [`Bitmap::bad_pages`] covers, kept apart from the
    /// LBA list because membership is checked by block number.
    bad_ranges: Vec<core::ops::Range<u64>>,
}

impl Bitmap {
    /// Is the root's `bitmap_flag` −1?
    ///
    /// **False means do not trust anything else in here.** The bits are
    /// still readable — a recovery tool wants to see what the interrupted
    /// update left behind — but they are not a statement about what is
    /// allocated, and an allocator that acts on them will hand out blocks
    /// a file is using.
    pub fn valid(&self) -> bool {
        self.valid
    }

    /// The first block the bitmap covers: the volume's `reserved` count,
    /// normally 2. Blocks below it are the boot blocks, which have no bit
    /// because they are not allocatable.
    pub fn first_block(&self) -> u64 {
        self.reserved
    }

    /// One past the last block the bitmap covers.
    pub fn end_block(&self) -> u64 {
        self.block_count
    }

    /// Does the bitmap have a bit for this block? False for the boot
    /// blocks and for anything past the end of the volume.
    pub fn covers(&self, lba: u64) -> bool {
        (self.reserved..self.block_count).contains(&lba)
            && (lba - self.reserved) / 32 < self.bits.len() as u64
            && !self.bad_ranges.iter().any(|r| r.contains(&lba))
    }

    /// The bitmap pages whose checksum did not sum: their own block, not
    /// a block they describe.
    ///
    /// The blocks each one *would* have described are not "free" or
    /// "allocated" — [`Bitmap::covers`] says so for every one of them —
    /// they are simply not answerable, same as a block past the end of a
    /// short bitmap.
    pub fn bad_pages(&self) -> &[u64] {
        &self.bad_pages
    }

    /// Is `lba` marked free? `None` for a block the bitmap does not cover.
    pub fn is_free(&self, lba: u64) -> Option<bool> {
        if !self.covers(lba) {
            return None;
        }
        let i = lba - self.reserved;
        Some(self.bits[(i / 32) as usize] >> (i % 32) & 1 == 1)
    }

    /// Is `lba` marked allocated? `None` for a block the bitmap does not
    /// cover — which is deliberately not `Some(true)`: the boot blocks
    /// *are* in use, but the bitmap does not say so, and a validator
    /// needs to tell "in use" from "not answerable".
    pub fn is_allocated(&self, lba: u64) -> Option<bool> {
        self.is_free(lba).map(|free| !free)
    }

    /// Every block the bitmap marks allocated, ascending.
    pub fn allocated(&self) -> impl Iterator<Item = u64> + '_ {
        self.covered()
            .filter(move |&lba| self.is_free(lba) == Some(false))
    }

    /// Every block the bitmap marks free, ascending.
    pub fn free(&self) -> impl Iterator<Item = u64> + '_ {
        self.covered()
            .filter(move |&lba| self.is_free(lba) == Some(true))
    }

    /// Every block the bitmap has a bit for, ascending.
    pub fn covered(&self) -> impl Iterator<Item = u64> + '_ {
        let end = self
            .block_count
            .min(self.reserved + self.bits.len() as u64 * 32);
        (self.reserved..end).filter(move |&lba| self.covers(lba))
    }

    /// How many blocks are marked allocated.
    ///
    /// Counted by popcount over the words rather than by walking blocks,
    /// because on a 2 GB volume the difference is four million iterations.
    pub fn allocated_count(&self) -> u64 {
        self.covered_count() - self.free_count()
    }

    /// How many blocks are marked free.
    pub fn free_count(&self) -> u64 {
        // A bad page breaks the "covered range is a contiguous prefix"
        // assumption the word-popcount fast path relies on, so a volume
        // with one falls back to counting through `covers` -- rare
        // enough (a validator's own recovery path) that the fast path
        // stays fast for every volume that never hits it.
        if !self.bad_ranges.is_empty() {
            return self.free().count() as u64;
        }
        let covered = self.covered_count();
        let mut n = 0u64;
        for (i, w) in self.bits.iter().enumerate() {
            let base = i as u64 * 32;
            // The last word may run past the end of the volume; those
            // bits are padding, whatever value they hold.
            let usable = covered.saturating_sub(base).min(32);
            if usable == 0 {
                break;
            }
            let mask = if usable == 32 {
                u32::MAX
            } else {
                (1u32 << usable) - 1
            };
            n += (w & mask).count_ones() as u64;
        }
        n
    }

    /// How many blocks the bitmap has bits for.
    ///
    /// A bad page's block range does not count as covered: its bits are
    /// unknown, not free or allocated, so a validator relying on this to
    /// know what fraction of the volume it could actually assess must not
    /// be told a broken page's blocks were checked.
    pub fn covered_count(&self) -> u64 {
        let base = self
            .block_count
            .saturating_sub(self.reserved)
            .min(self.bits.len() as u64 * 32);
        let bad: u64 = self
            .bad_ranges
            .iter()
            .map(|r| r.end.saturating_sub(r.start))
            .sum();
        base.saturating_sub(bad)
    }

    /// Does the bitmap have enough pages to cover the whole volume? A
    /// short bitmap is not a corrupt one — it is a volume whose last
    /// blocks can never be allocated, which is a real (and reported)
    /// state rather than an error.
    pub fn covers_whole_volume(&self) -> bool {
        self.reserved + self.covered_count() >= self.block_count
    }

    /// The bits exactly as they came off the disk: **1 = free**,
    /// LSB-first within each longword, `bits[0]` bit 0 being block
    /// [`Bitmap::first_block`].
    ///
    /// Raw rather than inverted, because the one consumer that wants them
    /// whole is [`Allocator`](crate::allocator::Allocator), which writes them
    /// back — and a representation that changed sense between reading and
    /// writing is the inversion bug this module exists to prevent.
    pub fn words(&self) -> &[u32] {
        &self.bits
    }

    /// The bitmap blocks themselves, in page order. They are allocated
    /// blocks like any other — and a validator that forgets to count them
    /// reachable reports every one as an orphan.
    pub fn pages(&self) -> &[u64] {
        &self.pages
    }

    /// The bitmap extension blocks walked to find the pages past the
    /// root's 25, in chain order.
    pub fn ext_blocks(&self) -> &[u64] {
        &self.ext_blocks
    }
}

impl<S: BlockSource> Volume<S> {
    /// Read the whole allocation bitmap: the root's 25 page pointers,
    /// then as many extension blocks as the volume's size needs.
    ///
    /// A zero page pointer ends the list — the format packs them from the
    /// front — so a volume needing fewer than 25 pages simply stops. Each
    /// page's checksum is verified at [`BITMAP_CHECKSUM_INDEX`]; a page
    /// whose checksum fails does **not** abort the read. Its own LBA goes
    /// into [`Bitmap::bad_pages`] and its block range is excluded from
    /// [`Bitmap::covers`] — bits that do not sum are not bits to allocate
    /// from, but they are also not a reason to stop reading every other
    /// page's bits. Only a failure reading the page at all (a transport
    /// error, not a checksum mismatch) is still an `Err` here.
    ///
    /// An invalid `bitmap_flag` is **not** an error here. The bits are
    /// read and returned with [`Bitmap::valid`] false, because the state
    /// a recovery tool most needs to inspect is exactly the one an error
    /// return would hide.
    pub fn read_bitmap(&mut self) -> Result<Bitmap, Error<S::Error>> {
        let bs = self.block_size();
        let reserved = self.reserved();
        let block_count = self.block_count();
        let valid = self.root().bitmap_flag == -1;

        let mut pages: Vec<u64> = Vec::new();
        let mut ext_blocks: Vec<u64> = Vec::new();
        for &p in self.root().bitmap_pages.iter() {
            if p == 0 {
                break;
            }
            pages.push(p as u64);
        }

        // Only walk the extension chain if the root's pages were all
        // used: an extension pointer beside a half-empty root page list
        // is a leftover, and following it would invent coverage.
        let mut next = if pages.len() == BITMAP_PAGES {
            self.root().bitmap_ext
        } else {
            0
        };
        let per_ext = bitmap_ext_pointers(bs);
        let mut visited: BTreeSet<u64> = BTreeSet::new();
        while next != 0 {
            let lba = next as u64;
            self.guard_chain(&mut visited, lba)?;
            // A bitmap extension block has no type, no own key and no
            // checksum: there is nothing in it to verify, only pointers
            // to range-check as they are used.
            self.read_raw(lba)?;
            ext_blocks.push(lba);
            for i in 0..per_ext {
                let p = be32(&self.buf, i * 4);
                if p == 0 {
                    break;
                }
                pages.push(p as u64);
            }
            next = be32(&self.buf, bitmap_ext_next(bs));
        }

        let per_page = bitmap_bits_per_block(bs);
        let words_per_page = (bs - OFF_BITMAP_BITS) / 4;
        let need = block_count.saturating_sub(reserved);
        let mut bits: Vec<u32> =
            Vec::with_capacity((pages.len() * words_per_page).min((need / 32 + 1) as usize));
        let mut bad_pages: Vec<u64> = Vec::new();
        let mut bad_ranges: Vec<core::ops::Range<u64>> = Vec::new();
        for (i, &page) in pages.iter().enumerate() {
            // Stop reading pages the volume has no blocks for, however
            // many the root lists: a bitmap longer than the volume is a
            // formatting artefact, not coverage.
            if i as u64 * per_page >= need {
                break;
            }
            self.read_raw(page)?;
            if !checksum_ok(&self.buf) {
                // A checksum failure is proof about *this page only*: the
                // rest of the bitmap, and the whole reachability walk
                // this feeds into, is not less true for it. Record the
                // page and its block range as unreadable and keep going
                // -- an aborted read here used to take down the entire
                // bitmap-analysis phase of `validate()`, silently
                // skipping orphan/reachable-but-free detection for every
                // OTHER page, intact or not.
                let start = reserved + i as u64 * per_page;
                let end = (start + per_page).min(block_count);
                bad_pages.push(page);
                bad_ranges.push(start..end);
                // Placeholder words, so later pages' block-to-word
                // indexing stays correct. `Bitmap::covers` excludes this
                // page's range, so these zeros are never read as "this
                // block is allocated" -- only as words to skip past.
                bits.extend(core::iter::repeat(0u32).take(words_per_page));
                continue;
            }
            for w in 0..words_per_page {
                bits.push(be32(&self.buf, OFF_BITMAP_BITS + w * 4));
            }
        }

        Ok(Bitmap {
            reserved,
            block_count,
            valid,
            bits,
            pages,
            ext_blocks,
            bad_pages,
            bad_ranges,
        })
    }
}

/// Build a bitmap in memory from an explicit allocated set — the shape
/// [`Volume::validate`](crate::validate) compares against, and the shape
/// a formatter will write.
///
/// Present here rather than in a writer because the bit conventions
/// belong in one place: whatever writes a bitmap must invert, order and
/// offset exactly the way [`Bitmap`] reads, and the cheapest way to
/// guarantee that is for both to be this module's problem.
pub fn pack_bits(
    reserved: u64,
    block_count: u64,
    allocated: &[u64],
    block_size: usize,
) -> Vec<u32> {
    let words_per_page = (block_size - OFF_BITMAP_BITS) / 4;
    let per_page = bitmap_bits_per_block(block_size);
    let need = block_count.saturating_sub(reserved);
    let pages = (need / per_page + u64::from(need % per_page != 0)) as usize;
    // Every bit starts free, which is also what the padding bits past the
    // end of the volume are left as.
    let mut words = vec![u32::MAX; pages * words_per_page];
    for &lba in allocated {
        clear(&mut words, reserved, lba);
    }
    words
}

/// The same packing, from *ranges* of allocated blocks rather than a list.
///
/// A populator allocates with a cursor, so what it has in hand at the end
/// is two runs — the format's own metadata and everything handed out
/// since — not a list. Enumerating a 2 GB volume's four million allocated
/// blocks into a `Vec` just to set their bits would cost 32 MB to say
/// what two `Range`s already say. Overlapping ranges are fine: clearing a
/// bit twice clears it once.
pub fn pack_ranges(
    reserved: u64,
    block_count: u64,
    ranges: &[core::ops::Range<u64>],
    block_size: usize,
) -> Vec<u32> {
    let words_per_page = (block_size - OFF_BITMAP_BITS) / 4;
    let per_page = bitmap_bits_per_block(block_size);
    let need = block_count.saturating_sub(reserved);
    let pages = (need / per_page + u64::from(need % per_page != 0)) as usize;
    let mut words = vec![u32::MAX; pages * words_per_page];
    for range in ranges {
        for lba in range.start.max(reserved)..range.end.min(block_count) {
            clear(&mut words, reserved, lba);
        }
    }
    words
}

/// Mark one block allocated: clear its bit, because **1 is free**.
fn clear(words: &mut [u32], reserved: u64, lba: u64) {
    if lba < reserved {
        return;
    }
    let i = lba - reserved;
    if let Some(w) = words.get_mut((i / 32) as usize) {
        *w &= !(1u32 << (i % 32));
    }
}
