//! The read side: find the root block, parse it, and walk directories.
//!
//! Everything here is layered on [`crate::BlockSource`] and the offsets in
//! [`crate::layout`]. Nothing allocates a block per read — a [`Volume`]
//! owns one scratch block and parses out of it — and nothing trusts a
//! pointer it read off the disk: every LBA is range-checked against the
//! volume's block count and every chain walk refuses to revisit a block.
//! A hash chain that points at itself is a shape real damaged volumes
//! take, and a reader that loops forever on one is worse than a reader
//! that says so.

use alloc::collections::BTreeSet;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

use crate::layout::*;
use crate::meta::Protection;
use crate::{bcpl_str, be32, checksum_ok, hash_table_size, name_hash, Variant};
use crate::{MAX_NAME_CLASSIC, MAX_NAME_LONG};

/// Blocks reserved at the start of a volume before the filesystem's own
/// structures: two, on every volume anyone has ever made. It is a mount
/// parameter (`RDB`'s `de_Reserved`), so it is a parameter here too.
pub const DEFAULT_RESERVED: u64 = 2;

/// Where the root block goes on a volume of `block_count` blocks with
/// `reserved` blocks at the front: the midpoint of the usable range,
/// `reserved + (block_count - reserved - 1) / 2`, so that a seek to the
/// root is on average half a disk from anywhere. A 1760-block floppy
/// with two reserved blocks puts it at 880, which is the number every
/// ADF tool hardcodes.
///
/// Returns `None` for a volume too small to have one.
pub fn canonical_root_lba(block_count: u64, reserved: u64) -> Option<u64> {
    if block_count <= reserved {
        return None;
    }
    Some(reserved + (block_count - reserved - 1) / 2)
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Everything the read side can refuse, with the block it refused on.
///
/// Generic over the [`crate::BlockSource`]'s error so the transport's own
/// diagnosis survives — "why did the read fail" is the first question,
/// and a flattened error type cannot answer it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error<E> {
    /// The underlying block source failed.
    Io(E),
    /// A block size with no defined hash-table size: not a power of two
    /// in 512..=32768.
    BadBlockSize(usize),
    /// The source could not say how many blocks it has, and no count was
    /// supplied — the root's location cannot be computed without one.
    UnknownBlockCount,
    /// A volume too small to hold a root block.
    VolumeTooSmall {
        /// Blocks in the volume.
        block_count: u64,
        /// Blocks reserved at the front.
        reserved: u64,
    },
    /// A block pointer read off the disk points outside the volume.
    LbaOutOfRange {
        /// The offending block number.
        lba: u64,
        /// Blocks in the volume.
        block_count: u64,
    },
    /// A block whose longwords do not sum to zero.
    Checksum {
        /// The block that failed.
        lba: u64,
    },
    /// A block that should have been `T_HEADER` and was not.
    NotHeader {
        /// The block read.
        lba: u64,
        /// The primary type found in longword 0.
        found: u32,
    },
    /// A header block with the wrong secondary type for its position.
    WrongSecondaryType {
        /// The block read.
        lba: u64,
        /// The secondary type found.
        found: i32,
        /// The secondary type required.
        expected: i32,
    },
    /// A header block carrying a secondary type this crate does not
    /// know. Refused rather than guessed at: accepting a type whose
    /// layout is unimplemented is this format's signature trap.
    UnknownSecondaryType {
        /// The block read.
        lba: u64,
        /// The secondary type found.
        found: i32,
    },
    /// The block referred to is not something a directory listing can
    /// walk (no hash table).
    NotADirectory {
        /// The block read.
        lba: u64,
        /// Its secondary type.
        found: i32,
    },
    /// The boot block's dostype is not `DOS\0`..`DOS\7` and the caller
    /// named no variant to fall back on.
    UnknownDosType(u32),
    /// The dostype on disk and the variant the caller expected disagree.
    /// Not a warning: the layouts differ, and parsing one as the other
    /// is how a `DOS\7` volume comes back with every name empty.
    VariantMismatch {
        /// What the volume says it is.
        found: Variant,
        /// What the caller said it should be.
        expected: Variant,
        /// Where the disagreeing dostype was read from.
        source: DostypeSource,
    },
    /// A hash chain (or link chain) that revisits a block.
    ChainCycle {
        /// The block reached for the second time.
        lba: u64,
    },
    /// A chain longer than the volume has blocks.
    ChainTooLong {
        /// The block the walk gave up on.
        lba: u64,
    },
    /// A name longer than the variant can store was offered for lookup.
    NameTooLong {
        /// The offered length.
        len: usize,
        /// The variant's maximum.
        max: usize,
    },
    /// A block whose primary type is not the one its position requires —
    /// an extension block that is not `T_LIST`, an OFS data block that is
    /// not `T_DATA`, a comment block that is not `T_COMMENT`.
    WrongBlockType {
        /// The block read.
        lba: u64,
        /// The primary type found in longword 0.
        found: u32,
        /// The primary type required.
        expected: u32,
    },
    /// A block whose longword 1 does not name itself. Every metadata
    /// block records its own number; a block that disagrees was written
    /// somewhere it did not belong, which is the signature of a bad
    /// pointer *into* it rather than corruption within it.
    OwnKeyMismatch {
        /// The block read.
        lba: u64,
        /// The own key found.
        found: u32,
    },
    /// A block that names a different owner than the one that pointed at
    /// it: an extension block whose parent is another file, an OFS data
    /// block whose header key is another file's, a comment block
    /// belonging to another entry.
    BlockOwnerMismatch {
        /// The block read.
        lba: u64,
        /// The owner the block claims.
        found: u32,
        /// The owner that pointed at it.
        expected: u32,
    },
    /// A file header or extension block claiming more data pointers than
    /// its table has slots.
    DataPointerCount {
        /// The block read.
        lba: u64,
        /// The `high_seq` found.
        high_seq: u32,
        /// Slots the table actually has.
        max: u32,
    },
    /// A zero where `high_seq` promised a data block. A hole is not a
    /// sparse file — the format has no such thing — it is a truncated
    /// write, and reading zeroes for it would invent data.
    DataPointerHole {
        /// The header or extension block holding the table.
        lba: u64,
        /// Which data block (1-based) was missing.
        seq: u32,
    },
    /// The file's length and the number of data blocks it has disagree.
    /// Checked in both directions: a chain too short would read a short
    /// file, a chain too long would read blocks the file does not own.
    FileSizeMismatch {
        /// The file header block.
        lba: u64,
        /// The length the header claims.
        byte_size: u32,
        /// Data blocks the chain actually holds.
        blocks: u64,
        /// Data blocks `byte_size` implies.
        expected: u64,
    },
    /// An OFS data block whose sequence number is not its position in the
    /// file. The header's table is authoritative; this is the block's own
    /// disagreement with it.
    DataBlockSequence {
        /// The data block.
        lba: u64,
        /// The sequence number found.
        found: u32,
        /// The sequence number its position requires (1-based).
        expected: u32,
    },
    /// An OFS data block whose payload length is impossible, or short
    /// where it is not the last block of the file.
    DataBlockSize {
        /// The data block.
        lba: u64,
        /// The size found.
        found: u32,
        /// The size its position in the file requires.
        expected: u32,
    },
    /// [`Volume::resolve_link`] was handed something that is not a link
    /// and not a resolved object either.
    NotALink {
        /// The block read.
        lba: u64,
        /// Its secondary type.
        found: i32,
    },
    /// A hard link whose `real_entry` is zero: a link to nothing. The
    /// filesystem never writes one; a validator finding one has found an
    /// interrupted delete.
    LinkTargetMissing {
        /// The link's header block.
        lba: u64,
    },
    /// A dircache block whose record count promises more records than the
    /// block has room for, or whose name/comment length bytes run a
    /// record past the block's end. Refused rather than short-read: the
    /// count and the two length bytes are three independent chances to
    /// walk off the end of a 512-byte buffer.
    DircacheRecordOverflow {
        /// The dircache block.
        lba: u64,
        /// Which record (0-based) did not fit.
        index: u32,
        /// The byte offset it would have started at.
        off: usize,
    },
    /// A soft link reached where a resolved object was required. Not an
    /// error in the volume: soft links store a *path*, and resolving a
    /// path is the caller's job, not this crate's. See
    /// [`Volume::read_softlink`].
    SoftLinkNotResolved {
        /// The soft link's header block.
        lba: u64,
    },
}

