//! Creating a volume: the boot block, the root block and the bitmap that
//! together make a run of blocks a mountable, empty filesystem.
//!
//! What `Format` does, minus the icon. [`format`](format()) writes the
//! smallest set of blocks that constitutes a valid volume of any of the
//! eight variants at any block size, and nothing else: the blocks it does
//! not write are marked free and left exactly as they were. Formatting a
//! 2 GB image therefore touches a few hundred blocks, not four million,
//! which is also what the real thing does — a `Format` that zeroed the
//! medium would take all afternoon and buy nothing, because a free block
//! is never read.
//!
//! # Where everything goes, and how that was decided
//!
//! The root is at [`canonical_root_lba`] — recomputed from geometry on
//! every mount, so it is not a choice so much as an arithmetic result.
//! Everything else *is* a choice, and each one below was made by
//! formatting an image with xdftool (amitools, GPL — run as an oracle,
//! never copied) and reading the bytes back out:
//!
//! - **Bitmap pages sit immediately after the root**, contiguous and
//!   ascending. A `DOS\1` ADF xdftool formats has its single page at 881
//!   with the root at 880; a 10 MB image has its six pages at 10241..10246
//!   with the root at 10240.
//! - **Bitmap extension blocks come *before* the pages**, filling the
//!   gap between the root and the first page. A 400 MB image (819 200
//!   blocks, 202 pages needed) puts extension blocks at root+1 and root+2,
//!   the root's own 25 page pointers at root+3..root+27, and the remaining
//!   177 pages after them — the first extension block holding 127 and the
//!   second the last 50. This crate lays down the identical shape.
//! - **`DOS\4`/`DOS\5` roots get a dircache block at birth.** A freshly
//!   formatted `DOS\5` ADF carries a `T_DIRCACHE` block with zero records
//!   whose parent is the root, and the root's longword −2 points at it.
//!   An empty root on a dircache volume is *not* one with a null pointer.
//!   Its *position* is where this crate diverges: xdftool's allocator
//!   returns the lowest free bit of the bitmap longword the root falls in
//!   (866 for a root at 880; root−30 for the two larger images), which is
//!   an artefact of how that allocator scans and not a property of the
//!   format. This crate puts it in the next block after the last bitmap
//!   page, which is contiguous, deterministic, and the same rule the
//!   bitmap blocks follow.
//!
//! # The boot block, and the checksum that is deliberately not written
//!
//! Block 0 gets the dostype in longword 0 and the root block's LBA in
//! longword 2, as xdftool writes it and as `struct BootBlock` defines it.
//! Longword 1 — the checksum — is left **zero** unless
//! [`FormatOptions::boot_checksum`] says otherwise, and that default is
//! the interesting decision. A zero checksum does not fail to be valid by
//! accident: it is what makes the boot block *non-bootable*, and a
//! non-bootable boot block is what `Format` produces (`Install` is the
//! separate command that writes boot code and a checksum over it). A boot
//! block whose checksum passes and whose code is 1012 zero bytes is
//! strictly worse than one that fails: the ROM would accept it and jump
//! into the zeroes. xdftool leaves it zero too. The option exists because
//! a caller writing its own boot code needs the checksum
//! ([`bootblock_checksum`](crate::bootblock_checksum())) applied to the
//! result, and it is documented as the footgun it is.
//!
//! The boot *area* is 1024 bytes — two 512-byte sectors, which is what
//! the ROM reads, a fixed count that has nothing to do with the
//! filesystem's block size. At 512-byte blocks it spans blocks 0 and 1;
//! at 1 KB and above it lives inside block 0 alone, and the remaining
//! reserved blocks are simply zeroed. No oracle was available for that
//! last claim — xdftool's images are 512-blocked and larger block sizes
//! live behind an RDB — so it is stated as the inference it is. Nothing
//! reads past 1024 bytes for a checksum this crate leaves at zero anyway.
//!
//! # Order of writes
//!
//! Bitmap pages and extension blocks first, then the dircache, then the
//! root, then the boot block. An interruption therefore leaves a volume
//! that does not mount, rather than one that mounts with a root pointing
//! at blocks that were never written — the same failure direction
//! milestone 3's mutation ordering exists to preserve.

use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

use crate::bitmap::pack_bits;
use crate::layout::*;
use crate::read::{canonical_root_lba, DateStamp, DEFAULT_RESERVED};
use crate::{bootblock_checksum, checksum_compute, hash_table_size};
use crate::{BlockSink, Variant, MAX_NAME_CLASSIC};

