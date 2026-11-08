//! Amiga Fast File System (and its Old File System ancestor): pure
//! format logic for every `DOS\0`–`DOS\7` variant, including the
//! `DOS\6`/`DOS\7` long-filename layouts.
//!
//! This crate is the filesystem half of a deliberate split: partition
//! tables live in [`amiga-rdb`](https://github.com/sidick/amiga-rdb-rs),
//! one filesystem family per crate, composed by the consumer through a
//! block-device seam. Nothing here knows what an RDB is; a volume is
//! just a run of blocks.
//!
//! # The seam
//!
//! [`BlockSource`] carries four decisions, each made deliberately and
//! each confirmed by watching another crate choose differently and pay
//! for it:
//!
//! - `&mut self` — a device read is a mutation (seeks, caches, wear);
//!   `&self` just forces interior mutability onto every real backend.
//! - A typed error — "why did the read fail" is the first question a
//!   storage stack asks; `Result<(), ()>` cannot answer it.
//! - `u64` LBAs — 32-bit block arithmetic is how the platform's formats
//!   hit their 2 TB walls.
//! - **Runtime block size** — filesystem blocks are 512 bytes to 32 KB,
//!   *per volume*, on the same disk. A `[u8; 512]` in the trait
//!   signature is a wall built exactly where the format is flexible.
//!
//! # The shape of the API
//!
//! Read once; it explains several things that look like duplication and
//! are not.
//!
//! **Two writers, same verbs.** [`Populator`] and [`Mutator`] both have
//! `create_dir`/`create_file`, deliberately under the same names: the
//! *operation* is the same, the *session contract* differs. A
//! `Populator` fills a volume this crate just formatted — the free
//! space is known, not discovered, the bitmap is written once at
//! `finish()`, and the volume is unmountable mid-session by design. A
//! `Mutator` changes a volume anybody wrote — every block comes from
//! the volume's own bitmap, every operation leaves the volume
//! consistent, and the crash-shape rules hold at each step. Building a
//! fresh image: `Populator`. Everything else: `Mutator`.
//!
//! **[`Metadata`] describes; [`MetaUpdate`] amends.** Creation takes a
//! full description (absent means the default); `set_metadata` takes
//! `Option` per field (absent means untouched) — because "unmentioned"
//! must not mean "cleared" when there is existing state to clear.
//!
//! **One error type per operation family**, not one for the crate:
//! [`Error`] (reading), [`FormatError`], [`PopulateError`],
//! [`AllocError`], [`MutateError`], [`ResizeError`],
//! [`CompactError`]. Each is generic over the
//! block device's own error, which survives unflattened and is
//! reachable through `source()` under `std`; each lists exactly what
//! its operations can refuse, so a caller matches what can actually
//! happen rather than a crate-wide catch-all. `validate()` is the
//! deliberate exception: damage is not an error, so it returns typed
//! [`Finding`]s in a [`Report`] and keeps walking.
//!
//! **[`Volume`] accretes capabilities by module** — reading and
//! traversal from [`read`], file data from [`mod@file`], the bitmap
//! from [`bitmap`], `validate()`/`repair()`/`resize()` from their
//! modules — so its docs page is long; the module docs are where each
//! capability's reasoning lives.
//!
//! **Constructor verbs say what happens**: `Volume::open` reads and
//! verifies structures off a medium; `Populator::new` formats first
//! (`adopt` wraps a volume already formatted this session);
//! `Mutator::open` checks the volume is safe to mutate;
//! `Allocator::load` reads the whole bitmap. None of them is a plain
//! `new` because none of them is free.
//!
//! # Status
//!
//! Primitives — DOS-type/variant model, block checksums (standard and
//! boot-block), BCPL strings, and the two name-hash functions with their
//! case-folding rules — plus the structural layers on top: [`layout`]
//! (every on-disk offset, for both the classic and the long-name name
//! layouts), [`read`] (boot block, root block, directory traversal),
//! [`mod@file`] (file data through FFS chains and OFS data blocks, hard-link
//! resolution, soft-link paths, overflow comment blocks), [`meta`]
//! (the protection longword's inverted-sense bits, and the `DateStamp`'s
//! calendar conversion), [`dircache`] (`DOS\4`/`DOS\5` cache blocks, read
//! and marked advisory), [`bitmap`] (the allocation bitmap, with its four
//! easily-inverted conventions stated), [`validate`] (the whole-volume
//! walk that reports rather than refuses), [`mod@format`] (creating a
//! volume: boot block, root and bitmap, every variant, every block size)
//! [`populate`] (filling one: directories, files, metadata, and a
//! host directory tree under `std`), [`allocator`] (block allocation
//! against a volume that already has something in it, with the
//! mark-then-use ordering in the types), [`repair`] (rebuilding a
//! bitmap from the same walk [`validate`] performs) and [`mutate`]
//! (creating, deleting, renaming and re-describing entries in a volume
//! that already exists), [`resize`] (growing or shrinking a volume in
//! place — the root must move, because FFS recomputes its position from
//! geometry on every mount) and [`compact`] (relocating file data and
//! header blocks toward the layout the crate's own policy states, with
//! the safety ReOrg's manual admits it lacks).
//!
//! The primitives come first because they are where a subtle mistake —
//! the wrong `toupper` table, a hash off by the length byte, a name read
//! at the classic offset on a `DOS\7` volume — produces a filesystem that
//! *mostly* works. Everything above them is pointer-following, and every
//! pointer is range-checked and every chain refuses to revisit a block.
//!
//! Milestone 1 (read) is complete, and milestone 2's creation side with
//! it: [`BlockSink`] is the write seam (a second trait, not a bound on
//! [`BlockSource`] — read-only sources are the common case),
//! [`format`](format()) creates a fresh, empty, valid volume of any
//! variant at any block size, and [`Populator`] fills one — directories,
//! files, comments, dates, protection, every name layout, OFS and FFS
//! data blocks, extension blocks and `DOS\4`/`DOS\5` dircaches — with
//! `populate::populate_from_tree` turning a host directory into an image
//! in one call under the `std` feature.
//!
//! Milestone 3 (mutate) is complete, its differential suite closed in
//! both directions — the ROM's own FFS and this crate taking turns
//! mutating the same images. [`Allocator`] hands out blocks
//! from an existing volume's own bitmap — refusing one that is
//! mid-update, tracking dirty pages, and making the crash ordering
//! visible in the types ([`Allocation::block`] to write a block,
//! [`Allocator::reference`] to point at one, and it refuses until the
//! bitmap page is on the disk) — and [`Volume::repair`] rebuilds a damaged
//! bitmap from the reachability walk, adding allocation and never removing
//! it. On top of those, [`Mutator`] creates, deletes and renames entries
//! in a volume this crate did not write, and changes their metadata:
//! hash-chain splicing under both fold tables, `T_COMMENT` blocks moving
//! in and out as an LNFS name grows past what the merged field holds,
//! `DOS\4`/`DOS\5` dircaches regenerated from the chains they cache, and
//! a write order at every step whose worst crash outcome is a leak. It
//! also writes, appends to and truncates a file's *contents*
//! ([`Mutator::write_file`], [`Mutator::append`], [`Mutator::truncate`]),
//! with the file header block as the single commit — before that one
//! write the file is entirely the old one, after it entirely the new —
//! and [`Volume::read_range`] reads a slice of a file back, walking only
//! the blocks the range covers. Deletion is two halves —
//! [`Mutator::unlink`] removes the entry, [`Mutator::release`] frees the
//! chain — so a consumer with open-handle semantics (a FUSE adapter's
//! create-then-unlink) can hold the gap open; `delete` is the composed
//! pair.
//!
//! Beyond the milestones: [`Volume::resize`] grows and shrinks a volume
//! in place (idempotent on retry, with `resize_evacuating` moving what
//! blocks the way); [`Volume::resize_to`] does the same for a medium that
//! implements [`ResizableMedium`] — one that owns its own storage rather
//! than being sized by an outside authority — by growing or truncating
//! the medium itself as part of the one call, instead of a caller
//! extending raw bytes and reopening at the old size by hand;
//! [`Mutator::compact`], `defragment_file`,
//! `relocate_header` and `make_room` reorganise existing volumes; and
//! the allocator's [`Intent`] vocabulary carries a measured layout
//! policy — metadata clustered at the root where directory walks pay,
//! file data as single ascending runs where streaming pays, extension
//! blocks in the stream because a real Kickstart read both placements
//! and the interleaved one won (`docs/layout-survey.md` has the
//! numbers: a fragmented file cost 47% through a real ROM, and
//! defragmenting recovered all of it). Ordinary [`Mutator`] writes
//! place blocks by the same policy, so a full-content rewrite lands as
//! one run — the copy-out-and-back defragmentation gesture that never
//! worked on stock FFS works through this crate.

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(missing_docs)]