/// Which copy of the dostype an [`Error::VariantMismatch`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DostypeSource {
    /// Block 0 of the volume, longword 0.
    BootBlock,
    /// The LNFS root block's `FileSystemType` field (longword −4),
    /// which only long-name volumes fill in.
    RootBlock,
}

impl<E: fmt::Display> fmt::Display for Error<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "block read failed: {e}"),
            Self::BadBlockSize(n) => {
                write!(f, "block size {n} is not a power of two in 512..=32768")
            }
            Self::UnknownBlockCount => f.write_str("volume block count unknown"),
            Self::VolumeTooSmall {
                block_count,
                reserved,
            } => write!(
                f,
                "volume of {block_count} blocks with {reserved} reserved has no room for a root block"
            ),
            Self::LbaOutOfRange { lba, block_count } => write!(
                f,
                "block pointer {lba} is outside the volume ({block_count} blocks)"
            ),
            Self::Checksum { lba } => write!(f, "bad checksum on block {lba}"),
            Self::NotHeader { lba, found } => {
                write!(f, "block {lba} has type {found}, expected T_HEADER (2)")
            }
            Self::WrongSecondaryType {
                lba,
                found,
                expected,
            } => write!(
                f,
                "block {lba} has secondary type {found}, expected {expected}"
            ),
            Self::UnknownSecondaryType { lba, found } => {
                write!(f, "block {lba} has unknown secondary type {found}")
            }
            Self::NotADirectory { lba, found } => write!(
                f,
                "block {lba} (secondary type {found}) is not a directory"
            ),
            Self::UnknownDosType(t) => write!(f, "dostype {t:#010x} is not DOS\\0..DOS\\7"),
            Self::VariantMismatch {
                found,
                expected,
                source,
            } => write!(
                f,
                "{source} says {found:?} ({:#010x}) but the caller expected {expected:?} ({:#010x})",
                found.dostype(),
                expected.dostype()
            ),
            Self::ChainCycle { lba } => write!(f, "chain revisits block {lba}"),
            Self::ChainTooLong { lba } => write!(f, "chain longer than the volume at block {lba}"),
            Self::NameTooLong { len, max } => {
                write!(f, "name of {len} bytes exceeds this variant's {max}")
            }
            Self::WrongBlockType {
                lba,
                found,
                expected,
            } => write!(f, "block {lba} has type {found}, expected {expected}"),
            Self::OwnKeyMismatch { lba, found } => {
                write!(f, "block {lba} calls itself block {found}")
            }
            Self::BlockOwnerMismatch {
                lba,
                found,
                expected,
            } => write!(
                f,
                "block {lba} belongs to block {found}, but block {expected} pointed at it"
            ),
            Self::DataPointerCount { lba, high_seq, max } => write!(
                f,
                "block {lba} claims {high_seq} data pointers, table holds {max}"
            ),
            Self::DataPointerHole { lba, seq } => {
                write!(f, "block {lba} has no pointer for data block {seq}")
            }
            Self::FileSizeMismatch {
                lba,
                byte_size,
                blocks,
                expected,
            } => write!(
                f,
                "file {lba} is {byte_size} bytes ({expected} data blocks) but has {blocks}"
            ),
            Self::DataBlockSequence {
                lba,
                found,
                expected,
            } => write!(
                f,
                "data block {lba} is sequence {found}, expected {expected}"
            ),
            Self::DataBlockSize {
                lba,
                found,
                expected,
            } => write!(
                f,
                "data block {lba} holds {found} bytes, expected {expected}"
            ),
            Self::DircacheRecordOverflow { lba, index, off } => write!(
                f,
                "dircache block {lba} record {index} does not fit at offset {off}"
            ),
            Self::NotALink { lba, found } => write!(
                f,
                "block {lba} (secondary type {found}) is not a link to resolve"
            ),
            Self::LinkTargetMissing { lba } => write!(f, "hard link {lba} points at nothing"),
            Self::SoftLinkNotResolved { lba } => write!(
                f,
                "block {lba} is a soft link; its path is the caller's to resolve"
            ),
        }
    }
}