/// Bytes the boot area occupies: two 512-byte sectors, because that is
/// what the ROM reads, regardless of the filesystem's block size.
pub const BOOT_AREA_LEN: usize = 1024;

/// Boot block, longword 2: the root block's LBA. Advisory — every mount
/// recomputes the root from geometry — but it is part of the structure
/// and both `Format` and xdftool fill it in.
///
/// `pub(crate)` rather than private: [`crate::resize`] rewrites this
/// longword too, when the root moves, for the same "advisory but still
/// worth keeping honest" reason.
pub(crate) const OFF_BOOT_ROOT: usize = 8;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Everything [`format`](format()) refuses, and why.
///
/// Generic over the [`BlockSink`]'s error for the same reason
/// [`crate::read::Error`] is generic over the source's: "why did the
/// write fail" is a question only the transport can answer, and a
/// flattened error type throws the answer away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError<E> {
    /// The underlying block sink failed.
    Io(E),
    /// A block size with no defined hash-table size: not a power of two
    /// in 512..=32768.
    BadBlockSize(usize),
    /// `reserved` is not smaller than the volume — there would be no
    /// filesystem left to format.
    BadReserved {
        /// Blocks reserved at the front.
        reserved: u64,
        /// Blocks in the volume.
        block_count: u64,
    },
    /// `reserved` is too small to hold this block size's boot area
    /// ([`BOOT_AREA_LEN`] bytes).
    ///
    /// `reserved == 0` is the sharpest shape of this: the boot-area write
    /// loop is `for lba in 0..reserved`, so it writes *nothing* — the
    /// dostype never reaches block 0, `Volume::open` cannot identify the
    /// volume it just wrote, and block 0 is simultaneously covered by
    /// the bitmap as an ordinary allocatable block, so a later
    /// allocation can hand it out and overwrite whatever a caller
    /// expected to find there. A `reserved` that covers only part of the
    /// boot area (one block at 512 bytes, where the area is two sectors)
    /// has the same problem with the second sector.
    ReservedTooSmall {
        /// What the caller asked for.
        reserved: u64,
        /// The fewest blocks this block size's boot area needs.
        min_reserved: u64,
    },
    /// The volume has no room for the metadata an empty filesystem needs:
    /// a root block, at least one bitmap page, and on `DOS\4`/`DOS\5` a
    /// dircache block.
    VolumeTooSmall {
        /// Blocks in the volume.
        block_count: u64,
        /// Blocks reserved at the front.
        reserved: u64,
        /// Blocks the layout needs, counting from block 0.
        needed: u64,
    },
    /// A volume with more blocks than a 32-bit block pointer can name.
    /// Every pointer in the format — hash slots, bitmap pages, parents —
    /// is one longword, so this is the format's wall and not this
    /// crate's.
    VolumeTooLarge {
        /// The block count asked for.
        block_count: u64,
    },
    /// The sink says it is smaller than the volume being formatted.
    /// Checked before the first block is written, which is the whole
    /// reason [`BlockSink::block_count`] exists.
    SinkTooSmall {
        /// Blocks the volume claims.
        block_count: u64,
        /// Blocks the sink reports.
        sink_blocks: u64,
    },
    /// An empty volume name. AmigaDOS cannot address a volume without
    /// one, so this is refused rather than written.
    NameEmpty,
    /// A volume name longer than the root block's 30-byte field — which
    /// is 30 on *every* variant, long-name ones included: only directory
    /// entries got the merged 112-byte field, and the root's name field
    /// did not move.
    NameTooLong {
        /// The length offered.
        len: usize,
        /// The maximum, [`MAX_NAME_CLASSIC`].
        max: usize,
    },
    /// A byte AmigaDOS cannot have in a name: `:` and `/` are path
    /// syntax, and control characters are unaddressable from a Shell.
    /// Anything else — the whole of printable Latin-1 — is allowed,
    /// because the name is Latin-1 and not this crate's to reinterpret.
    NameInvalidByte {
        /// The offending byte.
        byte: u8,
        /// Where it was.
        index: usize,
    },
}

