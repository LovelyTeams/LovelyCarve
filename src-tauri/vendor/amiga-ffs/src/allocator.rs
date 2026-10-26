//! Block allocation against a volume that already has something in it.
//!
//! [`Populator`](crate::Populator) allocates with a cursor over a *known*
//! free extent and never asks the disk whether a block is free. This
//! module is where that question finally gets asked, and where the answer
//! has to be right: an allocator over an existing volume hands out blocks
//! that other files' metadata is pointing at, and getting it wrong once
//! means two owners for one block and a file that reads as somebody else's
//! bytes.
//!
//! # The policy, and what the real thing does
//!
//! Implemented here: **first fit from a hint, forward, wrapping once**.
//! [`Allocator::allocate_near`] takes a hint LBA, converts it to a bit
//! index, scans upward a longword at a time to the end of the bitmap's
//! coverage, and then wraps to the volume's first allocatable block and
//! scans up to the hint. [`Allocator::allocate`] uses the block after the
//! last one handed out — a rotating cursor — so a caller with no opinion
//! still gets ascending, locally contiguous runs rather than a rescan of
//! the same full region on every call.
//!
//! That is behaviourally what the shipping implementations do, as far as
//! they can be observed:
//!
//! - **Linux `affs`** (`fs/affs/bitmap.c`, GPL — read to describe, never
//!   copied) takes a *goal* block from the caller, defaulting to the
//!   volume's first allocatable block, scans forward within the goal's
//!   bitmap page, moves on to the next page when that one is full, and
//!   wraps to page zero before giving up. It additionally *pre-allocates*
//!   a run of consecutive free bits within the goal's longword and hands
//!   them out one at a time to the same inode. That last part is a
//!   performance trick with no on-disk consequence, and this crate does
//!   not do it: a run handed out but not used would have to be given back
//!   or leaked, and the mark-then-use ordering below is easier to state
//!   when one call marks exactly one block.
//! - **amitools' `xdftool`** (an oracle, run and never copied) scans from
//!   the *bottom* of the bitmap every time: on a `DOS\1` ADF it filled
//!   866..902 for three files, and after deleting the middle one and
//!   writing another it filled the freed 873..879 and 882..883 before
//!   touching anything higher. First fit, with the hint pinned to zero.
//!
//! Any of these is *correct*; the differences are locality, not
//! semantics, and nothing on disk records which was used. What must hold,
//! and what this module's tests assert, is only ever the same three
//! things: never hand out a block the bitmap says is allocated, never hand
//! out the same block twice, and never hand out a block outside
//! `reserved..block_count`.
//!
//! # Mark-then-use, and how the API makes it hard to get backwards
//!
//! The format has no journal, so ordering is the whole of crash safety,
//! and the two directions are not equally bad. A block that is marked
//! allocated and reached by nothing is a **leak**
//! ([`Finding::OrphanBlock`](crate::Finding::OrphanBlock)) — space lost
//! until a validator rebuilds the bitmap, and harmless meanwhile. A block
//! that something reaches while the bitmap says free is a **pending
//! double-allocation**
//! ([`Finding::ReachableButFree`](crate::Finding::ReachableButFree)) — the
//! next allocation hands it to a second owner, and then one of the two
//! files is silently wrong. So:
//!
//! - **Allocating**: mark the bitmap, get the mark to the disk, and only
//!   then write the metadata that points at the block. Crash anywhere in
//!   that order and the worst case is a leak.
//! - **Freeing**: unlink in the metadata first, get *that* to the disk,
//!   and only then clear the bit. Crash anywhere in that order and the
//!   worst case is, again, a leak.
//!
//! Documentation alone would not make the first of those hard to get
//! wrong, so it is in the types. [`Allocator::allocate`] returns an
//! [`Allocation`], not a block number. [`Allocation::block`] gives the LBA
//! to *write the block's own contents at* — always safe, because nothing
//! references it yet — while [`Allocator::reference`] gives the LBA that
//! may be stored in somebody else's pointer, and **refuses**
//! ([`AllocError::NotDurable`]) while the bitmap page holding the bit is
//! still only marked in memory. Between them sits [`Allocator::flush`],
//! which writes the dirty pages and nothing else.
//!
//! Freeing cannot be enforced the same way — this module cannot see the
//! caller's metadata write — so [`Allocator::free`] is documented as the
//! second half of an unlink and refuses the one error it *can* see: a
//! double free ([`AllocError::DoubleFree`]), which is a block already
//! marked free being freed again, and the shape a retried delete takes.
//!
//! # Dirty pages
//!
//! Edits accumulate in memory, one dirty flag per bitmap page, and
//! [`Allocator::flush`] writes only the pages that changed — a session
//! that allocates twenty blocks out of one region writes one block, not
//! twenty. Each page is written with its checksum in longword **0**
//! ([`BITMAP_CHECKSUM_INDEX`]), the format's one exception, and a page's
//! dirty flag is cleared only after its write returns, so an interrupted
//! flush leaves the rest still queued rather than silently dropped.
//!
//! # An invalid bitmap is not an allocation source
//!
//! [`Allocator::load`] refuses a volume whose root says `bitmap_flag == 0`
//! ([`AllocError::BitmapInvalid`]). Those bits are whatever an interrupted
//! update left behind: a validator wants to see them (which is why
//! [`Bitmap`](crate::Bitmap) reads them anyway), and an allocator acting
//! on them would hand out blocks a file is using. Rebuilding is
//! [`repair`](crate::repair)'s job, and it is the one caller here that
//! constructs an allocator from a set it computed itself
//! ([`Allocator::rebuilt`]) rather than from the disk.
//!
//! # Block layout policy
//!
//! `docs/layout-survey.md` (in the repository, not published with the
//! crate) is the evidence base for everything below; citations here are to
//! its section numbers. The policy is stated here, once, because
//! [`Intent`] is where a caller states *what kind* of block it is placing,
//! and the reasoning for why that vocabulary exists belongs next to it.
//!
//! **What goes where, and why.** ReOrg's own author judged directory- and
//! file-header *scatter*, not file-data fragmentation, the dominant
//! real-world cost on FFS/OFS (survey §1) — a conclusion this crate's own
//! measurement corroborates independently: laying one file's data as six
//! runs instead of one cost a real Kickstart 3.1 ROM 47% more wall-clock
//! time reading it off a floppy (survey §4a, `examples/frag-bench.rs`).
//! Two, not one, conclusions follow:
//!
//! - **Data contiguity is real and measured** — [`Intent::DataFor`] and
//!   [`Allocator::allocate_run`] exist so a caller can ask for (up to) a
//!   whole file's extent as one ascending run, rather than one block at a
//!   time from a hint that a colliding allocation could break.
//! - **Metadata locality is the *larger*, and evidence-independent-of-
//!   floppies, effect.** [`Intent::HeaderIn`] and [`Intent::MetadataNearRoot`]
//!   place headers, dircache blocks and comment overflow near a directory
//!   or the root because AROS's `getCacheBlock` — a hold-cache with no
//!   read-ahead, not a prefetch buffer (survey §2) — only pays off when the
//!   working set a directory walk touches is small and close together.
//!   That is a cache-*hit-rate* argument, not a seek argument, so unlike
//!   data contiguity it transfers unconditionally to CF/SD, where there is
//!   no seek cost to save at all (survey §3, the one row of the ranking
//!   table with a "full" mark in every column).
//!
//! Metadata locality does not, however, cover *every* block a file's
//! header reaches. `T_LIST` extension blocks were measured both ways
//! through the same real-ROM rig — clustered near the root with the
//! header, and interleaved into the data run at their natural stream
//! position — and interleaved won (19 s versus 21 s on the identical
//! 391-block file; see the addendum to survey §4a). An extension block
//! is fetched mid-stream, by a reader already reading the file, not once
//! at open the way a header is, so it is [`Intent::DataFor`]'s to place,
//! not [`Intent::HeaderIn`]'s — see [`Populator`](crate::Populator)'s own
//! documentation for the full reasoning.
//!
//! **What this deliberately does not do**, and why doing it would be
//! cargo-cult rather than policy (survey §3, §6a-4):
//!
//! - **No cylinder or track alignment derived from RDB geometry.** The
//!   RDB's `rdb_Cylinders`/`rdb_Heads`/`rdb_Sectors` fields are software-
//!   chosen to multiply out to the reported size on essentially every
//!   medium this crate targets, not tied to physical platters — AmigaOS's
//!   own RDB documentation says so. A real floppy's geometry (11
//!   sectors/track, 22 blocks/cylinder on DD) is a format constant that
//!   never needs reading from anywhere, and floppies do not carry an RDB
//!   to read it from regardless — but nothing here computes cylinder
//!   boundaries for a hard disk or CF/SD image, because there is nothing
//!   physical on the other end of that arithmetic.
//! - **No segregated allocation zones.** [`Intent`] reduces to a
//!   *hint and a preference*, resolved by the same first-fit scan
//!   [`Allocator::allocate_near`] already does — not a partition of the
//!   bitmap into a metadata region and a data region with their own
//!   accounting. A caller with an anchor (a directory's LBA, a file
//!   header's LBA) gets locality by scanning from that anchor; nothing
//!   here reserves address ranges in advance for a class of block, which
//!   would be exactly the fixed-geometry reasoning the previous point
//!   rejects, aimed at content instead of cylinders.
//! - **No free-space policy knob.** ReOrg exposed where a *compaction*
//!   pass leaves reclaimed free space as a user choice (survey §1); this
//!   module allocates blocks, and creation-time free space is simply
//!   "the rest of the bitmap" — a knob for where a compactor leaves its
//!   leftovers belongs to that later piece of work, not to allocation.
//!
//! [`Populator`](crate::Populator) implements the metadata/data split
//! `Intent` makes possible directly, with two independent forward cursors
//! rather than by calling into this module — see its own documentation for
//! why (it has no bitmap to scan a hint against mid-session, only a known
//! free extent and a monotonic frontier). [`Mutator`](crate::Mutator)'s own
//! adoption of `Intent` for its day-to-day writes, and a compaction pass
//! that applies the same policy retroactively, are later work; this module
//! only has to make the vocabulary and the primitives honest today.