impl fmt::Display for DostypeSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::BootBlock => "the boot block",
            Self::RootBlock => "the root block",
        })
    }
}

#[cfg(feature = "std")]
impl<E: std::error::Error + 'static> std::error::Error for Error<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Dates
// ---------------------------------------------------------------------------

/// An AmigaDOS `DateStamp`, raw: days since 1978-01-01, minutes past
/// midnight, and ticks (1/50 s) past the minute.
///
/// Stored as the disk stores it — three longwords, no date type, no
/// timezone — because that is what the volume actually contains and a
/// parser that only hands back a converted value has thrown away the
/// bytes. [`DateStamp::to_calendar`] does the conversion when the caller
/// wants it, with the leap-year rules stated rather than assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DateStamp {
    /// Days since 1978-01-01.
    pub days: u32,
    /// Minutes past midnight.
    pub mins: u32,
    /// Ticks (1/50 second) past the minute.
    pub ticks: u32,
}

impl DateStamp {
    fn at(block: &[u8], off: usize) -> Self {
        Self {
            days: be32(block, off),
            mins: be32(block, off + 4),
            ticks: be32(block, off + 8),
        }
    }
}

// ---------------------------------------------------------------------------
// Root block
// ---------------------------------------------------------------------------

/// A parsed root block.
///
/// The bitmap pointers are captured but not followed — reading bitmaps
/// is a later job, and a root that records them is the thing that job
/// will need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootBlock {
    /// Where this root block lives.
    pub lba: u64,
    /// The volume name, raw Latin-1. 30 bytes max on every variant,
    /// long-name volumes included.
    pub name: Vec<u8>,
    /// The hash table, `hash_table_size(block_size)` slots; 0 means an
    /// empty slot.
    pub hash_table: Vec<u32>,
    /// −1 if the bitmap is valid; 0 means a validator must rebuild it.
    pub bitmap_flag: i32,
    /// The 25 bitmap block pointers held in the root, 0-terminated.
    pub bitmap_pages: [u32; BITMAP_PAGES],
    /// Pointer to a bitmap-extension block, or 0.
    pub bitmap_ext: u32,
    /// When the root directory was last altered.
    pub dir_altered: DateStamp,
    /// When any part of the volume was last altered. FFS has a
    /// long-standing bug updating `dir_altered` instead, so this is
    /// frequently stale — recorded, not believed.
    pub disk_altered: DateStamp,
    /// When the volume was formatted.
    pub disk_made: DateStamp,
    /// The directory-cache list (`DOS\4`/`DOS\5`), or 0.
    pub dircache: u32,
    /// LNFS only: blocks allocated per the bitmap.
    pub blocks_used: Option<u32>,
    /// LNFS only: the volume's own dostype, repeated from the boot
    /// block. `None` on classic variants, which leave the field zero.
    pub fs_type: Option<u32>,
}