extern crate alloc;

pub mod allocator;
pub mod bitmap;
/// Pure block assembly, shared by the two writers.
///
/// Deliberately crate-private: it is the *shape* of a header block, a
/// comment block, a data block and a dircache record, factored out of
/// [`populate`] when [`mutate`] turned out to need exactly the same bytes.
/// Two copies of that would be two chances to put an LNFS comment at the
/// classic offset.
mod build;
pub mod compact;
pub mod dircache;
pub mod file;
pub mod format;
pub mod layout;
pub mod meta;
pub mod mutate;
pub mod populate;
pub mod read;
pub mod repair;
pub mod resize;
pub mod validate;

pub use layout::{
    ST_FILE, ST_LINKDIR, ST_LINKFILE, ST_ROOT, ST_SOFTLINK, ST_USERDIR, T_COMMENT, T_DATA,
    T_DIRCACHE, T_HEADER, T_LIST,
};
pub use meta::{CalendarDate, Protection};
pub use read::{
    canonical_root_lba, names_equal, read_boot_dostype, resolve_variant, verify_root_block,
    DateStamp, DostypeSource, Entry, EntryKind, Error, RootBlock, Volume, DEFAULT_RESERVED,
};

pub use allocator::{AllocError, Allocation, Allocator, Intent};
pub use bitmap::Bitmap;
pub use compact::{
    CompactError, CompactEvent, CompactOptions, CompactReport, FileDefragReport,
    HeaderRelocateReport, MakeRoomReport,
};
pub use dircache::{Dircache, DircacheRecord};
pub use file::FileChain;
pub use format::{format, FormatError, FormatLayout, FormatOptions, BOOT_AREA_LEN};
pub use mutate::{MetaUpdate, MutateError, Mutator, MutatorVolume};
pub use populate::{Metadata, PopulateError, Populator};
pub use repair::{Action, RepairOptions, RepairReport};
pub use resize::{ResizableMedium, ResizeError, ResizeReport};
pub use validate::{DircacheDiscrepancy, Finding, Report, Summary};