impl<E: fmt::Display> fmt::Display for FormatError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "block write failed: {e}"),
            Self::BadBlockSize(n) => {
                write!(f, "block size {n} is not a power of two in 512..=32768")
            }
            Self::BadReserved {
                reserved,
                block_count,
            } => write!(
                f,
                "{reserved} reserved blocks leaves nothing of a {block_count}-block volume"
            ),
            Self::ReservedTooSmall {
                reserved,
                min_reserved,
            } => write!(
                f,
                "{reserved} reserved blocks cannot hold the boot area; {min_reserved} are needed"
            ),
            Self::VolumeTooSmall {
                block_count,
                reserved,
                needed,
            } => write!(
                f,
                "a {block_count}-block volume with {reserved} reserved cannot hold an empty \
                 filesystem: {needed} blocks are needed"
            ),
            Self::VolumeTooLarge { block_count } => write!(
                f,
                "{block_count} blocks exceeds the 32-bit block pointers the format has"
            ),
            Self::SinkTooSmall {
                block_count,
                sink_blocks,
            } => write!(
                f,
                "formatting {block_count} blocks onto a sink of {sink_blocks}"
            ),
            Self::NameEmpty => f.write_str("a volume name is required"),
            Self::NameTooLong { len, max } => {
                write!(f, "volume name of {len} bytes exceeds {max}")
            }
            Self::NameInvalidByte { byte, index } => write!(
                f,
                "byte {byte:#04x} at index {index} cannot appear in a volume name"
            ),
        }
    }
}

#[cfg(feature = "std")]
impl<E: std::error::Error + 'static> std::error::Error for FormatError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Options and layout
// ---------------------------------------------------------------------------

/// What to format, and with what.
///
/// The creation date is the caller's to supply rather than this crate's
/// to read: there is no clock in a `no_std` crate, and a formatter that
/// stamped "now" would make every test of its output non-deterministic.
/// [`DateStamp::default`] — 1978-01-01 00:00 — is a perfectly valid date
/// to leave on a volume, and is what a caller with no clock should use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatOptions<'a> {
    /// Which of `DOS\0`–`DOS\7` to write.
    pub variant: Variant,
    /// Blocks in the volume. This is the *filesystem's* extent, in
    /// filesystem blocks — for a partition, `(high_cyl - low_cyl + 1) *
    /// heads * sectors` scaled to the block size, which is the RDB's
    /// arithmetic and the caller's to have done.
    pub block_count: u64,
    /// Blocks reserved at the front, before the bitmap's first bit.
    /// [`DEFAULT_RESERVED`] on every volume anyone has made; it is a
    /// mount parameter (`de_Reserved`), so it is a parameter here.
    pub reserved: u64,
    /// The volume name, raw Latin-1, 1..=30 bytes, no `:` or `/`.
    pub name: &'a [u8],
    /// The stamp written to all three of the root's dates: `dir_altered`,
    /// `disk_altered` and `disk_made`. xdftool writes one stamp to all
    /// three too — at format time they are the same instant, and
    /// pretending otherwise would be inventing history.
    pub created: DateStamp,
    /// Write a valid boot-block checksum. **Leave this false** unless
    /// the caller is also writing boot code: a boot block that checksums
    /// correctly and contains no code is one the ROM will accept and
    /// jump into. See this module's documentation.
    pub boot_checksum: bool,
}

impl<'a> FormatOptions<'a> {
    /// The usual case: a variant, a size, a name, [`DEFAULT_RESERVED`]
    /// reserved blocks, an epoch creation date and no boot checksum.
    pub fn new(variant: Variant, block_count: u64, name: &'a [u8]) -> Self {
        Self {
            variant,
            block_count,
            reserved: DEFAULT_RESERVED,
            name,
            created: DateStamp::default(),
            boot_checksum: false,
        }
    }

    /// Set the creation date (all three root stamps).
    pub fn created(mut self, created: DateStamp) -> Self {
        self.created = created;
        self
    }

    /// Set the reserved-block count.
    pub fn reserved(mut self, reserved: u64) -> Self {
        self.reserved = reserved;
        self
    }
}

/// Where [`format`](format()) put everything.
///
/// Returned rather than merely written because the caller frequently
/// needs it: a partition builder wants to know the volume's used-block
/// count, a test wants to assert the bitmap marks exactly these blocks,
/// and a resize wants the shape it is about to move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatLayout {
    /// The root block.
    pub root_lba: u64,
    /// The bitmap extension blocks, in chain order. Empty when the
    /// root's own [`BITMAP_PAGES`] pointers were enough.
    pub bitmap_ext: Vec<u64>,
    /// The bitmap pages, in coverage order: page *n* covers blocks
    /// `reserved + n * bitmap_bits_per_block(bs)` onward.
    pub bitmap_pages: Vec<u64>,
    /// The root's `T_DIRCACHE` block on `DOS\4`/`DOS\5`, `None`
    /// elsewhere.
    pub dircache: Option<u64>,
}