fn parse_root(block: &[u8], lba: u64, variant: Variant) -> RootBlock {
    let bs = block.len();
    let slots = hash_table_size(bs) as usize;
    let mut hash_table = Vec::with_capacity(slots);
    for i in 0..slots {
        hash_table.push(be32(block, OFF_HASH_TABLE + i * 4));
    }
    let mut bitmap_pages = [0u32; BITMAP_PAGES];
    for (i, page) in bitmap_pages.iter_mut().enumerate() {
        *page = be32(block, tail(bs, TL_BITMAP_PAGES) + i * 4);
    }
    let long = variant.has_long_names();
    RootBlock {
        lba,
        // The root name does *not* move on LNFS volumes: only directory
        // entries got the merged field, and the volume name keeps its
        // 30-byte limit.
        name: bcpl_str(block, tail(bs, TL_ROOT_NAME), MAX_NAME_CLASSIC).to_vec(),
        hash_table,
        bitmap_flag: be32(block, tail(bs, TL_BITMAP_FLAG)) as i32,
        bitmap_pages,
        bitmap_ext: be32(block, tail(bs, TL_BITMAP_EXT)),
        dir_altered: DateStamp::at(block, tail(bs, TL_ROOT_DIR_ALTERED)),
        disk_altered: DateStamp::at(block, tail(bs, TL_ROOT_DISK_ALTERED)),
        disk_made: DateStamp::at(block, tail(bs, TL_ROOT_DISK_MADE)),
        dircache: be32(block, tail(bs, TL_EXTENSION)),
        blocks_used: long.then(|| be32(block, tail(bs, TL_ROOT_NUM_BLOCKS_USED))),
        fs_type: match be32(block, tail(bs, TL_ROOT_FS_TYPE)) {
            0 => None,
            t => Some(t),
        },
    }
}

// ---------------------------------------------------------------------------
// Directory entries
// ---------------------------------------------------------------------------

/// What a directory entry is, by its secondary type.
///
/// The three link kinds are recognised and classified but not resolved
/// here — following them is a separate job with its own loop refusal.
/// Classifying them is still the point: a reader that cannot name what
/// it found should not silently drop it from a listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntryKind {
    /// `ST_USERDIR` (2).
    Directory,
    /// `ST_FILE` (−3).
    File,
    /// `ST_SOFTLINK` (3): a stored path, not a block pointer.
    SoftLink,
    /// `ST_LINKDIR` (4): a hard link to a directory.
    LinkDir,
    /// `ST_LINKFILE` (−4): a hard link to a file.
    LinkFile,
}

impl EntryKind {
    /// Classify a secondary type, or `None` for one this crate has not
    /// implemented a layout for.
    pub fn from_secondary_type(st: i32) -> Option<Self> {
        Some(match st {
            ST_USERDIR => Self::Directory,
            ST_FILE => Self::File,
            ST_SOFTLINK => Self::SoftLink,
            ST_LINKDIR => Self::LinkDir,
            ST_LINKFILE => Self::LinkFile,
            _ => return None,
        })
    }

    /// The secondary type this kind is stored as.
    pub fn secondary_type(self) -> i32 {
        match self {
            Self::Directory => ST_USERDIR,
            Self::File => ST_FILE,
            Self::SoftLink => ST_SOFTLINK,
            Self::LinkDir => ST_LINKDIR,
            Self::LinkFile => ST_LINKFILE,
        }
    }

    /// Whether this kind has a hash table to walk. Hard links to
    /// directories do not: the hash table lives on the target.
    pub fn is_directory(self) -> bool {
        matches!(self, Self::Directory)
    }
}

/// One directory entry, parsed from its header block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The header block's LBA.
    pub lba: u64,
    /// The name, raw Latin-1 bytes. Presenting it is the consumer's
    /// call; guessing UTF-8 here would be a lie about the disk.
    pub name: Vec<u8>,
    /// The comment, raw Latin-1. Empty when there is none — and, on an
    /// LNFS volume whose comment overflowed, when [`Entry::comment_block`]
    /// holds it instead.
    pub comment: Vec<u8>,
    /// What kind of entry this is.
    pub kind: EntryKind,
    /// Longword 1: the block's own key, which should equal `lba`.
    pub own_key: u32,
    /// The parent directory's block.
    pub parent: u32,
    /// File length in bytes; 0 for anything that is not a file.
    pub byte_size: u32,
    /// The full 32-bit protection long — group and other RWED bits
    /// included, not just the low nibble AmigaDOS shows you.
    pub protection: u32,
    /// The raw owner longword: UID in the high word, GID in the low.
    /// Zero on plain FFS, meaningful under muFS, first-class on both.
    pub owner: u32,
    /// The entry's DateStamp.
    pub date: DateStamp,
    /// Next entry in this hash chain, 0 at the end.
    pub hash_chain: u32,
    /// Longword −2: the first file-extension block for a file, the
    /// directory-cache list for a directory.
    pub extension: u32,
    /// Longword −10: first hard link pointing at this entry, or 0.
    pub next_link: u32,
    /// LNFS only: an overflow comment block, or 0. Non-zero means
    /// [`Entry::comment`] is empty because the comment did not fit
    /// beside the name.
    pub comment_block: u32,
    /// Hard links only, longword −11: the block of the object this link
    /// really names. Zero on anything that is not a link — and zero on a
    /// hard link is a link to nothing, which
    /// [`Volume::resolve_link`] refuses rather than reads.
    pub real_entry: u32,
}

