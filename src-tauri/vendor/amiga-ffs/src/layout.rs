//! On-disk block layout: the offsets, and *why* they are where they are.
//!
//! Every FFS metadata block is split in two halves that grow towards
//! each other. The head — type, own key, sequence, size, first data,
//! checksum, then the hash or data-pointer table — is indexed from
//! longword 0 forward. The tail — dates, name, comment, protection,
//! hash chain, secondary type — is indexed from the *end* of the block,
//! because the table in the middle changes length with the block size
//! (`block_size/4 - 56` entries) and everything after it would otherwise
//! move. Hence [`tail`]: a field at "longword −20" lives at
//! `block_size - 80`, on a 512-byte block and on a 32 KB one alike.
//!
//! The two halves meet exactly: the head occupies `24 + 4*(bs/4 - 56)`
//! = `bs - 200` bytes, and the first tail field ([`TL_BITMAP_FLAG`],
//! longword −50) starts at `bs - 200`. Any block size from 512 to 32768
//! keeps that identity, which is the whole reason the format indexes the
//! tail backwards.
//!
//! # Two name layouts
//!
//! Classic variants (`DOS\0`–`DOS\5`) store the comment as a BCPL string
//! at longword −46 (80 bytes used) and the name as a BCPL string at
//! longword −20 (32 bytes, 30 characters usable), with the entry's
//! DateStamp between them at −23.
//!
//! Long-name variants (`DOS\6`/`DOS\7`, "LNFS", added with the OS 3.1.4
//! / 3.2-era FastFileSystem reimplementation) merge those into one
//! 112-byte field at longword −46, [`TL_NAC`] — *N*ame *a*nd *C*omment,
//! two BCPL strings back to back — and push the DateStamp down to
//! longword −15, freeing −18 for a [`TL_COMMENT_BLOCK`] pointer used
//! when the comment no longer fits beside a long name.
//!
//! This is the trap the crate exists to not fall into. A reader using
//! the classic name offset on an LNFS volume reads byte 104 of `NaC` as
//! a length byte — padding, so zero — and reports **every entry's name
//! as empty**. Two independent implementations do exactly that. The
//! layouts below are transcribed from the AmigaOS `LongName*Block`
//! structure definitions, and the field-by-field arithmetic that proves
//! each offset is in this module's tests.

/// `T_HEADER` / `TYPE_SHORT`: root, directory, file-header and hard-link
/// blocks all carry this in longword 0.
pub const T_HEADER: u32 = 2;
/// `T_LIST`: file extension (file-list) blocks.
pub const T_LIST: u32 = 16;
/// `T_DATA`: OFS data blocks (FFS data blocks have no header at all).
pub const T_DATA: u32 = 8;
/// `T_DIRCACHE`: `DOS\4`/`DOS\5` directory-cache blocks.
pub const T_DIRCACHE: u32 = 33;
/// `TYPE_COMMENT`: the LNFS overflow-comment block referenced by
/// [`TL_COMMENT_BLOCK`].
pub const T_COMMENT: u32 = 64;

/// Secondary type: the volume root.
pub const ST_ROOT: i32 = 1;
/// Secondary type: a directory.
pub const ST_USERDIR: i32 = 2;
/// Secondary type: a soft link (a stored path, resolved by the caller).
pub const ST_SOFTLINK: i32 = 3;
/// Secondary type: a hard link to a directory.
pub const ST_LINKDIR: i32 = 4;
/// Secondary type: a file. Negative, as the FileInfoBlock contract
/// requires — `ST_FILE` is `-3`, not `0xFFFFFFFD` by accident.
pub const ST_FILE: i32 = -3;
/// Secondary type: a hard link to a file.
pub const ST_LINKFILE: i32 = -4;

// --- head, indexed forward from longword 0 -------------------------------