impl FormatLayout {
    /// Every block the format allocated, ascending — exactly the set the
    /// bitmap marks in use, and exactly what a walk from the root will
    /// reach.
    pub fn allocated(&self) -> Vec<u64> {
        let mut v = Vec::with_capacity(2 + self.bitmap_ext.len() + self.bitmap_pages.len());
        v.push(self.root_lba);
        v.extend_from_slice(&self.bitmap_ext);
        v.extend_from_slice(&self.bitmap_pages);
        v.extend(self.dircache);
        v.sort_unstable();
        v
    }

    /// How many blocks the format allocated — the root's LNFS
    /// `NumBlocksUsed` field, and the number a "0% full" volume reports
    /// as used.
    pub fn blocks_used(&self) -> u64 {
        1 + self.bitmap_ext.len() as u64
            + self.bitmap_pages.len() as u64
            + u64::from(self.dircache.is_some())
    }
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

/// Create a fresh, empty, valid volume.
///
/// Takes a [`BlockSink`] alone: formatting writes and never reads, so
/// requiring `BlockSource` too would exclude a write-only target for
/// nothing. Verifying the result — [`Volume::open_with`](crate::Volume)
/// followed by [`validate`](crate::Volume::validate) — is a separate step
/// for whoever also holds a source, and this crate's own test suite does
/// exactly that for every variant at every block size.
///
/// The block size comes from the sink. Every metadata block is written
/// zeroed-then-filled, so a reused image cannot leak old bytes into a
/// field this function does not set.
pub fn format<S: BlockSink>(
    sink: &mut S,
    opts: &FormatOptions<'_>,
) -> Result<FormatLayout, FormatError<S::Error>> {
    let bs = sink.block_size();
    if !block_size_ok(bs) {
        return Err(FormatError::BadBlockSize(bs));
    }
    let FormatOptions {
        variant,
        block_count,
        reserved,
        name,
        created,
        boot_checksum,
    } = *opts;

    check_name(name)?;
    if block_count > u64::from(u32::MAX) {
        return Err(FormatError::VolumeTooLarge { block_count });
    }
    // The boot area is two 512-byte sectors' worth of bytes,
    // [`BOOT_AREA_LEN`], regardless of the filesystem's own block size;
    // `reserved` blocks have to be enough to hold all of it, or the
    // dostype (and the boot-area write below, `for lba in 0..reserved`)
    // never reaches the disk at all when `reserved == 0`, and reaches it
    // only partly otherwise.
    let min_reserved = div_ceil(BOOT_AREA_LEN as u64, bs as u64);
    if reserved < min_reserved {
        return Err(FormatError::ReservedTooSmall {
            reserved,
            min_reserved,
        });
    }
    if reserved >= block_count {
        return Err(FormatError::BadReserved {
            reserved,
            block_count,
        });
    }
    if let Some(sink_blocks) = sink.block_count() {
        if sink_blocks < block_count {
            return Err(FormatError::SinkTooSmall {
                block_count,
                sink_blocks,
            });
        }
    }

    let layout = plan(bs, variant, block_count, reserved)?;

    // Bitmap first: the root is about to claim these blocks, and a root
    // that claims a block nobody wrote is the one failure shape worth
    // ruling out by ordering alone.
    let allocated = layout.allocated();
    let words = pack_bits(reserved, block_count, &allocated, bs);
    let words_per_page = (bs - OFF_BITMAP_BITS) / 4;
    let mut buf = vec![0u8; bs];
    for (i, &page) in layout.bitmap_pages.iter().enumerate() {
        buf.iter_mut().for_each(|b| *b = 0);
        for w in 0..words_per_page {
            let v = words
                .get(i * words_per_page + w)
                .copied()
                .unwrap_or(u32::MAX);
            wr32(&mut buf, OFF_BITMAP_BITS + w * 4, v);
        }
        // Longword 0, not longword 5. A bitmap block has no type and no
        // own key; the checksum is its whole header, and a writer that
        // uses index 5 here produces blocks that verify against
        // themselves and against nothing else.
        let ck = checksum_compute(&buf, BITMAP_CHECKSUM_INDEX);
        wr32(&mut buf, BITMAP_CHECKSUM_INDEX * 4, ck);
        put(sink, page, &buf)?;
    }

    // Extension blocks: bare pointer arrays. No type, no own key, no
    // checksum — there is nothing in one to verify, so there is nothing
    // in one to compute.
    let per_ext = bitmap_ext_pointers(bs);
    for (i, &ext) in layout.bitmap_ext.iter().enumerate() {
        buf.iter_mut().for_each(|b| *b = 0);
        let first = BITMAP_PAGES + i * per_ext;
        for (k, &page) in layout
            .bitmap_pages
            .iter()
            .skip(first)
            .take(per_ext)
            .enumerate()
        {
            wr32(&mut buf, k * 4, page as u32);
        }
        let next = layout.bitmap_ext.get(i + 1).copied().unwrap_or(0);
        wr32(&mut buf, bitmap_ext_next(bs), next as u32);
        put(sink, ext, &buf)?;
    }

    // The root's dircache, on the two variants that have one: present
    // from birth with zero records, which is what a formatted `DOS\5`
    // volume actually contains.
    if let Some(dc) = layout.dircache {
        buf.iter_mut().for_each(|b| *b = 0);
        wr32(&mut buf, OFF_TYPE, T_DIRCACHE);
        wr32(&mut buf, OFF_OWN_KEY, dc as u32);
        wr32(&mut buf, OFF_DIRCACHE_PARENT, layout.root_lba as u32);
        wr32(&mut buf, OFF_DIRCACHE_RECORDS, 0);
        wr32(&mut buf, OFF_DIRCACHE_NEXT, 0);
        let ck = checksum_compute(&buf, CHECKSUM_INDEX);
        wr32(&mut buf, OFF_CHECKSUM, ck);
        put(sink, dc, &buf)?;
    }

    // The root.
    buf.iter_mut().for_each(|b| *b = 0);
    wr32(&mut buf, OFF_TYPE, T_HEADER);
    // Longword 1 is the block's own key everywhere else and zero here:
    // the root is nobody's entry, and every implementation that checks it
    // checks it against zero.
    wr32(&mut buf, OFF_OWN_KEY, 0);
    wr32(&mut buf, OFF_HASH_TABLE_SIZE, hash_table_size(bs));
    // The hash table stays zeroed: an empty directory is an empty table.
    wr32(&mut buf, tail(bs, TL_BITMAP_FLAG), (-1i32) as u32);
    for (i, &page) in layout.bitmap_pages.iter().take(BITMAP_PAGES).enumerate() {
        wr32(&mut buf, tail(bs, TL_BITMAP_PAGES) + i * 4, page as u32);
    }
    if let Some(&first_ext) = layout.bitmap_ext.first() {
        wr32(&mut buf, tail(bs, TL_BITMAP_EXT), first_ext as u32);
    }
    wr_date(&mut buf, tail(bs, TL_ROOT_DIR_ALTERED), created);
    wr_bcpl(&mut buf, tail(bs, TL_ROOT_NAME), name);
    wr_date(&mut buf, tail(bs, TL_ROOT_DISK_ALTERED), created);
    wr_date(&mut buf, tail(bs, TL_ROOT_DISK_MADE), created);
    if variant.has_long_names() {
        // Only LNFS roots have these two fields; on a classic root
        // longword −11 is the volume name's padding and longword −4 is
        // reserved, and writing either would be writing into a field the
        // filesystem does not own. (xdftool writes the dostype at −4 on
        // every variant; harmless, but not what the format says.)
        wr32(
            &mut buf,
            tail(bs, TL_ROOT_NUM_BLOCKS_USED),
            layout.blocks_used() as u32,
        );
        wr32(&mut buf, tail(bs, TL_ROOT_FS_TYPE), variant.dostype());
    }
    if let Some(dc) = layout.dircache {
        wr32(&mut buf, tail(bs, TL_EXTENSION), dc as u32);
    }
    wr32(&mut buf, tail(bs, TL_SECONDARY_TYPE), ST_ROOT as u32);
    let ck = checksum_compute(&buf, CHECKSUM_INDEX);
    wr32(&mut buf, OFF_CHECKSUM, ck);
    put(sink, layout.root_lba, &buf)?;

    // The boot area last: until the dostype is down, nothing will try to
    // mount what the earlier writes were still assembling.
    let mut area = vec![0u8; bs.max(BOOT_AREA_LEN)];
    wr32(&mut area, OFF_TYPE, variant.dostype());
    wr32(&mut area, OFF_BOOT_ROOT, layout.root_lba as u32);
    if boot_checksum {
        let ck = bootblock_checksum(&area[..BOOT_AREA_LEN]);
        wr32(&mut area, 4, ck);
    }
    for lba in 0..reserved {
        let off = (lba as usize * bs).min(area.len());
        let end = (off + bs).min(area.len());
        buf.iter_mut().for_each(|b| *b = 0);
        buf[..end - off].copy_from_slice(&area[off..end]);
        put(sink, lba, &buf)?;
    }

    Ok(layout)
}

/// Work out where everything goes, before a byte is written.
///
/// Split out because it is the part with the arithmetic in it, and
/// because "does this volume have room" is a question worth answering
/// without a sink.
fn plan<E>(
    bs: usize,
    variant: Variant,
    block_count: u64,
    reserved: u64,
) -> Result<FormatLayout, FormatError<E>> {
    let too_small = |needed| FormatError::VolumeTooSmall {
        block_count,
        reserved,
        needed,
    };
    let root_lba =
        canonical_root_lba(block_count, reserved).ok_or_else(|| too_small(reserved + 1))?;

    let per_page = bitmap_bits_per_block(bs);
    let need = block_count - reserved;
    let pages = div_ceil(need, per_page);
    let exts = if pages as usize <= BITMAP_PAGES {
        0
    } else {
        div_ceil(pages - BITMAP_PAGES as u64, bitmap_ext_pointers(bs) as u64)
    };

    // Extension blocks first, then pages, then the dircache: the shape
    // xdftool's images have, block for block.
    let first = root_lba + 1;
    let bitmap_ext: Vec<u64> = (first..first + exts).collect();
    let bitmap_pages: Vec<u64> = (first + exts..first + exts + pages).collect();
    let mut next = first + exts + pages;
    let dircache = if variant.has_dircache() {
        next += 1;
        Some(next - 1)
    } else {
        None
    };

    if next > block_count {
        return Err(too_small(next));
    }
    Ok(FormatLayout {
        root_lba,
        bitmap_ext,
        bitmap_pages,
        dircache,
    })
}

/// What is wrong with a name, before anyone has decided whose error type
/// it belongs in.
///
/// The rule is one rule — the volume name and a directory entry's name
/// are checked identically, differing only in the maximum — so it lives
/// in one place and both [`FormatError`] and
/// [`crate::populate::PopulateError`] lift it into their own variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NameProblem {
    Empty,
    TooLong { len: usize, max: usize },
    InvalidByte { byte: u8, index: usize },
}