/// Anything that can produce fixed-size blocks by LBA.
///
/// `block_size()` is a property of the *source*, reported at runtime —
/// for a partition source this is the filesystem's block size, whatever
/// the volume was formatted with. `read_block` fills `buf`, whose length
/// the caller sizes to `block_size()`; implementations must reject a
/// mismatched buffer via their error type rather than truncate.
pub trait BlockSource {
    /// How this source reports a failed read. No bound is imposed:
    /// `no_std` backends bring whatever error type they have, and the
    /// crate's own error types carry it through unflattened.
    type Error;

    /// This source's block size in bytes. Stable for the source's
    /// lifetime; power of two, 512..=32768 for every real volume.
    fn block_size(&self) -> usize;

    /// Read block `lba` into `buf` (`buf.len() == self.block_size()`).
    fn read_block(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), Self::Error>;

    /// Total number of blocks, if known.
    fn block_count(&self) -> Option<u64> {
        None
    }
}

/// Anything that can accept fixed-size blocks by LBA — the write seam.
///
/// Deliberately a *second* trait rather than `write_block` bolted onto
/// [`BlockSource`], and the same decision (and the same wording) as
/// `amiga-rdb`'s: read-only sources are the common case and most of this
/// crate's work — a `File` opened for reading, a memory-mapped image, a
/// `&[u8]`, an emulator's read-only medium. One trait would force every
/// one of them to supply a `write_block` that can only fail at runtime,
/// which is a compile-time truth thrown away. Split, "this code writes"
/// is visible in the bound: anything that reads *and* writes says
/// `S: BlockSource + BlockSink`, and anything that only reads cannot be
/// handed a sink by accident. The cost is `block_size` and `block_count`
/// appearing on both traits — a deliberate duplication, since a type
/// implementing both will have them agree trivially, and making
/// `BlockSink: BlockSource` instead would rule out a write-only target
/// (a fresh image being streamed out) for no gain. [`format`](format())
/// is exactly such a caller: it writes a volume and never reads one
/// back, so it asks for `BlockSink` alone and leaves verifying the
/// result to whoever also has a [`BlockSource`].
///
/// The four decisions behind [`BlockSource`] — `&mut self`, a typed
/// error, `u64` LBAs, a runtime block size — are the four decisions
/// here, for the same four reasons.
pub trait BlockSink {
    /// How this sink reports a failed write. No bound is imposed here,
    /// as on [`BlockSource::Error`].
    type Error;