/// Longword 0: primary block type.
pub const OFF_TYPE: usize = 0;
/// Longword 1: the block's own number (0 in the root).
pub const OFF_OWN_KEY: usize = 4;
/// Longword 2: high sequence number — data-pointer count in a file
/// header, unused (0) in a directory or the root.
pub const OFF_HIGH_SEQ: usize = 8;
/// Longword 3: hash-table size in the root, unused elsewhere.
pub const OFF_HASH_TABLE_SIZE: usize = 12;
/// Longword 4: first data block, unused (0) in the root.
pub const OFF_FIRST_DATA: usize = 16;
/// Longword 5: the block checksum.
pub const OFF_CHECKSUM: usize = 20;
/// The checksum's longword index, for [`crate::checksum_compute`].
pub const CHECKSUM_INDEX: usize = 5;
/// Longword 6: the first hash-table slot (or data-pointer slot).
pub const OFF_HASH_TABLE: usize = 24;

// --- tail, indexed backward from the end ---------------------------------

/// Root only, longword −50: bitmap valid flag (−1 valid, 0 invalid).
pub const TL_BITMAP_FLAG: usize = 50;
/// Root only, longword −49: the first of [`BITMAP_PAGES`] bitmap block
/// pointers (−49..=−25).
pub const TL_BITMAP_PAGES: usize = 49;
/// How many bitmap block pointers the root itself holds.
pub const BITMAP_PAGES: usize = 25;
/// Root only, longword −24: pointer to a bitmap-extension block.
///
/// Occupying the same longword that a directory uses for
/// [`TL_PROTECTION`] is the historical reason old filesystems capped
/// partitions near 53 MB: they wrote protection bits into the root and
/// clobbered this pointer.
pub const TL_BITMAP_EXT: usize = 24;
/// Root only, longword −23: date the root directory was last altered.
pub const TL_ROOT_DIR_ALTERED: usize = 23;
/// Root only, longword −20: the volume name, BCPL, 30 characters max —
/// *including* on LNFS volumes, where the root name field does not move
/// and the 30-character limit still applies.
pub const TL_ROOT_NAME: usize = 20;
/// Root, LNFS only, longword −11: blocks allocated per the bitmap.
/// Classic roots have the volume name's padding here.
pub const TL_ROOT_NUM_BLOCKS_USED: usize = 11;
/// Root only, longword −10: date any part of the volume was last
/// altered.
pub const TL_ROOT_DISK_ALTERED: usize = 10;
/// Root only, longword −7: date the volume was formatted.
pub const TL_ROOT_DISK_MADE: usize = 7;
/// Root, LNFS only, longword −4: the volume's own dostype, repeating
/// the boot block's signature. Zero on every non-LNFS variant — which
/// makes a non-zero value here a free cross-check that the dostype the
/// caller was handed matches the volume actually on disk.
pub const TL_ROOT_FS_TYPE: usize = 4;

/// Longword −49: owner UID (high word) and GID (low word), as one raw
/// longword. Zero on plain FFS; meaningful under muFS.
pub const TL_OWNER: usize = 49;
/// Longword −48: the full 32-bit protection long, group and other bits
/// included.
pub const TL_PROTECTION: usize = 48;
/// File headers only, longword −47: the file's length in bytes.
pub const TL_BYTE_SIZE: usize = 47;
/// Classic layout, longword −46: the comment, BCPL, 79 characters max.
pub const TL_COMMENT: usize = 46;
/// Longest comment AmigaDOS will store.
pub const COMMENT_MAX: usize = 79;
/// Classic layout, longword −23: the entry's DateStamp.
pub const TL_DATE: usize = 23;
/// Classic layout, longword −20: the entry name, BCPL, 30 characters.
pub const TL_NAME: usize = 20;

/// LNFS layout, longword −46: the merged name-and-comment field, two
/// BCPL strings laid end to end in [`NAC_LEN`] bytes. Spans longwords
/// −46..=−19 inclusive (28 longwords), swallowing the classic comment
/// field, the spare longwords, the classic DateStamp *and* the classic
/// name field.
pub const TL_NAC: usize = 46;
/// The merged field's length. 112 bytes holds a 107-byte name
/// (`1 + 107`) with four bytes left for a short comment's length byte
/// and up to three characters; longer comments go to a
/// [`TL_COMMENT_BLOCK`].
pub const NAC_LEN: usize = 112;
/// LNFS layout, longword −18: block number of an overflow comment
/// block ([`T_COMMENT`]), or 0.
pub const TL_COMMENT_BLOCK: usize = 18;
/// LNFS layout, longword −15: the entry's DateStamp, moved down from
/// −23 to make room for [`TL_NAC`].
pub const TL_DATE_LONG: usize = 15;