use alloc::vec;
use alloc::vec::Vec;
use core::fmt;
use core::marker::PhantomData;

use crate::format::{div_ceil, wr32};
use crate::layout::*;
use crate::read::Volume;
use crate::{checksum_compute, checksum_ok, BlockSink, BlockSource, Variant};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Everything the allocator refuses, and why.
///
/// Generic over the medium's error for the same reason
/// [`crate::read::Error`] and [`crate::FormatError`] are: "why did the
/// block access fail" is a question only the transport can answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllocError<E> {
    /// A write to the medium failed.
    Io(E),
    /// Reading the bitmap failed, with the read side's own refusal kept
    /// whole — a page whose checksum does not balance, a page pointer
    /// outside the volume, a transport failure.
    Read(crate::read::Error<E>),
    /// The root's `bitmap_flag` is 0: the bitmap on disk is mid-update
    /// and says nothing trustworthy about what is free.
    ///
    /// Refused rather than worked around, because the workaround is a
    /// whole operation with its own name: [`repair`](crate::repair)
    /// rebuilds the bitmap from a reachability walk and stamps the flag
    /// back, and *then* there is something to allocate from.
    BitmapInvalid,
    /// A block size with no defined bitmap geometry: not a power of two
    /// in 512..=32768, or a sink disagreeing with the volume about it.
    BadBlockSize(usize),
    /// Every block the bitmap covers is allocated.
    VolumeFull {
        /// Blocks in the volume.
        block_count: u64,
    },
    /// A block the bitmap has no bit for: below `reserved` (the boot
    /// blocks, which are not allocatable), past the end of the volume, or
    /// past the end of a bitmap too short to cover it.
    NotCovered {
        /// The block.
        lba: u64,
    },
    /// A block freed that was already marked free. The shape a retried
    /// delete takes, and the one half of the free ordering this module
    /// can see for itself.
    DoubleFree {
        /// The block.
        lba: u64,
    },
    /// A freshly allocated block used as a *pointer target* before the
    /// bitmap page carrying its bit reached the disk. See this module's
    /// documentation: metadata that references a block the bitmap still
    /// calls free is exactly the state that turns a crash into a double
    /// allocation.
    NotDurable {
        /// The block.
        lba: u64,
    },
    /// A bitmap page with bits to write and no block to write them to —
    /// a rebuilt bitmap whose pages were not all supplied.
    PageMissing {
        /// Which page (0-based, in coverage order).
        index: usize,
    },
    /// [`Allocator::allocate_exact`] was asked for a block the bitmap
    /// already marks allocated. Distinct from [`AllocError::NotCovered`]
    /// on purpose: `NotCovered` means the volume has no bit for `lba` at
    /// all (outside `reserved..block_count`, or past a short bitmap's
    /// coverage), which is a fact about the volume's *geometry*; this is
    /// a fact about the block's *occupancy* — a caller that gets this back
    /// knows the destination is inside the volume and merely occupied, so
    /// evacuating it first is worth trying, where a caller told
    /// `NotCovered` would be wasting its time.
    AlreadyAllocated {
        /// The block.
        lba: u64,
    },
}