    /// Bytes per block. Stable for the sink's lifetime, and — for a type
    /// that is also a [`BlockSource`] — equal to what that trait reports.
    fn block_size(&self) -> usize;

    /// Write `buf` to block `lba`; `buf.len() == self.block_size()`.
    ///
    /// Whether the write has reached stable storage when this returns is
    /// the implementation's business — nothing here assumes it, and a
    /// caller that needs durability flushes the underlying object itself.
    /// *Ordering* is this crate's business, though: the write paths are
    /// ordered so an interruption leaves the previous structure intact,
    /// which only holds if a sink does not reorder writes behind the
    /// caller's back.
    fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), Self::Error>;

    /// Total number of blocks, if known — `None` on the same terms as
    /// [`BlockSource::block_count`]. A sink that knows its size lets
    /// [`format`](format()) refuse a layout that runs off the end
    /// *before* writing the first block rather than halfway through.
    fn block_count(&self) -> Option<u64> {
        None
    }
}

/// The transport error of a type that both reads and writes.
///
/// [`BlockSource`] and [`BlockSink`] each name their own error type, and
/// everything that mutates a volume needs both traits — so it requires
/// them to be the *same* type. Every real backend that does both (a file,
/// an image in memory, a partition on a device) reports its failures one
/// way; two error types would put a `Read`/`Write` split through every
/// signature in the write half for a distinction no backend actually
/// makes.
pub type Transport<S> = <S as BlockSource>::Error;

/// A type that can be written as well as read: it reads and writes the
/// same medium and fails the same way doing either.
///
/// The bound every mutating entry point takes. [`Populator`] introduced
/// it, and [`Allocator`] and [`repair`] share it — which is why it lives
/// here beside the two traits it composes rather than in whichever module
/// happened to need it first.
pub trait BlockMedium: BlockSource + BlockSink<Error = <Self as BlockSource>::Error> {}

impl<T> BlockMedium for T where T: BlockSource + BlockSink<Error = <T as BlockSource>::Error> {}

/// Read a big-endian u32 at byte offset `off`.
#[inline]
pub fn be32(block: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([block[off], block[off + 1], block[off + 2], block[off + 3]])
}