/// Hard-link blocks only, longword −11: the block of the *real* object
/// this link names — `original` in Linux's `struct affs_tail`,
/// `realEntry` in ADFlib's `bLinkBlock`. The link's own header block
/// carries name, dates and protection of its own; everything else
/// (data, hash table) lives on the target.
///
/// Immediately before [`TL_NEXT_LINK`] in both layouts: the classic tail
/// runs `…name[32], spare, original, link_chain, spare[5], hash_chain…`
/// and the LNFS tail runs `…Created[3], Spare5[2], FirstLink,
/// Spare6[5], HashChain…`, so LNFS's second `Spare5` longword is exactly
/// this field — a plain directory is nobody's link and leaves it zero.
pub const TL_REAL_ENTRY: usize = 11;
/// Longword −10: first hard link pointing at this entry (`next_link` in
/// the classic naming). Same place in both layouts.
///
/// On a *link* block this is the next link in the chain of links to the
/// same object; on the target it is the head of that chain. Following it
/// enumerates every name an object has — it does not resolve anything,
/// which is [`TL_REAL_ENTRY`]'s job.
pub const TL_NEXT_LINK: usize = 10;
/// Longword −4: next entry in this hash chain, 0 at the end. Zero in a
/// root block, which is nobody's directory entry.
pub const TL_HASH_CHAIN: usize = 4;
/// Longword −3: the parent directory's block. Zero in a root block.
pub const TL_PARENT: usize = 3;
/// Longword −2: extension — the first file-extension block for a file,
/// the directory-cache list for a directory or the root.
pub const TL_EXTENSION: usize = 2;
/// Longword −1: the secondary type (`ST_*`).
pub const TL_SECONDARY_TYPE: usize = 1;

// --- file data: extension blocks, OFS data blocks ------------------------

/// A file header's (and file-extension block's) data-pointer table shares
/// the hash table's space, longword 6 onward — but is filled from the
/// *end backwards*. Slot `n-1` holds data block 1, slot `n-2` data block
/// 2, and so on for [`OFF_HIGH_SEQ`] entries.
///
/// Backwards because the filesystem appends: a new pointer goes in the
/// next slot *down*, so the block's used region grows towards the head
/// and the table never has to be shuffled. A reader that walks the table
/// forwards gets the file's blocks in reverse, which for most files still
/// produces the right *number* of blocks and the wrong bytes.
///
/// The table holds [`hash_table_size`](crate::hash_table_size()) entries —
/// 72 at 512 bytes, so ~36 KB of FFS data before the first extension
/// block is needed, ~35 KB of OFS data.
#[inline]
pub fn data_pointer_offset(block_size: usize, seq: u32) -> usize {
    let slots = crate::hash_table_size(block_size);
    OFF_HASH_TABLE + (slots - seq) as usize * 4
}

/// OFS data block, longword 1: the *file header* block this data belongs
/// to. Redundant with the header's own table, and precisely because it is
/// redundant it is worth checking — it catches a table pointing into
/// another file's data.
pub const OFF_DATA_HEADER_KEY: usize = 4;
/// OFS data block, longword 2: the block's sequence number in the file,
/// counting from **1**.
pub const OFF_DATA_SEQ: usize = 8;
/// OFS data block, longword 3: bytes of payload in this block,
/// `<= block_size - 24`. Only the last block of a file may be short.
pub const OFF_DATA_SIZE: usize = 12;
/// OFS data block, longword 4: the next data block, 0 at the end.
/// Redundant with the header's table, which this crate treats as
/// authoritative; recorded, not enforced.
pub const OFF_DATA_NEXT: usize = 16;
/// OFS data block: where the payload starts, after the six-longword
/// header. FFS data blocks have no header at all and start at 0.
pub const OFF_DATA_PAYLOAD: usize = 24;