impl<E: fmt::Display> fmt::Display for AllocError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "block write failed: {e}"),
            Self::Read(e) => write!(f, "reading the bitmap failed: {e}"),
            Self::BitmapInvalid => f.write_str(
                "the root's bitmap flag is 0: the bitmap is mid-update and must be repaired \
                 before anything is allocated from it",
            ),
            Self::BadBlockSize(n) => {
                write!(f, "block size {n} is not a power of two in 512..=32768")
            }
            Self::VolumeFull { block_count } => {
                write!(f, "no free block left in {block_count}")
            }
            Self::NotCovered { lba } => {
                write!(f, "block {lba} has no bit in this volume's bitmap")
            }
            Self::DoubleFree { lba } => {
                write!(f, "block {lba} is already free")
            }
            Self::NotDurable { lba } => write!(
                f,
                "block {lba} was allocated but its bitmap page is not on the disk yet -- \
                 flush before writing metadata that points at it"
            ),
            Self::PageMissing { index } => {
                write!(f, "bitmap page {index} has bits to write and no block")
            }
            Self::AlreadyAllocated { lba } => {
                write!(f, "block {lba} is already allocated")
            }
        }
    }
}

#[cfg(feature = "std")]
impl<E: std::error::Error + 'static> std::error::Error for AllocError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Read(e) => Some(e),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Allocation
// ---------------------------------------------------------------------------

/// One block handed out by [`Allocator::allocate`], and the ordering rule
/// that comes with it.
///
/// Deliberately not a `u64`. The block number is safe to *write into*
/// immediately ([`Allocation::block`]) — nothing on the volume references
/// it, so an interrupted write leaves an orphan and nothing worse — and it
/// is only safe to *point at* once the bitmap page carrying its bit is on
/// the disk, which is what [`Allocator::reference`] checks. A plain `u64`
/// would make those two indistinguishable at the call site, which is
/// precisely where the distinction has to be visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Allocation {
    lba: u64,
    page: usize,
}

impl Allocation {
    /// The block's LBA, for writing the block's *own contents*.
    ///
    /// Always safe: until something points at this block it is
    /// unreachable, and an unreachable allocated block is a leak — the
    /// recoverable direction.
    pub fn block(self) -> u64 {
        self.lba
    }

    /// Which bitmap page carries this block's bit.
    pub fn page_index(self) -> usize {
        self.page
    }
}

// ---------------------------------------------------------------------------
// Placement intent
// ---------------------------------------------------------------------------

/// Where a block being allocated *wants* to sit, relative to something
/// already on the volume.
///
/// See this module's "Block layout policy" section for the reasoning.
/// Every variant reduces to a starting hint (or none, for the rotating
/// cursor) fed to the same first-fit-and-wrap scan
/// [`Allocator::allocate_near`] already does — this is a vocabulary over
/// that one mechanism, not a second allocation strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// File content, following its own header — a data block, or a
    /// `T_LIST` extension block at its natural position in the stream.
    /// [`Populator`](crate::Populator)'s own documentation has the
    /// measurement: an extension block is fetched *mid-stream* by a
    /// reader already reading the file, not once at open the way a
    /// header is, so it belongs with the data run it splices into, not
    /// with the header.
    ///
    /// Hints at `header_lba`: scanning forward from a file's own header
    /// finds the block immediately after it when nothing else has claimed
    /// that space yet, which is what keeps a freshly written file's data
    /// in one run.
    DataFor {
        /// The file's header block.
        header_lba: u64,
    },
    /// A file or directory header, placed near the directory it will be
    /// filed in.
    ///
    /// Hints at `dir_lba`: an `Examine` or a path lookup touches every
    /// header in a directory in one pass, so keeping them close together
    /// raises the hit rate of a bounded hold-cache (survey §2) — this is
    /// the primitive [`Populator`](crate::Populator)'s per-directory
    /// header placement is built from.
    HeaderIn {
        /// The parent directory's header block.
        dir_lba: u64,
    },
    /// Anything a directory walk touches that is not itself a header:
    /// dircache blocks, comment-overflow blocks. Clustered toward the
    /// root because that is the one anchor every walk starts from.
    ///
    /// Hints at `root_lba`.
    MetadataNearRoot {
        /// The volume's root block.
        root_lba: u64,
    },
    /// No opinion: the rotating cursor, [`Allocator::allocate`]'s own
    /// behaviour. For a caller with nothing to anchor to.
    Anywhere,
}