/// Read a big-endian u16 at byte offset `off`.
#[inline]
pub fn be16(block: &[u8], off: usize) -> u16 {
    u16::from_be_bytes([block[off], block[off + 1]])
}

// ---------------------------------------------------------------------------
// DOS types and variants
// ---------------------------------------------------------------------------

/// The `DOS\x` filesystem variants, by the low byte of the dostype.
///
/// The three axes are genuinely independent bits (`FFS`, `INTL`,
/// `DIRCACHE`/`LONGNAME`), but the *meaningful combinations* are the
/// eight shipped values, so this is an enum rather than a bitfield:
/// a `DOS\x` byte outside 0..=7 is a variant this crate has never seen
/// and refuses, rather than a bit pattern to guess semantics for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// `DOS\0` — the original OFS.
    Ofs,
    /// `DOS\1` — FFS.
    Ffs,
    /// `DOS\2` — OFS, international case rules.
    OfsIntl,
    /// `DOS\3` — FFS, international case rules.
    FfsIntl,
    /// `DOS\4` — OFS, international, directory cache blocks.
    OfsIntlDircache,
    /// `DOS\5` — FFS, international, directory cache blocks.
    FfsIntlDircache,
    /// `DOS\6` — OFS, international, long filenames.
    OfsIntlLongname,
    /// `DOS\7` — FFS, international, long filenames.
    FfsIntlLongname,
}

/// The `DOS` magic in a dostype's top three bytes.
pub const DOSTYPE_MAGIC: u32 = 0x444F_5300;

impl Variant {
    /// From a full 32-bit dostype (e.g. `0x444F5307`). `None` for
    /// anything that is not `DOS\0`..`DOS\7` — including other real
    /// filesystems (`PFS\x`, `SFS\x`, `muFS`...), which are simply not
    /// this crate's family.
    pub fn from_dostype(dostype: u32) -> Option<Self> {
        if dostype & 0xFFFF_FF00 != DOSTYPE_MAGIC {
            return None;
        }
        Some(match dostype & 0xFF {
            0 => Self::Ofs,
            1 => Self::Ffs,
            2 => Self::OfsIntl,
            3 => Self::FfsIntl,
            4 => Self::OfsIntlDircache,
            5 => Self::FfsIntlDircache,
            6 => Self::OfsIntlLongname,
            7 => Self::FfsIntlLongname,
            _ => return None,
        })
    }

    /// The full 32-bit dostype.
    pub fn dostype(self) -> u32 {
        DOSTYPE_MAGIC | self.low_byte() as u32
    }

    fn low_byte(self) -> u8 {
        match self {
            Self::Ofs => 0,
            Self::Ffs => 1,
            Self::OfsIntl => 2,
            Self::FfsIntl => 3,
            Self::OfsIntlDircache => 4,
            Self::FfsIntlDircache => 5,
            Self::OfsIntlLongname => 6,
            Self::FfsIntlLongname => 7,
        }
    }

    /// FFS data blocks (no per-block headers) rather than OFS ones.
    pub fn is_ffs(self) -> bool {
        matches!(
            self,
            Self::Ffs | Self::FfsIntl | Self::FfsIntlDircache | Self::FfsIntlLongname
        )
    }

    /// International case-folding rules for name hashing and comparison
    /// (`DOS\2` and up — everything after the original pair).
    pub fn is_intl(self) -> bool {
        !matches!(self, Self::Ofs | Self::Ffs)
    }

    /// Directory-cache blocks are present (`DOS\4`/`DOS\5`).
    pub fn has_dircache(self) -> bool {
        matches!(self, Self::OfsIntlDircache | Self::FfsIntlDircache)
    }