/// Payload bytes per data block: the whole block on FFS, 24 fewer on OFS.
/// This is the single number that makes the same file occupy a different
/// number of blocks on `DOS\0` and `DOS\1`.
#[inline]
pub fn data_payload_size(block_size: usize, is_ffs: bool) -> usize {
    if is_ffs {
        block_size
    } else {
        block_size - OFF_DATA_PAYLOAD
    }
}

// --- LNFS overflow comment block -----------------------------------------

/// [`T_COMMENT`] block, longword 2: the header block whose comment this
/// is. (`struct CommentBlock`'s `HeaderKey`.)
pub const OFF_COMMENT_HEADER_KEY: usize = 8;
/// [`T_COMMENT`] block: the comment itself, a BCPL string in 80 bytes,
/// starting where every other block's table starts.
pub const OFF_COMMENT_TEXT: usize = 24;

// --- soft links ----------------------------------------------------------

/// An `ST_SOFTLINK` block stores a *path*, not a block pointer, as a
/// NUL-terminated string in the space a directory uses for its hash table
/// — `struct slink_front`'s `symname`, longword 6 onward, running to the
/// start of the tail (`block_size - 200`).
///
/// A path, not a pointer, is the whole difference between the two link
/// kinds: a hard link is resolved by the filesystem, a soft link by
/// whatever is doing path resolution — which on AmigaDOS is the caller,
/// via the `ERROR_IS_SOFT_LINK` packet dance.
pub const OFF_SOFTLINK_PATH: usize = OFF_HASH_TABLE;

/// Bytes available for a soft link's path on a `block_size` block:
/// everything between the head and the tail.
#[inline]
pub fn softlink_path_capacity(block_size: usize) -> usize {
    block_size - 200 - OFF_SOFTLINK_PATH
}

// --- directory cache blocks (DOS\4 / DOS\5) ------------------------------

/// [`T_DIRCACHE`] block, longword 2: the directory this cache describes.
/// (`parent` in ADFlib's `bDirCacheBlock`.)
///
/// Note where the *pointer* to this block lives: longword −2, the same
/// [`TL_EXTENSION`] field a file header uses for its first `T_LIST` block.
/// The two never collide because the field's meaning follows the block's
/// secondary type — a directory or root has no data chain to extend, and
/// a file has no directory to cache — which is why the format could
/// afford to reuse it when `DOS\4` was added.
pub const OFF_DIRCACHE_PARENT: usize = 8;
/// [`T_DIRCACHE`] block, longword 3: how many records follow.
pub const OFF_DIRCACHE_RECORDS: usize = 12;
/// [`T_DIRCACHE`] block, longword 4: the next cache block for the same
/// directory, or 0. One directory's cache is a *chain* of these; a
/// directory with more entries than one block's records will hold spills
/// onward.
pub const OFF_DIRCACHE_NEXT: usize = 16;
/// [`T_DIRCACHE`] block: where the packed records start, after the same
/// six-longword head every other block has (the checksum still at
/// longword 5, [`CHECKSUM_INDEX`]).
pub const OFF_DIRCACHE_RECORDS_START: usize = 24;

/// Fixed part of a dircache record, before the two counted strings:
/// entry block (long), size (long), protection (long), UID (word), GID
/// (word), days/mins/ticks (three **words**, not longwords — the one
/// place the format stores a `DateStamp` narrowed), the entry's secondary
/// type as a signed byte, and the name's length byte.
///
/// So a record is `24 + name_len` bytes, then a comment length byte and
/// its bytes: [`DIRCACHE_RECORD_MIN`]` + name_len + comment_len`, rounded
/// **up to an even length** — records are word-aligned, because the
/// fields inside them are.
pub const DIRCACHE_RECORD_FIXED: usize = 24;
/// The shortest a dircache record can be: the fixed part plus the
/// comment's length byte, with both strings empty.
pub const DIRCACHE_RECORD_MIN: usize = DIRCACHE_RECORD_FIXED + 1;