/// A name AmigaDOS can address: non-empty, at most `max` bytes, and free
/// of the two path-syntax characters and of control codes.
///
/// `max` is [`MAX_NAME_CLASSIC`] for a volume name on *every* variant
/// (the root's name field did not move on LNFS) and for a directory entry
/// on `DOS\0`–`DOS\5`; [`crate::MAX_NAME_LONG`] for an entry on
/// `DOS\6`/`DOS\7`.
pub(crate) fn check_name_bytes(name: &[u8], max: usize) -> Result<(), NameProblem> {
    if name.is_empty() {
        return Err(NameProblem::Empty);
    }
    if name.len() > max {
        return Err(NameProblem::TooLong {
            len: name.len(),
            max,
        });
    }
    for (index, &byte) in name.iter().enumerate() {
        // Latin-1 above 0x7F is a name character and stays one; only the
        // two C0 ranges and the path separators are refused.
        if byte == b':' || byte == b'/' || byte < 0x20 || (0x7F..=0x9F).contains(&byte) {
            return Err(NameProblem::InvalidByte { byte, index });
        }
    }
    Ok(())
}

fn check_name<E>(name: &[u8]) -> Result<(), FormatError<E>> {
    check_name_bytes(name, MAX_NAME_CLASSIC).map_err(|p| match p {
        NameProblem::Empty => FormatError::NameEmpty,
        NameProblem::TooLong { len, max } => FormatError::NameTooLong { len, max },
        NameProblem::InvalidByte { byte, index } => FormatError::NameInvalidByte { byte, index },
    })
}