    /// Long-filename directory entries (`DOS\6`/`DOS\7`): names up to
    /// [`MAX_NAME_LONG`] bytes, stored in a different block layout.
    /// The variant where a classic-offset reader silently returns empty
    /// names for every entry — the failure this crate exists to not have.
    pub fn has_long_names(self) -> bool {
        matches!(self, Self::OfsIntlLongname | Self::FfsIntlLongname)
    }

    /// Which case-folding the variant hashes and compares names with.
    pub fn fold(self) -> fn(u8) -> u8 {
        if self.is_intl() {
            intl_toupper
        } else {
            classic_toupper
        }
    }

    /// The longest name this variant can store: [`MAX_NAME_LONG`] on
    /// `DOS\6`/`DOS\7`, [`MAX_NAME_CLASSIC`] everywhere else. One answer,
    /// next to every other capability question a variant settles, rather
    /// than three copies of the same `has_long_names()` branch on
    /// [`Volume`], [`Mutator`] and [`Populator`].
    pub fn max_name_len(self) -> usize {
        if self.has_long_names() {
            MAX_NAME_LONG
        } else {
            MAX_NAME_CLASSIC
        }
    }
}

/// Maximum name length on classic variants (`DOS\0`–`DOS\5`).
pub const MAX_NAME_CLASSIC: usize = 30;

/// Maximum name length on long-filename variants (`DOS\6`/`DOS\7`).
pub const MAX_NAME_LONG: usize = 107;

// ---------------------------------------------------------------------------
// Case folding and name hashing
// ---------------------------------------------------------------------------

/// `DOS\0`/`DOS\1` case folding: ASCII `a-z` only. Latin-1 letters with
/// diacritics do *not* fold — `é` and `É` are different names on an
/// original-variant volume.
#[inline]
pub fn classic_toupper(c: u8) -> u8 {
    if c.is_ascii_lowercase() {
        c - 0x20
    } else {
        c
    }
}

/// International case folding (`DOS\2`+): ASCII `a-z` plus the Latin-1
/// letters `0xE0..=0xFE` — except `0xF7` (÷), which is not a letter.
/// (`0xFF`, ÿ, has no Latin-1 uppercase, and the Amiga tables leave it
/// alone.)
#[inline]
pub fn intl_toupper(c: u8) -> u8 {
    if c.is_ascii_lowercase() || (0xE0..=0xFE).contains(&c) && c != 0xF7 {
        c - 0x20
    } else {
        c
    }
}

/// Hash a name into a directory hash-table slot.
///
/// The AmigaDOS function, exactly: seed with the length, then for each
/// byte `hash = (hash * 13 + fold(byte)) & 0x7FF`, then reduce modulo
/// the table size. `table_size` is in slots — 72 for a 512-byte block
/// (`block_size/4 - 56`), larger for larger blocks, which is one of the
/// two places block size changes directory layout (the other being how
/// many entries a dircache block holds).
///
/// The fold function must be [`Variant::fold`]'s choice for the volume:
/// hashing with the wrong table puts the name in the wrong chain, which
/// manifests as files that exist but cannot be found — or worse, can be
/// created twice.
pub fn name_hash(name: &[u8], fold: fn(u8) -> u8, table_size: u32) -> u32 {
    let mut hash = name.len() as u32;
    for &c in name {
        hash = hash.wrapping_mul(13).wrapping_add(fold(c) as u32) & 0x7FF;
    }
    hash % table_size
}

/// The hash-table size for a given block size, in slots
/// (`block_size/4 - 56`): 72 at 512 bytes, 968 at 4 KB, 8136 at 32 KB.
#[inline]
pub fn hash_table_size(block_size: usize) -> u32 {
    (block_size / 4) as u32 - 56
}

// ---------------------------------------------------------------------------
// Checksums
// ---------------------------------------------------------------------------