impl Intent {
    /// The LBA this intent hints at, or `None` for [`Intent::Anywhere`],
    /// which has no anchor and uses the rotating cursor instead.
    fn hint(self) -> Option<u64> {
        match self {
            Intent::DataFor { header_lba } => Some(header_lba),
            Intent::HeaderIn { dir_lba } => Some(dir_lba),
            Intent::MetadataNearRoot { root_lba } => Some(root_lba),
            Intent::Anywhere => None,
        }
    }
}

// ---------------------------------------------------------------------------
// The allocator
// ---------------------------------------------------------------------------

/// A volume's bitmap, held in memory, edited, and flushed a page at a
/// time.
///
/// Deliberately does **not** own the medium. A mutation is a read of some
/// metadata, a decision, an allocation and a write, and threading the
/// medium through the allocator would put every one of those behind it;
/// instead the caller keeps its [`Volume`] and hands the allocator a sink
/// when there is something to flush. The type parameter is the medium's
/// error, so the allocator is nonetheless pinned to one transport and
/// cannot be flushed to a device it was not loaded from by accident.
pub struct Allocator<E> {
    reserved: u64,
    block_count: u64,
    block_size: usize,
    /// Bits for blocks `reserved..`, **1 = free**, in the disk's LSB-first
    /// order — the same representation [`crate::bitmap`] reads and writes,
    /// because a second representation is a second chance to invert it.
    words: Vec<u32>,
    /// Page *i* holds words `i * words_per_page ..`. A zero entry is a
    /// page a rebuilt bitmap has not been given a block for yet.
    pages: Vec<u64>,
    dirty: Vec<bool>,
    /// Where the next hintless scan starts, as a bit index.
    cursor: u64,
    free: u64,
    _marker: PhantomData<fn() -> E>,
}

impl<E> Allocator<E> {
    /// Read a volume's bitmap and prepare to allocate from it.
    ///
    /// Refuses [`AllocError::BitmapInvalid`] when the root's flag is 0 —
    /// see this module's documentation for why that is a refusal and not
    /// a warning.
    pub fn load<S: BlockSource<Error = E>>(vol: &mut Volume<S>) -> Result<Self, AllocError<E>> {
        let bs = vol.block_size();
        if !block_size_ok(bs) {
            return Err(AllocError::BadBlockSize(bs));
        }
        let bitmap = vol.read_bitmap().map_err(AllocError::Read)?;
        if !bitmap.valid() {
            return Err(AllocError::BitmapInvalid);
        }
        let reserved = bitmap.first_block();
        let pages = bitmap.pages().to_vec();
        let words = bitmap.words().to_vec();
        Ok(Self::from_parts(
            reserved,
            bitmap.end_block(),
            bs,
            pages,
            words,
            false,
        ))
    }

    /// An allocator over a set somebody else computed: every bit free,
    /// every page dirty, waiting for [`Allocator::mark_in_use`].
    ///
    /// This is [`repair`](crate::repair)'s constructor, and the only way
    /// to get an allocator over a volume whose bitmap flag is 0. The words
    /// cover the **whole volume** whatever `pages` holds, because a repair
    /// has to be able to allocate a replacement bitmap page for a region
    /// whose page is exactly what went missing; [`Allocator::set_page`]
    /// fills the gaps in before the flush that would otherwise refuse with
    /// [`AllocError::PageMissing`].
    pub fn rebuilt(reserved: u64, block_count: u64, block_size: usize, pages: Vec<u64>) -> Self {
        let words_per_page = (block_size - OFF_BITMAP_BITS) / 4;
        let per_page = bitmap_bits_per_block(block_size);
        let need = block_count.saturating_sub(reserved);
        let page_count = div_ceil(need, per_page) as usize;
        let words = vec![u32::MAX; page_count * words_per_page];
        Self::from_parts(reserved, block_count, block_size, pages, words, true)
    }

    fn from_parts(
        reserved: u64,
        block_count: u64,
        block_size: usize,
        pages: Vec<u64>,
        words: Vec<u32>,
        dirty: bool,
    ) -> Self {
        let words_per_page = (block_size - OFF_BITMAP_BITS) / 4;
        let page_count = div_ceil(words.len() as u64, words_per_page as u64) as usize;
        let mut me = Self {
            reserved,
            block_count,
            block_size,
            words,
            pages,
            dirty: vec![dirty; page_count],
            cursor: 0,
            free: 0,
            _marker: PhantomData,
        };
        me.free = me.count_free();
        me
    }