fn put<S: BlockSink>(sink: &mut S, lba: u64, buf: &[u8]) -> Result<(), FormatError<S::Error>> {
    sink.write_block(lba, buf).map_err(FormatError::Io)
}

pub(crate) fn div_ceil(a: u64, b: u64) -> u64 {
    a / b + u64::from(a % b != 0)
}

pub(crate) fn wr32(block: &mut [u8], off: usize, v: u32) {
    block[off..off + 4].copy_from_slice(&v.to_be_bytes());
}

pub(crate) fn wr_date(block: &mut [u8], off: usize, d: DateStamp) {
    wr32(block, off, d.days);
    wr32(block, off + 4, d.mins);
    wr32(block, off + 8, d.ticks);
}

pub(crate) fn wr_bcpl(block: &mut [u8], off: usize, s: &[u8]) {
    block[off] = s.len() as u8;
    block[off + 1..off + 1 + s.len()].copy_from_slice(s);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The layouts this crate writes, checked against the block numbers
    /// xdftool's own images actually contain.
    #[test]
    fn the_plan_reproduces_the_oracles_block_numbers() {
        // A DD floppy formatted `DOS\1`: root 880, one bitmap page at 881.
        let l = plan::<()>(512, Variant::Ffs, 1760, 2).unwrap();
        assert_eq!(l.root_lba, 880);
        assert_eq!(l.bitmap_pages, [881]);
        assert!(l.bitmap_ext.is_empty());
        assert_eq!(l.dircache, None);
        assert_eq!(l.blocks_used(), 2);

        // The same floppy as `DOS\5`: a dircache block from birth.
        let l = plan::<()>(512, Variant::FfsIntlDircache, 1760, 2).unwrap();
        assert_eq!(l.dircache, Some(882));
        assert_eq!(l.allocated(), [880, 881, 882]);

        // 10 MB: six pages, 10241..=10246, no extension block.
        let l = plan::<()>(512, Variant::FfsIntlDircache, 20480, 2).unwrap();
        assert_eq!(l.root_lba, 10240);
        assert_eq!(l.bitmap_pages, [10241, 10242, 10243, 10244, 10245, 10246]);
        assert!(l.bitmap_ext.is_empty());

        // 400 MB: 202 pages needed, so two extension blocks at root+1 and
        // root+2 with the pages after them -- the exact shape a 400 MB
        // image xdftool formatted has.
        let l = plan::<()>(512, Variant::FfsIntl, 819_200, 2).unwrap();
        assert_eq!(l.root_lba, 409_600);
        assert_eq!(l.bitmap_ext, [409_601, 409_602]);
        assert_eq!(l.bitmap_pages.len(), 202);
        assert_eq!(l.bitmap_pages[0], 409_603);
        assert_eq!(l.bitmap_pages[BITMAP_PAGES - 1], 409_627);
        assert_eq!(l.bitmap_pages[BITMAP_PAGES], 409_628);
        assert_eq!(*l.bitmap_pages.last().unwrap(), 409_804);
    }

    #[test]
    fn a_volume_with_no_room_for_its_own_metadata_is_refused() {
        // Root at 2, page at 3: four blocks is exactly enough, and three
        // is one short.
        assert_eq!(plan::<()>(512, Variant::Ffs, 4, 2).unwrap().root_lba, 2);
        assert!(matches!(
            plan::<()>(512, Variant::Ffs, 3, 2),
            Err(FormatError::VolumeTooSmall { needed: 4, .. })
        ));
        // The dircache block costs one more, and is not optional.
        assert!(matches!(
            plan::<()>(512, Variant::FfsIntlDircache, 5, 2),
            Err(FormatError::VolumeTooSmall { needed: 6, .. })
        ));
    }

    #[test]
    fn volume_names_are_checked_the_way_amigados_would() {
        assert!(check_name::<()>(b"Workbench").is_ok());
        assert!(check_name::<()>(b"Caf\xE9").is_ok()); // Latin-1 is a name
        assert!(matches!(check_name::<()>(b""), Err(FormatError::NameEmpty)));
        assert!(matches!(
            check_name::<()>(&[b'x'; 31]),
            Err(FormatError::NameTooLong { len: 31, max: 30 })
        ));
        for bad in [b':', b'/', 0x00, 0x0A] {
            assert!(matches!(
                check_name::<()>(&[b'A', bad]),
                Err(FormatError::NameInvalidByte { index: 1, .. })
            ));
        }
    }
}