/// Verify the standard FFS block checksum: all longwords of the block
/// sum to zero (wrapping). Verification needs no field index — the
/// stored checksum participates in the sum, wherever it lives; only
/// [`checksum_compute`] needs to know which longword to leave out
/// (root/directory/file-header/OFS-data/extension blocks use index 5,
/// bitmap blocks index 0).
///
/// Works on any block size — the sum runs over `block.len()/4`
/// longwords, which is exactly why this takes a slice and not an array.
pub fn checksum_ok(block: &[u8]) -> bool {
    let mut sum: u32 = 0;
    for off in (0..block.len() & !3).step_by(4) {
        sum = sum.wrapping_add(be32(block, off));
    }
    sum == 0
}

/// Compute the value to store in longword `chksum_index` so that
/// [`checksum_ok`] passes: the negated wrapping sum of every *other*
/// longword.
pub fn checksum_compute(block: &[u8], chksum_index: usize) -> u32 {
    let mut sum: u32 = 0;
    for off in (0..block.len() & !3).step_by(4) {
        if off != chksum_index * 4 {
            sum = sum.wrapping_add(be32(block, off));
        }
    }
    sum.wrapping_neg()
}

/// The boot block's checksum is a *different algorithm* from every
/// other block: add with end-around carry (each 32-bit overflow feeds
/// a +1 back in), over the whole 1024-byte boot area, with the
/// checksum longword (offset 4) taken as zero; the stored value makes
/// the total `0xFFFFFFFF`. This computes the value to store.
///
/// Kept even though this crate reads hard-disk volumes (whose boot
/// blocks are rarely executable): `create` will have to write one, and
/// the two checksum algorithms being different is exactly the sort of
/// fact that gets lost if the code doesn't state it.
pub fn bootblock_checksum(boot: &[u8]) -> u32 {
    let mut sum: u32 = 0;
    for off in (0..boot.len() & !3).step_by(4) {
        let v = if off == 4 { 0 } else { be32(boot, off) };
        let (s, carry) = sum.overflowing_add(v);
        sum = s.wrapping_add(carry as u32);
    }
    !sum
}

// ---------------------------------------------------------------------------
// BCPL strings
// ---------------------------------------------------------------------------