impl Entry {
    /// The protection longword as a typed view, with the low nibble's
    /// inverted sense handled once. See [`Protection`].
    pub fn protection_bits(&self) -> Protection {
        Protection::from_bits(self.protection)
    }

    /// The owner's UID: the protection-adjacent longword's high word.
    /// Zero on plain FFS.
    pub fn uid(&self) -> u16 {
        (self.owner >> 16) as u16
    }

    /// The owner's GID: the low word of the same longword.
    pub fn gid(&self) -> u16 {
        self.owner as u16
    }
}

/// Split an LNFS `NaC` field into its two BCPL strings.
///
/// `[name_len][name…][comment_len][comment…]`, laid end to end in 112
/// bytes with no alignment between them. The comment's length byte sits
/// wherever the name ended, which is exactly why a classic reader — one
/// that looks for a name at a *fixed* offset 104 bytes into this field —
/// finds padding and reports nothing.
fn split_nac(nac: &[u8]) -> (&[u8], &[u8]) {
    let name = bcpl_str(nac, 0, MAX_NAME_LONG);
    let after = 1 + name.len();
    let comment = if after < nac.len() {
        bcpl_str(nac, after, COMMENT_MAX)
    } else {
        &[]
    };
    (name, comment)
}

/// A header block's own name, at whichever offset this variant's layout
/// keeps it — classic's fixed `TL_NAME` field, or LNFS's merged `NaC`
/// field where the comment's length byte follows wherever the name
/// happened to end.
///
/// This is the one branch this format's signature trap lives in (see this
/// module's own documentation): reading a long-name volume's name at the
/// classic offset finds padding, not a name. [`parse_entry`] is the
/// authoritative caller; [`crate::populate::Populator`]'s duplicate-name
/// check is the other one, and has a block but no reason to build a whole
/// [`Entry`] just to read one field, which is why this returns a slice
/// rather than something heavier.
pub(crate) fn entry_name(block: &[u8], variant: Variant) -> &[u8] {
    let bs = block.len();
    if variant.has_long_names() {
        let off = tail(bs, TL_NAC);
        split_nac(&block[off..off + NAC_LEN]).0
    } else {
        bcpl_str(block, tail(bs, TL_NAME), MAX_NAME_CLASSIC)
    }
}

fn parse_entry(block: &[u8], lba: u64, variant: Variant) -> Result<Entry, EntryError> {
    let bs = block.len();
    let ty = be32(block, OFF_TYPE);
    if ty != T_HEADER {
        return Err(EntryError::NotHeader(ty));
    }
    let st = be32(block, tail(bs, TL_SECONDARY_TYPE)) as i32;
    let kind = EntryKind::from_secondary_type(st).ok_or(EntryError::UnknownSecondary(st))?;

    let name = entry_name(block, variant).to_vec();
    let (comment, date, comment_block) = if variant.has_long_names() {
        let off = tail(bs, TL_NAC);
        let comment = split_nac(&block[off..off + NAC_LEN]).1;
        (
            comment.to_vec(),
            DateStamp::at(block, tail(bs, TL_DATE_LONG)),
            be32(block, tail(bs, TL_COMMENT_BLOCK)),
        )
    } else {
        (
            bcpl_str(block, tail(bs, TL_COMMENT), COMMENT_MAX).to_vec(),
            DateStamp::at(block, tail(bs, TL_DATE)),
            0,
        )
    };

    Ok(Entry {
        lba,
        name,
        comment,
        kind,
        own_key: be32(block, OFF_OWN_KEY),
        parent: be32(block, tail(bs, TL_PARENT)),
        // Longword −47 is the byte size only in a file header; in a
        // directory it is a spare longword, and reporting whatever it
        // happens to hold as a size would be inventing data.
        byte_size: match kind {
            EntryKind::File | EntryKind::LinkFile => be32(block, tail(bs, TL_BYTE_SIZE)),
            _ => 0,
        },
        protection: be32(block, tail(bs, TL_PROTECTION)),
        owner: be32(block, tail(bs, TL_OWNER)),
        date,
        hash_chain: be32(block, tail(bs, TL_HASH_CHAIN)),
        extension: be32(block, tail(bs, TL_EXTENSION)),
        next_link: be32(block, tail(bs, TL_NEXT_LINK)),
        comment_block,
        // Longword −11 is the link target only in a link block; in a
        // plain file or directory it is a spare longword the filesystem
        // leaves zero, and reporting whatever it holds as a target would
        // be inventing a link.
        real_entry: match kind {
            EntryKind::LinkFile | EntryKind::LinkDir => be32(block, tail(bs, TL_REAL_ENTRY)),
            _ => 0,
        },
    })
}

/// Internal: parse failures that need the LBA attached by the caller.
enum EntryError {
    NotHeader(u32),
    UnknownSecondary(i32),
}

impl EntryError {
    fn at<E>(self, lba: u64) -> Error<E> {
        match self {
            Self::NotHeader(found) => Error::NotHeader { lba, found },
            Self::UnknownSecondary(found) => Error::UnknownSecondaryType { lba, found },
        }
    }
}

// ---------------------------------------------------------------------------
// Boot block
// ---------------------------------------------------------------------------