    fn count_free(&self) -> u64 {
        let covered = self.covered();
        let mut n = 0u64;
        for (i, w) in self.words.iter().enumerate() {
            let base = i as u64 * 32;
            // The last word of the last page can run past the end of the
            // volume; those bits are padding and are never handed out, so
            // they must not be counted free either.
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

    // -- accounting --------------------------------------------------------

    /// How many blocks the bitmap has bits for: `block_count - reserved`,
    /// or fewer on a volume whose bitmap is too short to cover it (which
    /// [`Finding::BitmapIncomplete`](crate::Finding::BitmapIncomplete)
    /// reports and this allocator honours by never handing out a block it
    /// has no bit for).
    pub fn covered(&self) -> u64 {
        self.block_count
            .saturating_sub(self.reserved)
            .min(self.words.len() as u64 * 32)
    }

    /// Blocks marked allocated. Counted over the covered range only, so
    /// the two boot blocks — used, but with no bit — are not in it, which
    /// is the same convention [`Bitmap::allocated_count`](crate::Bitmap)
    /// uses and the number the LNFS root's `NumBlocksUsed` field holds.
    pub fn blocks_used(&self) -> u64 {
        self.covered() - self.free
    }

    /// Blocks marked free.
    pub fn blocks_free(&self) -> u64 {
        self.free
    }

    /// The first block the bitmap covers.
    pub fn first_block(&self) -> u64 {
        self.reserved
    }

    /// One past the last block of the volume.
    pub fn end_block(&self) -> u64 {
        self.block_count
    }

    /// The bitmap pages, in coverage order. A zero entry is a page a
    /// rebuild has not chosen a block for yet.
    pub fn pages(&self) -> &[u64] {
        &self.pages
    }

    /// How many pages the bits held here need — which can exceed
    /// [`Allocator::pages`] during a rebuild, and never does otherwise.
    pub fn page_count(&self) -> usize {
        self.dirty.len()
    }

    /// Pages edited since the last successful flush.
    pub fn dirty_pages(&self) -> usize {
        self.dirty.iter().filter(|d| **d).count()
    }

    /// Is `lba` marked allocated? `None` for a block the bitmap has no
    /// bit for — deliberately not `Some(true)`, on the same reasoning as
    /// [`Bitmap::is_allocated`](crate::Bitmap::is_allocated): the boot
    /// blocks are in use and the bitmap does not say so.
    pub fn is_allocated(&self, lba: u64) -> Option<bool> {
        let bit = self.bit_of(lba)?;
        Some(self.words[(bit / 32) as usize] >> (bit % 32) & 1 == 0)
    }

    /// The bits as they will be written: 1 = free, LSB-first per longword.
    pub fn words(&self) -> &[u32] {
        &self.words
    }

    fn bit_of(&self, lba: u64) -> Option<u64> {
        if lba < self.reserved || lba >= self.block_count {
            return None;
        }
        let bit = lba - self.reserved;
        (bit < self.covered()).then_some(bit)
    }

    // -- allocating and freeing --------------------------------------------

    /// Allocate the next free block from the rotating cursor.
    ///
    /// The cursor is one past the last block handed out, so consecutive
    /// calls produce an ascending run wherever the volume has one — the
    /// locality a file's data blocks want without the caller having to
    /// say so.
    pub fn allocate(&mut self) -> Result<Allocation, AllocError<E>> {
        let from = self.cursor;
        self.allocate_from_bit(from)
    }

    /// Allocate the free block nearest above `hint`, wrapping.
    ///
    /// `hint` is where the caller would *like* the block: the file's
    /// previous data block, or its header, which is the goal Linux's
    /// `affs` passes and the reason a file's blocks end up near each
    /// other. A hint outside the volume is not an error — it is a
    /// preference, and an unsatisfiable preference simply starts the scan
    /// at the first allocatable block.
    pub fn allocate_near(&mut self, hint: u64) -> Result<Allocation, AllocError<E>> {
        let from = hint.saturating_sub(self.reserved).min(self.covered());
        self.allocate_from_bit(from)
    }

    /// Allocate one block per [`Intent`]: [`Allocator::allocate_near`] the
    /// intent's hint, or [`Allocator::allocate`] for [`Intent::Anywhere`].
    ///
    /// The policy vocabulary's single entry point — everything in this
    /// module's "Block layout policy" section reduces to this one call.
    pub fn allocate_for(&mut self, intent: Intent) -> Result<Allocation, AllocError<E>> {
        match intent.hint() {
            Some(hint) => self.allocate_near(hint),
            None => self.allocate(),
        }
    }

    /// [`Allocator::allocate_for`], but scanning from `hint` instead of
    /// `intent`'s own hint.
    ///
    /// For a caller that already knows the *kind* of placement it wants
    /// (so the [`Intent`] tag it records or reasons about stays honest)
    /// but needs to retry at a different starting point than that intent
    /// would naturally give — [`crate::compact`]'s range-avoiding retries
    /// are the one caller today. Ordinary callers want
    /// [`Allocator::allocate_for`].
    pub fn allocate_for_hinted(
        &mut self,
        _intent: Intent,
        hint: u64,
    ) -> Result<Allocation, AllocError<E>> {
        self.allocate_near(hint)
    }

    /// [`Allocator::allocate_run`], but scanning from `hint` instead of
    /// `intent`'s own hint. See [`Allocator::allocate_for_hinted`] for why
    /// this exists as a separate entry point rather than a parameter on
    /// [`Allocator::allocate_run`] itself.
    pub fn allocate_run_hinted(
        &mut self,
        n: u64,
        _intent: Intent,
        hint: u64,
    ) -> Result<Vec<Allocation>, AllocError<E>> {
        self.allocate_run_from(n, Some(hint))
    }

    /// Reserve up to `n` *contiguous* free blocks near `intent`'s hint (or
    /// from the rotating cursor for [`Intent::Anywhere`]), so a file
    /// writer can grab its whole extent — or as much of one as the volume
    /// has room for in one place — before writing a byte.
    ///
    /// **Fallback semantics, precisely:** this scans for a run of exactly
    /// `n` free blocks the same way [`Allocator::allocate_near`] scans for
    /// one (forward from the hint, wrapping once). The hint only orders
    /// *preference*, never where a run is measured from: a free run that
    /// happens to have the hint fall in its middle is still one run, its
    /// whole length counted, not two shorter ones split at the hint. If a
    /// run that long exists, all `n` blocks are marked allocated and
    /// returned. If none does, the *longest* free run anywhere on the
    /// volume is marked and returned instead — fewer than `n` blocks, but
    /// still one contiguous
    /// run, never a scatter of leftovers from several holes. The only
    /// error is [`AllocError::VolumeFull`], and only when there is no free
    /// block at all; `n == 0` returns an empty run without touching the
    /// bitmap. A caller that gets back fewer blocks than it asked for is
    /// expected to call again for the remainder — exactly the same
    /// contract [`Allocator::allocate`] already has one block at a time,
    /// generalized to a run.
    ///
    /// The returned [`Allocation`]s are in ascending LBA order and are
    /// otherwise ordinary: each one still needs
    /// [`Allocator::flush`]-then-[`Allocator::reference`] before anything
    /// may point at it, the same as a block from [`Allocator::allocate`].
    pub fn allocate_run(
        &mut self,
        n: u64,
        intent: Intent,
    ) -> Result<Vec<Allocation>, AllocError<E>> {
        self.allocate_run_from(n, intent.hint())
    }

    /// [`Allocator::allocate_run`]'s body, taking a raw hint (already an
    /// LBA, `None` for the rotating cursor) instead of an [`Intent`] —
    /// shared by [`Allocator::allocate_run`] itself and
    /// [`Allocator::allocate_run_hinted`], which needs the scan to start
    /// somewhere other than the [`Intent`] tag it is still carrying for
    /// bookkeeping would imply.
    fn allocate_run_from(
        &mut self,
        n: u64,
        hint: Option<u64>,
    ) -> Result<Vec<Allocation>, AllocError<E>> {
        if n == 0 {
            return Ok(Vec::new());
        }
        let covered = self.covered();
        let from = match hint {
            Some(hint) => hint.saturating_sub(self.reserved).min(covered),
            None => self.cursor.min(covered),
        };

        // Every maximal free run in the whole covered range, found in
        // one unbroken pass. Scanning `from..covered` and `0..from` as
        // two separate calls (this used to) draws a boundary at `from`
        // that the bitmap itself has no reason to respect: a genuinely
        // contiguous free run that happens to have the hint land in its
        // *middle* is cut into two shorter pieces by the two scans, and
        // an exact-length match that plainly exists in the unbroken run
        // can be missed because neither piece alone is long enough —
        // and the "longest run" fallback under-reports for the same
        // reason.
        let runs: Vec<(u64, u64)> = self.runs(0, covered).collect();

        // Distance forward from `from`, wrapping once at `covered` —
        // the same preference order the two-piece scan used to give
        // for free, restated so it survives not splitting the scan.
        let rank = |start: u64| {
            if start >= from {
                start - from
            } else {
                covered - from + start
            }
        };

        let exact = runs
            .iter()
            .copied()
            .filter(|&(_, len)| len >= n)
            .min_by_key(|&(start, _)| rank(start));

        let (start, len) = match exact {
            // A run of at least `n` exists: take exactly `n` blocks of
            // it, not the whole (possibly longer) run it was found in.
            Some((start, _)) => (start, n),
            // None does: the single longest run anywhere in the volume.
            None => match runs.into_iter().max_by_key(|&(_, len)| len) {
                Some(r) => r,
                None => {
                    return Err(AllocError::VolumeFull {
                        block_count: self.block_count,
                    })
                }
            },
        };

        let mut out = Vec::with_capacity(len as usize);
        for bit in start..start + len {
            self.words[(bit / 32) as usize] &= !(1u32 << (bit % 32));
            self.free -= 1;
            let page = self.page_of(bit);
            self.dirty[page] = true;
            out.push(Allocation {
                lba: self.reserved + bit,
                page,
            });
        }
        self.cursor = start + len;
        Ok(out)
    }

    /// Every maximal run of free bits in `from..to`, as `(start, len)`,
    /// left to right. Bit-by-bit: run-finding is not [`Allocator::scan`]'s
    /// hot path (a single free bit, skipping whole allocated longwords at
    /// a time) — it has to look *past* the first free bit to measure how
    /// far the run goes, so there is nothing for the longword shortcut to
    /// buy here.
    fn runs(&self, from: u64, to: u64) -> impl Iterator<Item = (u64, u64)> + '_ {
        let mut bit = from;
        core::iter::from_fn(move || {
            while bit < to && !self.free_bit(bit) {
                bit += 1;
            }
            if bit >= to {
                return None;
            }
            let start = bit;
            while bit < to && self.free_bit(bit) {
                bit += 1;
            }
            Some((start, bit - start))
        })
    }

    /// Is bit `bit` free? Panics on a bit outside `words`' coverage —
    /// every caller here already bounds `bit < covered()`.
    fn free_bit(&self, bit: u64) -> bool {
        self.words[(bit / 32) as usize] >> (bit % 32) & 1 != 0
    }

    fn allocate_from_bit(&mut self, from: u64) -> Result<Allocation, AllocError<E>> {
        let covered = self.covered();
        let from = from.min(covered);
        let bit = match self.scan(from, covered).or_else(|| self.scan(0, from)) {
            Some(b) => b,
            None => {
                return Err(AllocError::VolumeFull {
                    block_count: self.block_count,
                })
            }
        };
        // Clear, because 1 is free. The single most invertible line in
        // the crate, and the reason `pack_bits` and this share a module's
        // worth of documented conventions.
        self.words[(bit / 32) as usize] &= !(1u32 << (bit % 32));
        self.free -= 1;
        self.cursor = bit + 1;
        let page = self.page_of(bit);
        self.dirty[page] = true;
        Ok(Allocation {
            lba: self.reserved + bit,
            page,
        })
    }

    /// The first free bit in `from..to`, a longword at a time.
    ///
    /// Word-wise rather than bit-wise because the common case on a
    /// half-full 2 GB volume is skipping a few hundred thousand allocated
    /// bits, and `trailing_zeros` does thirty-two of them at once.
    fn scan(&self, from: u64, to: u64) -> Option<u64> {
        if from >= to {
            return None;
        }
        let first_word = (from / 32) as usize;
        let last_word = ((to - 1) / 32) as usize;
        for w in first_word..=last_word {
            let mut word = self.words[w];
            let base = w as u64 * 32;
            if base < from {
                // Bits below the start of the range are not candidates.
                word &= u32::MAX << (from - base);
            }
            let end = base + 32;
            if end > to {
                let keep = to - base;
                word &= if keep == 32 {
                    u32::MAX
                } else {
                    (1u32 << keep) - 1
                };
            }
            if word != 0 {
                return Some(base + word.trailing_zeros() as u64);
            }
        }
        None
    }

    /// Allocate one specific, already-known-free block.
    ///
    /// The exact-destination counterpart of [`Allocator::allocate_near`]:
    /// a caller that has already chosen `lba` (a compactor evacuating a
    /// range, a caller retrying a relocation at the same target) needs an
    /// [`Allocation`] naming *that* block, not the nearest free one to a
    /// hint. Refuses [`AllocError::NotCovered`] for a block the volume has
    /// no bit for at all, and, distinctly, [`AllocError::AlreadyAllocated`]
    /// for a block the bitmap covers but already marks in use — the two
    /// are different facts (geometry versus occupancy) and a caller that
    /// wants to distinguish "evacuate this block first" from "this LBA
    /// isn't even in the volume" needs them as different variants, not one
    /// reused for both.
    pub fn allocate_exact(&mut self, lba: u64) -> Result<Allocation, AllocError<E>> {
        let bit = self.bit_of(lba).ok_or(AllocError::NotCovered { lba })?;
        let w = (bit / 32) as usize;
        let mask = 1u32 << (bit % 32);
        if self.words[w] & mask == 0 {
            return Err(AllocError::AlreadyAllocated { lba });
        }
        self.words[w] &= !mask;
        self.free -= 1;
        let page = self.page_of(bit);
        self.dirty[page] = true;
        Ok(Allocation { lba, page })
    }

    /// Mark a block allocated without handing it out.
    ///
    /// For a caller that already knows the block is in use — a repair
    /// rebuilding from a reachability walk, or an adopter of a layout
    /// somebody else laid down. Returns whether the bit changed, so a
    /// repair can count what it actually corrected; marking an
    /// already-allocated block again is deliberately *not* an error,
    /// because the sets a rebuild unions overlap by construction.
    pub fn mark_in_use(&mut self, lba: u64) -> Result<bool, AllocError<E>> {
        let bit = self.bit_of(lba).ok_or(AllocError::NotCovered { lba })?;
        let w = (bit / 32) as usize;
        let mask = 1u32 << (bit % 32);
        if self.words[w] & mask == 0 {
            return Ok(false);
        }
        self.words[w] &= !mask;
        self.free -= 1;
        let page = self.page_of(bit);
        self.dirty[page] = true;
        Ok(true)
    }

    /// Give a block back.
    ///
    /// **Call this after the metadata that referenced the block is off
    /// the disk, never before.** A crash between the unlink and this call
    /// leaks the block, which a validator fixes; a crash the other way
    /// round leaves the block free and still referenced, which the next
    /// allocation turns into two owners.
    ///
    /// Refuses [`AllocError::DoubleFree`] for a block already marked
    /// free. That is the one ordering error visible from inside this
    /// module, and it is worth refusing rather than absorbing: a second
    /// free means either a retried delete or two structures that both
    /// think they own the block, and neither should be silent.
    pub fn free(&mut self, lba: u64) -> Result<(), AllocError<E>> {
        let bit = self.bit_of(lba).ok_or(AllocError::NotCovered { lba })?;
        let w = (bit / 32) as usize;
        let mask = 1u32 << (bit % 32);
        if self.words[w] & mask != 0 {
            return Err(AllocError::DoubleFree { lba });
        }
        self.words[w] |= mask;
        self.free += 1;
        let page = self.page_of(bit);
        self.dirty[page] = true;
        Ok(())
    }

    /// The LBA of an allocation, for storing in somebody else's pointer.
    ///
    /// Refuses [`AllocError::NotDurable`] while the bitmap page holding
    /// the bit has not been flushed. This is the mark-then-use rule as a
    /// return type: a caller that writes `reference(&a)?` into a hash
    /// slot, a data-pointer table or a chain longword cannot have done it
    /// before the bitmap said the block was taken.
    pub fn reference(&self, a: &Allocation) -> Result<u64, AllocError<E>> {
        match self.dirty.get(a.page) {
            Some(true) => Err(AllocError::NotDurable { lba: a.lba }),
            _ => Ok(a.lba),
        }
    }

    fn page_of(&self, bit: u64) -> usize {
        (bit / bitmap_bits_per_block(self.block_size)) as usize
    }

    /// Record which block a bitmap page lives in.
    ///
    /// A rebuild's tool: [`Allocator::rebuilt`] starts with the pages the
    /// damaged volume could still be believed about, and this fills in the
    /// replacements as they are allocated. The page is marked dirty, since
    /// a page that has just moved has certainly not been written where it
    /// now lives.
    pub fn set_page(&mut self, index: usize, lba: u64) {
        if self.pages.len() <= index {
            self.pages.resize(index + 1, 0);
        }
        self.pages[index] = lba;
        if let Some(d) = self.dirty.get_mut(index) {
            *d = true;
        }
    }

    // -- flushing ----------------------------------------------------------

    /// Write every dirty bitmap page, and nothing else.
    ///
    /// Returns how many pages were written. Each page's dirty flag is
    /// cleared only once its write has returned, so an interrupted flush
    /// leaves the remainder queued and a retry finishes the job rather
    /// than starting a new one.
    ///
    /// The checksum goes in longword **0** ([`BITMAP_CHECKSUM_INDEX`]),
    /// which is the format's one exception and the one a writer gets
    /// wrong in the direction that still verifies against itself.
    pub fn flush<K: BlockSink<Error = E>>(&mut self, sink: &mut K) -> Result<usize, AllocError<E>> {
        let bs = self.block_size;
        if sink.block_size() != bs {
            return Err(AllocError::BadBlockSize(sink.block_size()));
        }
        let words_per_page = (bs - OFF_BITMAP_BITS) / 4;
        let mut buf = vec![0u8; bs];
        let mut written = 0;
        for i in 0..self.dirty.len() {
            if !self.dirty[i] {
                continue;
            }
            let page = self.pages.get(i).copied().unwrap_or(0);
            if page == 0 || page >= self.block_count {
                return Err(AllocError::PageMissing { index: i });
            }
            buf.iter_mut().for_each(|b| *b = 0);
            for w in 0..words_per_page {
                let v = self
                    .words
                    .get(i * words_per_page + w)
                    .copied()
                    .unwrap_or(u32::MAX);
                wr32(&mut buf, OFF_BITMAP_BITS + w * 4, v);
            }
            let ck = checksum_compute(&buf, BITMAP_CHECKSUM_INDEX);
            wr32(&mut buf, BITMAP_CHECKSUM_INDEX * 4, ck);
            sink.write_block(page, &buf).map_err(AllocError::Io)?;
            self.dirty[i] = false;
            written += 1;
        }
        Ok(written)
    }

    // -- the root's two bitmap fields --------------------------------------

    /// Clear the root's `bitmap_flag`: "my bitmap is mid-update".
    ///
    /// The first write of a session that is about to make the bitmap
    /// temporarily disagree with the volume, and the state an interrupted
    /// one is then left in — a volume that refuses to be allocated from
    /// rather than one that quietly hands out blocks a file is using.
    /// [`Populator`](crate::Populator) does the same thing for the same
    /// reason.
    pub fn mark_bitmap_invalid<M>(&self, medium: &mut M, root_lba: u64) -> Result<(), AllocError<E>>
    where
        M: BlockSource<Error = E> + BlockSink<Error = E>,
    {
        self.stamp_root(medium, root_lba, None, 0)
    }

    /// Stamp the root valid again: `NumBlocksUsed` on `DOS\6`/`DOS\7`,
    /// then `bitmap_flag = -1`.
    ///
    /// **Call it last**, after [`Allocator::flush`] has put the pages it
    /// describes on the disk. Until the flag is −1 nothing trusts those
    /// pages, so a failure before this point leaves the honest
    /// "do not allocate from me" state rather than a lie.
    pub fn mark_bitmap_valid<M>(
        &self,
        medium: &mut M,
        root_lba: u64,
        variant: Variant,
    ) -> Result<(), AllocError<E>>
    where
        M: BlockSource<Error = E> + BlockSink<Error = E>,
    {
        let used = variant.has_long_names().then(|| self.blocks_used() as u32);
        self.stamp_root(medium, root_lba, used, -1)
    }

    fn stamp_root<M>(
        &self,
        medium: &mut M,
        root_lba: u64,
        blocks_used: Option<u32>,
        flag: i32,
    ) -> Result<(), AllocError<E>>
    where
        M: BlockSource<Error = E> + BlockSink<Error = E>,
    {
        let bs = self.block_size;
        if BlockSource::block_size(medium) != bs {
            return Err(AllocError::BadBlockSize(BlockSource::block_size(medium)));
        }
        if root_lba >= self.block_count {
            return Err(AllocError::Read(crate::read::Error::LbaOutOfRange {
                lba: root_lba,
                block_count: self.block_count,
            }));
        }
        let mut buf = vec![0u8; bs];
        medium
            .read_block(root_lba, &mut buf)
            .map_err(|e| AllocError::Read(crate::read::Error::Io(e)))?;
        if !checksum_ok(&buf) {
            return Err(AllocError::Read(crate::read::Error::Checksum {
                lba: root_lba,
            }));
        }
        if let Some(used) = blocks_used {
            wr32(&mut buf, tail(bs, TL_ROOT_NUM_BLOCKS_USED), used);
        }
        wr32(&mut buf, tail(bs, TL_BITMAP_FLAG), flag as u32);
        let ck = checksum_compute(&buf, CHECKSUM_INDEX);
        wr32(&mut buf, OFF_CHECKSUM, ck);
        medium.write_block(root_lba, &buf).map_err(AllocError::Io)?;
        Ok(())
    }
}