/// Read a BCPL string (length byte, then bytes, no terminator) at
/// `off`, capped at `max` — [`MAX_NAME_CLASSIC`] for classic name
/// fields, [`MAX_NAME_LONG`] for `DOS\6`/`DOS\7` ones. Returns the raw
/// bytes; names are Latin-1, and how to present them is the consumer's
/// decision, not a place to guess at UTF-8.
pub fn bcpl_str(block: &[u8], off: usize, max: usize) -> &[u8] {
    let len = (block[off] as usize).min(max).min(block.len() - off - 1);
    &block[off + 1..off + 1 + len]
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn variants_round_trip_and_classify() {
        for byte in 0u32..=7 {
            let v = Variant::from_dostype(DOSTYPE_MAGIC | byte).unwrap();
            assert_eq!(v.dostype(), DOSTYPE_MAGIC | byte);
        }
        assert_eq!(Variant::from_dostype(0x444F_5308), None);
        assert_eq!(Variant::from_dostype(0x5046_5301), None); // PFS\1

        let v7 = Variant::from_dostype(0x444F_5307).unwrap();
        assert!(v7.is_ffs() && v7.is_intl() && v7.has_long_names() && !v7.has_dircache());
        let v0 = Variant::from_dostype(0x444F_5300).unwrap();
        assert!(!v0.is_ffs() && !v0.is_intl() && !v0.has_long_names());
        let v5 = Variant::from_dostype(0x444F_5305).unwrap();
        assert!(v5.has_dircache() && !v5.has_long_names());
    }

    #[test]
    fn max_name_len_follows_has_long_names() {
        for v in [
            Variant::Ofs,
            Variant::Ffs,
            Variant::OfsIntl,
            Variant::FfsIntl,
            Variant::OfsIntlDircache,
            Variant::FfsIntlDircache,
        ] {
            assert_eq!(v.max_name_len(), MAX_NAME_CLASSIC, "{v:?}");
        }
        for v in [Variant::OfsIntlLongname, Variant::FfsIntlLongname] {
            assert_eq!(v.max_name_len(), MAX_NAME_LONG, "{v:?}");
        }
    }

    #[test]
    fn folding_tables_differ_exactly_where_documented() {
        // ASCII folds under both.
        assert_eq!(classic_toupper(b'a'), b'A');
        assert_eq!(intl_toupper(b'a'), b'A');
        // Latin-1 letters fold only under intl.
        assert_eq!(classic_toupper(0xE9), 0xE9); // é stays é
        assert_eq!(intl_toupper(0xE9), 0xC9); // é -> É

        // ÷ (0xF7) is not a letter and never folds.
        assert_eq!(intl_toupper(0xF7), 0xF7);
        // ÿ (0xFF) has no Latin-1 uppercase; the tables leave it alone.
        assert_eq!(intl_toupper(0xFF), 0xFF);
    }

    #[test]
    fn name_hash_is_case_insensitive_per_table() {
        let t = hash_table_size(512);
        assert_eq!(t, 72);
        assert_eq!(
            name_hash(b"Workbench", classic_toupper, t),
            name_hash(b"WORKBENCH", classic_toupper, t)
        );
        // Latin-1 case-insensitivity only under intl folding.
        assert_ne!(
            name_hash(b"caf\xE9", classic_toupper, t),
            name_hash(b"CAF\xC9", classic_toupper, t)
        );
        assert_eq!(
            name_hash(b"caf\xE9", intl_toupper, t),
            name_hash(b"CAF\xC9", intl_toupper, t)
        );
        // And every hash lands inside the table.
        for name in [
            &b"S"[..],
            b"Startup-Sequence",
            b"a very long file name indeed",
        ] {
            assert!(name_hash(name, intl_toupper, t) < t);
        }
    }

    #[test]
    fn hash_table_scales_with_block_size() {
        assert_eq!(hash_table_size(512), 72);
        assert_eq!(hash_table_size(4096), 968);
        assert_eq!(hash_table_size(32768), 8136);
    }

    #[test]
    fn standard_checksum_round_trips_at_any_block_size() {
        for size in [512usize, 4096] {
            let mut block = vec![0u8; size];
            block[0] = 2; // arbitrary content
            block[size - 1] = 0x77;
            let ck = checksum_compute(&block, 5);
            block[20..24].copy_from_slice(&ck.to_be_bytes());
            assert!(checksum_ok(&block), "size {size}");
            // Any corruption breaks it.
            block[100] ^= 1;
            assert!(!checksum_ok(&block));
        }
    }

    #[test]
    fn bootblock_checksum_is_the_carry_algorithm() {
        // A boot block of all-0xFF longs exercises the end-around carry:
        // plain wrapping addition gives a different answer.
        let mut boot = vec![0xFFu8; 1024];
        let ck = bootblock_checksum(&boot);
        boot[4..8].copy_from_slice(&ck.to_be_bytes());
        // Verify by re-running the sum including the stored value.
        let mut sum: u32 = 0;
        for off in (0..1024).step_by(4) {
            let (s, carry) = sum.overflowing_add(be32(&boot, off));
            sum = s.wrapping_add(carry as u32);
        }
        assert_eq!(sum, 0xFFFF_FFFF);
    }

    #[test]
    fn bcpl_str_reads_and_caps() {
        let mut block = vec![0u8; 512];
        block[432] = 3;
        block[433..436].copy_from_slice(b"SYS");
        assert_eq!(bcpl_str(&block, 432, MAX_NAME_CLASSIC), b"SYS");
        // A hostile length byte is capped, never read past.
        block[500] = 200;
        let s = bcpl_str(&block, 500, MAX_NAME_LONG);
        assert!(s.len() <= 11); // clamped to the block edge
    }
}