/// Byte offset within a dircache record: the entry's header block.
pub const DC_ENTRY: usize = 0;
/// Byte offset within a dircache record: the file's length in bytes
/// (0 for a directory).
pub const DC_SIZE: usize = 4;
/// Byte offset within a dircache record: the protection longword.
pub const DC_PROTECTION: usize = 8;
/// Byte offset within a dircache record: the owner UID (a *word* here,
/// where a header block holds UID and GID as one longword).
pub const DC_UID: usize = 12;
/// Byte offset within a dircache record: the owner GID.
pub const DC_GID: usize = 14;
/// Byte offset within a dircache record: the date's day count, as a word.
pub const DC_DAYS: usize = 16;
/// Byte offset within a dircache record: minutes past midnight, a word.
pub const DC_MINS: usize = 18;
/// Byte offset within a dircache record: ticks past the minute, a word.
/// A minute is 3000 ticks, so the narrowing costs nothing here — unlike
/// [`DC_DAYS`], which runs out in 2157.
pub const DC_TICKS: usize = 20;
/// Byte offset within a dircache record: the entry's secondary type,
/// narrowed to a signed byte (`2` for a directory, `-3` for a file).
pub const DC_TYPE: usize = 22;
/// Byte offset within a dircache record: the name's length byte, with
/// the name itself at [`DIRCACHE_RECORD_FIXED`].
pub const DC_NAME_LEN: usize = 23;

/// The length of a dircache record with these two string lengths, padded
/// to the word boundary the next record starts on.
#[inline]
pub fn dircache_record_len(name_len: usize, comment_len: usize) -> usize {
    let raw = DIRCACHE_RECORD_MIN + name_len + comment_len;
    raw + (raw & 1)
}

// --- bitmap blocks and bitmap extension blocks ---------------------------

/// A bitmap block's checksum lives in longword **0**, not longword 5.
///
/// This is the format's one exception, and it is an easy one to get
/// wrong in the direction that still passes [`crate::checksum_ok`]:
/// verification never needs the index (the stored value participates in
/// the sum wherever it sits), so a writer using index 5 produces blocks
/// that verify against themselves and against nothing else. A bitmap
/// block has no type longword and no own-key — the checksum is the whole
/// header, and the rest of the block is bits.
pub const BITMAP_CHECKSUM_INDEX: usize = 0;
/// A bitmap block: where the bits start, immediately after the checksum.
pub const OFF_BITMAP_BITS: usize = 4;

/// How many blocks one bitmap block accounts for: every bit of every
/// longword after the checksum. 4064 at 512 bytes, 32704 at 4 KB.
#[inline]
pub fn bitmap_bits_per_block(block_size: usize) -> u64 {
    (block_size - OFF_BITMAP_BITS) as u64 * 8
}

/// How many bitmap-page pointers a bitmap **extension** block holds:
/// every longword but the last, which is the chain to the next extension
/// block. 127 at 512 bytes.
///
/// An extension block has no type longword, no own key and **no
/// checksum** — it is a bare array of pointers. Confirmed by dumping one
/// out of an image xdftool built: a 60 MB volume's 31 bitmap pages are 25
/// in the root and 6 in an extension block whose longwords do not sum to
/// zero.
#[inline]
pub fn bitmap_ext_pointers(block_size: usize) -> usize {
    block_size / 4 - 1
}

/// A bitmap extension block: the byte offset of its next-block pointer,
/// the last longword.
#[inline]
pub fn bitmap_ext_next(block_size: usize) -> usize {
    block_size - 4
}

/// Smallest filesystem block size the format allows.
pub const MIN_BLOCK_SIZE: usize = 512;
/// Largest filesystem block size the format allows.
pub const MAX_BLOCK_SIZE: usize = 32768;

/// Byte offset of the field `lw_from_end` longwords before the end of a
/// `block_size`-byte block: `TL_NAME` (20) on 512 bytes is byte 432, on
/// 4096 bytes byte 4016.
#[inline]
pub fn tail(block_size: usize, lw_from_end: usize) -> usize {
    block_size - lw_from_end * 4
}