/// Read the dostype from block 0.
///
/// The root block does not record what variant it is (except on LNFS,
/// which added the field late); the boot block's first longword does,
/// and on a hard-disk partition the RDB's `de_DosType` does too. Both
/// are worth having, because they disagree on real disks.
///
/// The boot block's checksum is deliberately *not* verified: on
/// hard-disk volumes it is routinely left zero, and refusing to mount
/// over that would refuse most real partitions.
pub fn read_boot_dostype<S: crate::BlockSource>(src: &mut S) -> Result<u32, Error<S::Error>> {
    let bs = src.block_size();
    if !block_size_ok(bs) {
        return Err(Error::BadBlockSize(bs));
    }
    let mut buf = vec![0u8; bs];
    src.read_block(0, &mut buf).map_err(Error::Io)?;
    Ok(be32(&buf, 0))
}

// ---------------------------------------------------------------------------
// Volume
// ---------------------------------------------------------------------------

/// An opened volume: a block source, the variant it is being read as,
/// and its parsed root.
pub struct Volume<S: crate::BlockSource> {
    pub(crate) src: S,
    pub(crate) variant: Variant,
    pub(crate) block_size: usize,
    pub(crate) block_count: u64,
    pub(crate) reserved: u64,
    pub(crate) root: RootBlock,
    pub(crate) buf: Vec<u8>,
}

impl<S: crate::BlockSource> Volume<S> {
    /// Open a volume, taking its geometry from the source.
    ///
    /// `expect` is the dostype the *caller* believes the partition has —
    /// from an RDB entry, a mount file, or the user. It is checked
    /// against the boot block's, and against the LNFS root's own copy
    /// where present. Passing `None` means "believe the disk", which
    /// works whenever the boot block carries a recognisable `DOS\x`.
    pub fn open(src: S, expect: Option<Variant>) -> Result<Self, Error<S::Error>> {
        let count = src.block_count().ok_or(Error::UnknownBlockCount)?;
        Self::open_with(src, expect, count, DEFAULT_RESERVED)
    }

    /// Open a volume with an explicit block count and reserved-block
    /// count — the form a partition table drives, where the geometry is
    /// the RDB's to state and not the block device's to guess.
    pub fn open_with(
        mut src: S,
        expect: Option<Variant>,
        block_count: u64,
        reserved: u64,
    ) -> Result<Self, Error<S::Error>> {
        let root_lba = canonical_root_lba(block_count, reserved).ok_or(Error::VolumeTooSmall {
            block_count,
            reserved,
        })?;
        let dostype = read_boot_dostype(&mut src)?;
        let variant = resolve_variant(dostype, expect)?;
        let mut vol = Self::open_at_root(src, variant, block_count, root_lba)?;
        vol.reserved = reserved;
        Ok(vol)
    }

    /// Open a volume whose root block is already known — for recovery,
    /// where the geometry is what is in doubt and the root has been
    /// found by search. The variant must be stated: with the geometry
    /// unknown, the boot block may not be where this code would look.
    pub fn open_at_root(
        mut src: S,
        variant: Variant,
        block_count: u64,
        root_lba: u64,
    ) -> Result<Self, Error<S::Error>> {
        let block_size = src.block_size();
        if !block_size_ok(block_size) {
            return Err(Error::BadBlockSize(block_size));
        }
        if root_lba >= block_count {
            return Err(Error::LbaOutOfRange {
                lba: root_lba,
                block_count,
            });
        }
        let mut buf = vec![0u8; block_size];
        src.read_block(root_lba, &mut buf).map_err(Error::Io)?;
        verify_root_block(&buf, root_lba)?;
        let root = parse_root(&buf, root_lba, variant);

        // LNFS roots repeat the dostype. When it is there, it is the
        // volume's own word on what it is, and it outranks a guess.
        if let Some(t) = root.fs_type {
            match Variant::from_dostype(t) {
                Some(found) if found != variant => {
                    return Err(Error::VariantMismatch {
                        found,
                        expected: variant,
                        source: DostypeSource::RootBlock,
                    })
                }
                _ => {}
            }
        }

        Ok(Self {
            src,
            variant,
            block_size,
            block_count,
            reserved: DEFAULT_RESERVED,
            root,
            buf,
        })
    }

    /// Re-read and re-parse the root block.
    ///
    /// The cached [`RootBlock`] is a parse of the bytes as they were at
    /// open time, and every read path here trusts it — the hash table
    /// most of all. Anything that *writes* the root (a repair rewriting
    /// its bitmap pointers, a mutator changing a hash slot) has therefore
    /// invalidated it, and this is how it says so. Deliberately not
    /// automatic: an open volume is a read-side object, and reloading
    /// after every write would put a read of the root in front of every
    /// operation that could not have changed it.
    pub(crate) fn reload_root(&mut self) -> Result<(), Error<S::Error>> {
        let lba = self.root.lba;
        self.read_raw(lba)?;
        verify_root_block(&self.buf, lba)?;
        self.root = parse_root(&self.buf, lba, self.variant);
        Ok(())
    }

    /// Blocks reserved at the front of the volume, before the bitmap's
    /// first bit and before anything the filesystem allocates.
    ///
    /// [`Volume::open_with`] takes it from the caller (an RDB's
    /// `de_Reserved`); the other constructors assume
    /// [`DEFAULT_RESERVED`], which is what every volume anyone has made
    /// actually uses. It matters here rather than only at open time
    /// because the bitmap's first bit is block `reserved`, so a wrong
    /// value shifts every allocation answer.
    pub fn reserved(&self) -> u64 {
        self.reserved
    }

    /// Correct the reserved-block count on an already-open volume — for
    /// recovery, where the geometry is what is in doubt.
    pub fn set_reserved(&mut self, reserved: u64) {
        self.reserved = reserved;
    }

    /// The variant this volume is being read as.
    pub fn variant(&self) -> Variant {
        self.variant
    }

    /// The filesystem block size.
    pub fn block_size(&self) -> usize {
        self.block_size
    }

    /// The volume's block count.
    pub fn block_count(&self) -> u64 {
        self.block_count
    }

    /// The parsed root block.
    pub fn root(&self) -> &RootBlock {
        &self.root
    }

    /// The root block's LBA.
    pub fn root_lba(&self) -> u64 {
        self.root.lba
    }

    /// Borrow the underlying source (for a bitmap reader, a validator,
    /// or anything else that needs raw blocks).
    pub fn source_mut(&mut self) -> &mut S {
        &mut self.src
    }

    /// Give the source back.
    pub fn into_inner(self) -> S {
        self.src
    }

    /// The longest name this volume's variant can store.
    pub fn max_name_len(&self) -> usize {
        self.variant.max_name_len()
    }

    /// Read `lba` into the scratch buffer, range-checked, with no
    /// checksum: for FFS data blocks, which are raw payload with no
    /// checksum longword to verify.
    pub(crate) fn read_raw(&mut self, lba: u64) -> Result<(), Error<S::Error>> {
        if lba >= self.block_count {
            return Err(Error::LbaOutOfRange {
                lba,
                block_count: self.block_count,
            });
        }
        self.src.read_block(lba, &mut self.buf).map_err(Error::Io)
    }

    /// Read `lba` into the scratch buffer and verify its checksum.
    pub(crate) fn read_checked(&mut self, lba: u64) -> Result<(), Error<S::Error>> {
        self.read_raw(lba)?;
        if !checksum_ok(&self.buf) {
            return Err(Error::Checksum { lba });
        }
        Ok(())
    }

    /// Read one directory entry's header block by LBA.
    pub fn entry_at(&mut self, lba: u64) -> Result<Entry, Error<S::Error>> {
        self.read_checked(lba)?;
        parse_entry(&self.buf, lba, self.variant).map_err(|e| e.at(lba))
    }

    /// The hash table of a directory — the root, or any `ST_USERDIR`.
    ///
    /// Copied out rather than borrowed: the caller is about to walk it,
    /// and every step of that walk needs the scratch buffer back.
    pub fn hash_table(&mut self, dir_lba: u64) -> Result<Vec<u32>, Error<S::Error>> {
        if dir_lba == self.root.lba {
            return Ok(self.root.hash_table.clone());
        }
        self.read_checked(dir_lba)?;
        let ty = be32(&self.buf, OFF_TYPE);
        if ty != T_HEADER {
            return Err(Error::NotHeader {
                lba: dir_lba,
                found: ty,
            });
        }
        let st = be32(&self.buf, tail(self.block_size, TL_SECONDARY_TYPE)) as i32;
        if st != ST_USERDIR && st != ST_ROOT {
            return Err(Error::NotADirectory {
                lba: dir_lba,
                found: st,
            });
        }
        let slots = hash_table_size(self.block_size) as usize;
        let mut table = Vec::with_capacity(slots);
        for i in 0..slots {
            table.push(be32(&self.buf, OFF_HASH_TABLE + i * 4));
        }
        Ok(table)
    }

    /// Find `name` in the directory at `dir_lba`, or `Ok(None)`.
    ///
    /// Hashes with the volume's own fold table, then walks that one
    /// chain comparing folded bytes — the same fold on both sides, which
    /// is the whole reason [`Variant::fold`] exists. Hashing with the
    /// wrong table finds nothing while the file plainly exists; comparing
    /// with the wrong one finds the file only when the case matches.
    pub fn lookup(&mut self, dir_lba: u64, name: &[u8]) -> Result<Option<Entry>, Error<S::Error>> {
        let max = self.max_name_len();
        if name.len() > max {
            return Err(Error::NameTooLong {
                len: name.len(),
                max,
            });
        }
        let fold = self.variant.fold();
        let slot = name_hash(name, fold, hash_table_size(self.block_size)) as usize;
        let table = self.hash_table(dir_lba)?;
        let mut next = table[slot];
        let mut visited: BTreeSet<u64> = BTreeSet::new();
        while next != 0 {
            let lba = next as u64;
            self.guard_chain(&mut visited, lba)?;
            let entry = self.entry_at(lba)?;
            if names_equal(&entry.name, name, fold) {
                return Ok(Some(entry));
            }
            next = entry.hash_chain;
        }
        Ok(None)
    }