/// Is `block_size` one this crate will parse: a power of two in
/// 512..=32768? Anything else has no defined hash-table size, and
/// guessing one produces a filesystem that mostly works.
#[inline]
pub fn block_size_ok(block_size: usize) -> bool {
    (MIN_BLOCK_SIZE..=MAX_BLOCK_SIZE).contains(&block_size) && block_size.is_power_of_two()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash_table_size;

    /// Walk the published structure definitions field by field and check
    /// that every offset above falls where the C layout puts it. This is
    /// the arithmetic a reviewer would otherwise have to do by hand, and
    /// the only defence against a transcription slip in a constant.
    #[test]
    fn classic_directory_fields_match_the_c_layout() {
        // struct UserDirectoryBlock, 512-byte block:
        //   LONG Type, OwnKey, Reserved1..3, Checksum   6 longs -> 24
        //   ULONG HashTable[72]                         288     -> 312
        //   Reserved4, OwnerXID, Protection, Reserved6  16      -> 328
        //   char Comment[92]                            92      -> 420
        //   ULONG Created[3]                            12      -> 432
        //   char DirName[36]                            36      -> 468
        //   LONG Reserved7[7]                           28      -> 496
        //   HashChain, Parent, Reserved8, SecondaryType 16      -> 512
        let bs = 512;
        assert_eq!(OFF_HASH_TABLE, 24);
        assert_eq!(OFF_HASH_TABLE + 4 * hash_table_size(bs) as usize, 312);
        assert_eq!(tail(bs, TL_OWNER), 316);
        assert_eq!(tail(bs, TL_PROTECTION), 320);
        assert_eq!(tail(bs, TL_BYTE_SIZE), 324);
        assert_eq!(tail(bs, TL_COMMENT), 328);
        assert_eq!(tail(bs, TL_DATE), 420);
        assert_eq!(tail(bs, TL_NAME), 432);
        assert_eq!(tail(bs, TL_NEXT_LINK), 472);
        assert_eq!(tail(bs, TL_HASH_CHAIN), 496);
        assert_eq!(tail(bs, TL_PARENT), 500);
        assert_eq!(tail(bs, TL_EXTENSION), 504);
        assert_eq!(tail(bs, TL_SECONDARY_TYPE), 508);
    }

    #[test]
    fn lnfs_directory_fields_match_the_c_layout() {
        // struct LongNameUserDirectoryBlock, 512-byte block:
        //   head + HashTable[72]                        312
        //   Spare2, Owner/GroupID, Protection, Spare3   16      -> 328
        //   char NaC[112]                               112     -> 440
        //   ULONG CommentBlock                          4       -> 444
        //   LONG Spare4[2]                              8       -> 452
        //   ULONG Created[3]                            12      -> 464
        //   LONG Spare5[2]                              8       -> 472
        //   ULONG FirstLink                             4       -> 476
        //   LONG Spare6[5]                              20      -> 496
        //   HashChain, Parent, DirList, SecondaryType   16      -> 512
        let bs = 512;
        assert_eq!(tail(bs, TL_NAC), 328);
        assert_eq!(tail(bs, TL_NAC) + NAC_LEN, 440);
        assert_eq!(tail(bs, TL_COMMENT_BLOCK), 440);
        assert_eq!(tail(bs, TL_DATE_LONG), 452);
        assert_eq!(tail(bs, TL_NEXT_LINK), 472);
        assert_eq!(tail(bs, TL_HASH_CHAIN), 496);
        // The merged field swallows the classic comment, date and name
        // fields whole: -46..=-19 inclusive, 28 longwords.
        assert!(tail(bs, TL_NAC) <= tail(bs, TL_NAME));
        assert!(tail(bs, TL_NAME) < tail(bs, TL_NAC) + NAC_LEN);
        // And a 107-byte name plus its length byte still leaves room for
        // a comment's length byte.
        const _: () = assert!(1 + crate::MAX_NAME_LONG < NAC_LEN);
    }

    #[test]
    fn root_fields_match_the_c_layout() {
        // struct RootBlock: head + HashTable[72] = 312, BitmapFlag,
        // BitmapKeys[25] = 100, BitmapExtend, DirAltered[3], Name[..],
        // then (LNFS) Reserved1, NumBlocksUsed, DiskAltered[3],
        // DiskMade[3], FileSystemType, Reserved2, DirList, SecondaryType.
        let bs = 512;
        assert_eq!(tail(bs, TL_BITMAP_FLAG), 312);
        assert_eq!(tail(bs, TL_BITMAP_PAGES), 316);
        assert_eq!(tail(bs, TL_BITMAP_PAGES) + 4 * BITMAP_PAGES, 416);
        assert_eq!(tail(bs, TL_BITMAP_EXT), 416);
        assert_eq!(tail(bs, TL_ROOT_DIR_ALTERED), 420);
        assert_eq!(tail(bs, TL_ROOT_NAME), 432);
        assert_eq!(tail(bs, TL_ROOT_NUM_BLOCKS_USED), 468);
        assert_eq!(tail(bs, TL_ROOT_DISK_ALTERED), 472);
        assert_eq!(tail(bs, TL_ROOT_DISK_MADE), 484);
        assert_eq!(tail(bs, TL_ROOT_FS_TYPE), 496);
        assert_eq!(tail(bs, TL_SECONDARY_TYPE), 508);
    }

    #[test]
    fn link_and_comment_and_data_fields_match_the_c_layout() {
        // struct affs_tail (Linux fs/affs/amigaffs.h), from block_size-200:
        //   spare1 4, uid+gid 4, protect 4, size 4      -> 16
        //   comment[92]                                 -> 108
        //   change[3]                                   -> 120
        //   name[32]                                    -> 152
        //   spare2 4                                    -> 156
        //   original 4                                  -> 160   (= -11)
        //   link_chain 4                                -> 164   (= -10)
        let bs = 512;
        assert_eq!(tail(bs, TL_REAL_ENTRY), bs - 200 + 156);
        assert_eq!(tail(bs, TL_NEXT_LINK), bs - 200 + 160);
        // ADFlib's bLinkBlock names the same two longwords realEntry
        // (0x1d4 = 468) and nextLink (0x1d8 = 472).
        assert_eq!(tail(bs, TL_REAL_ENTRY), 0x1d4);
        assert_eq!(tail(bs, TL_NEXT_LINK), 0x1d8);

        // struct CommentBlock: Type, OwnKey, HeaderKey, Spare1[2],
        // Checksum, Comment[80] -> the comment starts at 24 and the
        // 80-byte field holds a 79-character BCPL string exactly.
        assert_eq!(OFF_COMMENT_HEADER_KEY, 8);
        assert_eq!(OFF_COMMENT_TEXT, 24);
        assert_eq!(OFF_COMMENT_TEXT + 1 + COMMENT_MAX, 104);

        // struct affs_data_head: ptype, key, sequence, size, next,
        // checksum, then data[] -- six longwords of header.
        assert_eq!(OFF_DATA_HEADER_KEY, 4);
        assert_eq!(OFF_DATA_SEQ, 8);
        assert_eq!(OFF_DATA_SIZE, 12);
        assert_eq!(OFF_DATA_NEXT, 16);
        assert_eq!(OFF_DATA_PAYLOAD, 24);
        assert_eq!(data_payload_size(512, false), 488);
        assert_eq!(data_payload_size(512, true), 512);
        assert_eq!(data_payload_size(4096, false), 4072);

        // struct slink_front: six longwords, then symname to the tail.
        assert_eq!(OFF_SOFTLINK_PATH, 24);
        assert_eq!(softlink_path_capacity(512), 288);
        assert_eq!(softlink_path_capacity(4096), 3872);
    }

    #[test]
    fn the_data_pointer_table_is_filled_from_the_end_backwards() {
        // Data block 1 goes in the last slot, and the high_seq'th data
        // block in the slot high_seq places from the end.
        for bs in [512usize, 1024, 4096] {
            let slots = hash_table_size(bs);
            assert_eq!(
                data_pointer_offset(bs, 1),
                OFF_HASH_TABLE + (slots as usize - 1) * 4
            );
            // The last usable slot is the first one, longword 6.
            assert_eq!(data_pointer_offset(bs, slots), OFF_HASH_TABLE);
            // And it never runs into the tail.
            assert!(data_pointer_offset(bs, 1) + 4 <= tail(bs, TL_BITMAP_FLAG));
        }
        // At 512 bytes: 72 pointers, so 36 KB of FFS data in the header
        // alone before an extension block is needed.
        assert_eq!(
            hash_table_size(512) as usize * data_payload_size(512, true),
            36864
        );
    }

    /// The record layout, checked against a `DOS\5` block dumped out of
    /// an image xdftool built: three records for `abcd`, `ab` and
    /// `xyzzyx`, starting at bytes 24, 54 and 82. Only word-alignment
    /// makes those numbers come out — 24 + 29 is 53, and the next record
    /// begins at 54.
    #[test]
    fn dircache_records_are_word_aligned_after_two_counted_strings() {
        assert_eq!(OFF_DIRCACHE_RECORDS_START, 24);
        assert_eq!(DIRCACHE_RECORD_MIN, 25);
        assert_eq!(DC_NAME_LEN + 1, DIRCACHE_RECORD_FIXED);

        let starts = {
            let mut p = OFF_DIRCACHE_RECORDS_START;
            let mut v = alloc::vec![p];
            for name in [4usize, 2, 6] {
                p += dircache_record_len(name, 0);
                v.push(p);
            }
            v
        };
        assert_eq!(starts, alloc::vec![24, 54, 82, 114]);

        // An odd raw length rounds up; an even one is left alone.
        assert_eq!(dircache_record_len(4, 0), 30); // 29 -> 30
        assert_eq!(dircache_record_len(3, 0), 28); // 28 -> 28
        assert_eq!(dircache_record_len(7, 0), 32);
        assert_eq!(dircache_record_len(3, 12), 40);
    }

    /// Bitmap geometry, checked against images xdftool built: a 1760-block
    /// floppy needs one bitmap page (4064 bits covers it), and a
    /// 122880-block volume needs 31 — 25 in the root and 6 in one
    /// extension block, which is exactly what that image contains.
    #[test]
    fn bitmap_pages_cover_the_volume_and_extensions_chain_at_the_end() {
        assert_eq!(BITMAP_CHECKSUM_INDEX, 0);
        assert_eq!(OFF_BITMAP_BITS, 4);
        assert_eq!(bitmap_bits_per_block(512), 4064);
        assert_eq!(bitmap_bits_per_block(4096), 32_736);

        // A DD floppy: 1758 blocks to cover, one page.
        assert!(bitmap_bits_per_block(512) >= 1760 - 2);
        // 60 MB at 512 bytes: 122878 blocks to cover.
        let need = (122_880u64 - 2).div_ceil_(bitmap_bits_per_block(512));
        assert_eq!(need, 31);
        assert_eq!(need as usize - BITMAP_PAGES, 6);
        assert!(6 <= bitmap_ext_pointers(512));

        // The pointers fill the block but for the chain longword.
        assert_eq!(bitmap_ext_pointers(512), 127);
        assert_eq!(bitmap_ext_next(512), 508);
        assert_eq!(bitmap_ext_pointers(512) * 4, bitmap_ext_next(512));
        assert_eq!(bitmap_ext_pointers(4096) * 4, bitmap_ext_next(4096));
    }

    trait DivCeil {
        fn div_ceil_(self, rhs: u64) -> u64;
    }
    impl DivCeil for u64 {
        fn div_ceil_(self, rhs: u64) -> u64 {
            self / rhs + u64::from(self % rhs != 0)
        }
    }

    #[test]
    fn head_and_tail_meet_exactly_at_every_block_size() {
        let mut bs = MIN_BLOCK_SIZE;
        while bs <= MAX_BLOCK_SIZE {
            assert!(block_size_ok(bs));
            let head_end = OFF_HASH_TABLE + 4 * hash_table_size(bs) as usize;
            assert_eq!(head_end, tail(bs, TL_BITMAP_FLAG), "block size {bs}");
            assert_eq!(head_end, bs - 200);
            bs *= 2;
        }
        assert!(!block_size_ok(256));
        assert!(!block_size_ok(65536));
        assert!(!block_size_ok(1000));
    }
}