    /// Every entry in the directory at `dir_lba`, slot by slot and chain
    /// by chain.
    ///
    /// Order is the hash table's, not alphabetical and not creation
    /// order — the same order `List` shows, and not something to sort
    /// behind the caller's back.
    pub fn read_dir(&mut self, dir_lba: u64) -> Result<Vec<Entry>, Error<S::Error>> {
        let table = self.hash_table(dir_lba)?;
        let mut out = Vec::new();
        // One visited set across every chain in the directory: a block
        // reachable from two slots is corruption too, and would
        // otherwise be listed twice.
        let mut visited: BTreeSet<u64> = BTreeSet::new();
        for slot in table {
            let mut next = slot;
            while next != 0 {
                let lba = next as u64;
                self.guard_chain(&mut visited, lba)?;
                let entry = self.entry_at(lba)?;
                next = entry.hash_chain;
                out.push(entry);
            }
        }
        Ok(out)
    }

    /// Resolve a `/`-separated path relative to `dir_lba`. Empty
    /// components are skipped, so a trailing slash is harmless.
    ///
    /// Links are **not** followed. A hard link component is returned as
    /// the link entry it is ([`Volume::resolve_link`] follows it); a soft
    /// link is returned as itself, because resolving one means re-entering
    /// path resolution possibly on another volume, through assigns and
    /// device names this crate has never heard of. That is `DosPacket`
    /// work and lives in the consumer — see [`Volume::read_softlink`].
    /// Walking *through* a link mid-path therefore stops at the link, and
    /// the caller decides what to do next.
    pub fn lookup_path(
        &mut self,
        dir_lba: u64,
        path: &[u8],
    ) -> Result<Option<Entry>, Error<S::Error>> {
        let mut here = dir_lba;
        let mut found = None;
        for component in path.split(|&c| c == b'/').filter(|c| !c.is_empty()) {
            let entry = match self.lookup(here, component)? {
                Some(e) => e,
                None => return Ok(None),
            };
            here = entry.lba;
            found = Some(entry);
        }
        Ok(found)
    }

    /// Refuse a chain that revisits a block or outruns the volume.
    ///
    /// Both, not either: the visited set catches the self-pointing chain
    /// a damaged volume actually produces, and the length bound keeps
    /// that set from growing without limit on a volume whose blocks all
    /// point somewhere new.
    ///
    /// `visited` is a [`BTreeSet`], not a `Vec`: a `Vec` makes every call
    /// here an O(so-far) scan, so a long, perfectly valid, non-cyclic
    /// chain of `n` blocks (a hostile image's cheapest way to slow this
    /// crate down, since it never has to trip `ChainCycle` to do it)
    /// walks in O(n^2). A `BTreeSet` makes each call O(log n), the whole
    /// walk O(n log n), without depending on a hasher this `no_std`
    /// crate would have to bring in itself.
    pub(crate) fn guard_chain(
        &self,
        visited: &mut BTreeSet<u64>,
        lba: u64,
    ) -> Result<(), Error<S::Error>> {
        if visited.contains(&lba) {
            return Err(Error::ChainCycle { lba });
        }
        if visited.len() as u64 >= self.block_count {
            return Err(Error::ChainTooLong { lba });
        }
        visited.insert(lba);
        Ok(())
    }
}

/// Compare two names under one fold table, the way the filesystem does.
pub fn names_equal(a: &[u8], b: &[u8], fold: fn(u8) -> u8) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| fold(x) == fold(y))
}

/// Structural check that a block is a root block: `T_HEADER` at
/// longword 0, `ST_ROOT` at longword −1, and a checksum that balances.
///
/// Offered separately because "is this a root block" is the question a
/// recovery scan asks of every block on a disk whose geometry is gone.
pub fn verify_root_block<E>(block: &[u8], lba: u64) -> Result<(), Error<E>> {
    if !block_size_ok(block.len()) {
        return Err(Error::BadBlockSize(block.len()));
    }
    let ty = be32(block, OFF_TYPE);
    if ty != T_HEADER {
        return Err(Error::NotHeader { lba, found: ty });
    }
    let st = be32(block, tail(block.len(), TL_SECONDARY_TYPE)) as i32;
    if st != ST_ROOT {
        return Err(Error::WrongSecondaryType {
            lba,
            found: st,
            expected: ST_ROOT,
        });
    }
    if !checksum_ok(block) {
        return Err(Error::Checksum { lba });
    }
    Ok(())
}

/// Reconcile the dostype found on disk with the one the caller expected.
///
/// Three outcomes, all of them deliberate: they agree, or only one of
/// them exists and it is used, or they disagree and this is an error
/// rather than a preference. Two independent readers have shipped the
/// "prefer the classic interpretation" version of this decision, and
/// both return empty names on `DOS\7` volumes because of it.
pub fn resolve_variant<E>(dostype: u32, expect: Option<Variant>) -> Result<Variant, Error<E>> {
    match (Variant::from_dostype(dostype), expect) {
        (Some(found), Some(expected)) if found != expected => Err(Error::VariantMismatch {
            found,
            expected,
            source: DostypeSource::BootBlock,
        }),
        (Some(found), _) => Ok(found),
        // An unrecognised boot dostype with a stated expectation is
        // normal on hard disks: the partition's dostype is the RDB's,
        // and the boot block may never have been written.
        (None, Some(expected)) => Ok(expected),
        (None, None) => Err(Error::UnknownDosType(dostype)),
    }
}
