//! Block assembly: the bytes of an entry, with no medium in sight.
//!
//! Two writers in this crate lay down the same blocks for two different
//! reasons. [`Populator`](crate::Populator) fills a volume it just
//! formatted, allocating with a cursor over a known-free extent;
//! [`Mutator`](crate::Mutator) changes a volume somebody else wrote,
//! allocating from its bitmap under the mark-then-use ordering. What they
//! do *differently* is which block goes where and in what order; what they
//! must do **identically** is what a header block, a comment block, a data
//! block and a dircache record contain — and a second copy of that would
//! be a second chance to put the LNFS comment at the classic offset, which
//! is the one mistake this crate exists to not make.
//!
//! So the assembly lives here, as pure functions over a `&mut [u8]` that
//! is already the block's size. Nothing in this module reads or writes a
//! medium, allocates a block, or decides what is free: every function
//! takes the numbers it needs and fills in bytes. That is also what makes
//! it testable without a disk — the layout tests in [`crate::layout`] check
//! the offsets, and these functions are the only place those offsets are
//! turned into a block.

use alloc::vec;
use alloc::vec::Vec;

use crate::format::{wr32, wr_bcpl, wr_date};
use crate::layout::*;
use crate::populate::Metadata;
use crate::read::{DateStamp, EntryKind};
use crate::{checksum_compute, Variant, MAX_NAME_CLASSIC};

/// Fix a metadata block's checksum at longword 5, the last thing done to
/// every block but a bitmap page.
#[inline]
pub(crate) fn finish_checksum(block: &mut [u8]) {
    let ck = checksum_compute(block, CHECKSUM_INDEX);
    wr32(block, OFF_CHECKSUM, ck);
}

/// Write a big-endian u16 — the dircache record's own width, and nowhere
/// else in the format.
#[inline]
pub(crate) fn wr16(block: &mut [u8], off: usize, v: u16) {
    block[off..off + 2].copy_from_slice(&v.to_be_bytes());
}

// ---------------------------------------------------------------------------
// Header blocks
// ---------------------------------------------------------------------------

/// Everything a header block records about the entry it names, minus the
/// table in the middle — which the caller has already filled in (a
/// directory's hash slots, a file's data pointers, a dircache pointer at
/// longword −2) and which this must not disturb.
pub(crate) struct EntryFields<'a> {
    /// The block the header lives in, which it also records as its own
    /// key.
    pub lba: u64,
    /// The directory holding it.
    pub parent: u64,
    /// What kind of entry this is.
    pub kind: EntryKind,
    /// The file's length; ignored for anything that is not a file.
    pub byte_size: u32,
    /// The next entry in this hash chain — the slot's *previous* head, on
    /// the head insertion both writers do.
    pub hash_chain: u32,
    /// The name, raw Latin-1, already checked.
    pub name: &'a [u8],
    /// Protection, comment, date and owner.
    pub meta: &'a Metadata<'a>,
    /// An LNFS overflow comment block, or 0 when the comment is inline.
    pub comment_block: u32,
}

/// Assemble a whole header block, checksum included.
///
/// The buffer is expected to arrive with the head's table already as the
/// caller wants it and everything else zero: this fills the head's first
/// six longwords and the tail, and touches nothing between them.
pub(crate) fn write_entry_header(hdr: &mut [u8], variant: Variant, f: &EntryFields<'_>) {
    let bs = hdr.len();
    wr32(hdr, OFF_TYPE, T_HEADER);
    wr32(hdr, OFF_OWN_KEY, f.lba as u32);
    wr32(hdr, tail(bs, TL_OWNER), f.meta.owner);
    wr32(hdr, tail(bs, TL_PROTECTION), f.meta.protection);
    // Longword −47 is the byte size only in a file header; in a directory
    // it is a spare longword, and writing a size there would make the
    // reader's own refusal to report it look like a bug.
    if matches!(f.kind, EntryKind::File | EntryKind::LinkFile) {
        wr32(hdr, tail(bs, TL_BYTE_SIZE), f.byte_size);
    }
    write_name_and_comment(hdr, variant, f.name, f.meta.comment, f.comment_block);
    write_date(hdr, variant, f.meta.date);
    wr32(hdr, tail(bs, TL_HASH_CHAIN), f.hash_chain);
    wr32(hdr, tail(bs, TL_PARENT), f.parent as u32);
    wr32(
        hdr,
        tail(bs, TL_SECONDARY_TYPE),
        f.kind.secondary_type() as u32,
    );
    finish_checksum(hdr);
}

/// Write the name and comment into a header block, in whichever of the
/// two layouts the variant uses.
///
/// Both fields are **cleared first**, which matters for the in-place
/// rewrite a rename does and not at all for a freshly zeroed block: on
/// LNFS the comment's position follows the name's length, so a shorter
/// new name would otherwise leave the old comment's tail sitting where
/// the new one's length byte now points.
///
/// `comment_block` non-zero means the comment lives in its own
/// [`T_COMMENT`] block: the inline comment is written **empty** and
/// longword −18 names the block. That is the state
/// [`Volume::comment`](crate::Volume::comment) exists to resolve, and the
/// one a reader must not mistake for "no comment".
pub(crate) fn write_name_and_comment(
    hdr: &mut [u8],
    variant: Variant,
    name: &[u8],
    comment: &[u8],
    comment_block: u32,
) {
    let bs = hdr.len();
    if variant.has_long_names() {
        let off = tail(bs, TL_NAC);
        for b in hdr[off..off + NAC_LEN].iter_mut() {
            *b = 0;
        }
        wr_bcpl(hdr, off, name);
        let after = off + 1 + name.len();
        if comment_block == 0 {
            wr_bcpl(hdr, after, comment);
        } else {
            hdr[after] = 0;
        }
        wr32(hdr, tail(bs, TL_COMMENT_BLOCK), comment_block);
    } else {
        let n = tail(bs, TL_NAME);
        for b in hdr[n..n + 1 + MAX_NAME_CLASSIC].iter_mut() {
            *b = 0;
        }
        let c = tail(bs, TL_COMMENT);
        for b in hdr[c..c + 1 + COMMENT_MAX].iter_mut() {
            *b = 0;
        }
        wr_bcpl(hdr, n, name);
        wr_bcpl(hdr, c, comment);
    }
}

/// Write an entry's DateStamp where its variant keeps it: longword −23 on
/// the classic layout, −15 on LNFS, where the merged name-and-comment
/// field pushed it down.
pub(crate) fn write_date(hdr: &mut [u8], variant: Variant, date: DateStamp) {
    let bs = hdr.len();
    if variant.has_long_names() {
        wr_date(hdr, tail(bs, TL_DATE_LONG), date);
    } else {
        wr_date(hdr, tail(bs, TL_DATE), date);
    }
}

/// Does this name-and-comment pair need a [`T_COMMENT`] block?
///
/// Only ever on LNFS, and only when the two do not both fit in the
/// 112-byte merged field: a length byte and a name, then a length byte
/// and a comment. On the classic layout the two have separate fields and
/// neither can crowd the other out — and on LNFS a *comment-less* entry
/// never needs one either, since the longest name the variant allows
/// still leaves the comment's length byte inside the field.
#[inline]
pub(crate) fn needs_comment_block(variant: Variant, name_len: usize, comment_len: usize) -> bool {
    variant.has_long_names() && 1 + name_len + 1 + comment_len > NAC_LEN
}

// ---------------------------------------------------------------------------
// Comment, data and dircache blocks
// ---------------------------------------------------------------------------

/// Assemble a [`T_COMMENT`] overflow-comment block.
///
/// Longword 2 names the header block this comment belongs to, which is
/// what [`Volume::comment`](crate::Volume::comment) checks before it
/// believes the text — a comment block that names a different entry is
/// not this entry's comment, whatever pointed at it.
pub(crate) fn build_comment_block(buf: &mut [u8], lba: u64, header: u64, comment: &[u8]) {
    for b in buf.iter_mut() {
        *b = 0;
    }
    wr32(buf, OFF_TYPE, T_COMMENT);
    wr32(buf, OFF_OWN_KEY, lba as u32);
    wr32(buf, OFF_COMMENT_HEADER_KEY, header as u32);
    wr_bcpl(buf, OFF_COMMENT_TEXT, comment);
    finish_checksum(buf);
}

/// Assemble one `T_LIST` extension block: type, own key, high_seq, parent,
/// `ST_FILE` secondary type, the data-pointer table and the chain pointer
/// to the next extension block (0 for the last one).
///
/// `pointers` is this block's own slice of the file's data-pointer table,
/// already in order; `pointers.len()` becomes `high_seq`. Two writers
/// assemble that slice differently — [`Mutator`](crate::Mutator) and
/// [`crate::compact`] already know a file's whole block count and hand
/// over a complete slice in one call, while [`Populator`](crate::Populator)
/// discovers data blocks one at a time from a streaming callback and
/// accumulates them into a block's worth of pointers before calling this —
/// but the six fixed longwords and the table they surround are one
/// implementation either way.
pub(crate) fn build_extension_block(
    buf: &mut [u8],
    lba: u64,
    header: u64,
    pointers: &[u32],
    next: u32,
) {
    let bs = buf.len();
    for b in buf.iter_mut() {
        *b = 0;
    }
    wr32(buf, OFF_TYPE, T_LIST);
    wr32(buf, OFF_OWN_KEY, lba as u32);
    wr32(buf, OFF_HIGH_SEQ, pointers.len() as u32);
    wr32(buf, tail(bs, TL_PARENT), header as u32);
    wr32(buf, tail(bs, TL_SECONDARY_TYPE), ST_FILE as u32);
    wr32(buf, tail(bs, TL_EXTENSION), next);
    for (i, &p) in pointers.iter().enumerate() {
        wr32(buf, data_pointer_offset(bs, i as u32 + 1), p);
    }
    finish_checksum(buf);
}

/// Assemble one data block: raw payload on FFS, six longwords of header
/// and a checksum on OFS.
///
/// The FFS case deliberately leaves no checksum. An FFS data block is
/// `block_size` bytes of the file and nothing else, so a writer that
/// "fixed the checksum" would be writing four bytes of its own over the
/// caller's data.
pub(crate) fn build_data_block(
    buf: &mut [u8],
    header: u64,
    seq: u32,
    data: &[u8],
    next: u32,
    ffs: bool,
) {
    for b in buf.iter_mut() {
        *b = 0;
    }
    if ffs {
        buf[..data.len()].copy_from_slice(data);
        return;
    }
    wr32(buf, OFF_TYPE, T_DATA);
    wr32(buf, OFF_DATA_HEADER_KEY, header as u32);
    wr32(buf, OFF_DATA_SEQ, seq);
    wr32(buf, OFF_DATA_SIZE, data.len() as u32);
    wr32(buf, OFF_DATA_NEXT, next);
    buf[OFF_DATA_PAYLOAD..OFF_DATA_PAYLOAD + data.len()].copy_from_slice(data);
    finish_checksum(buf);
}

/// Assemble a `T_DIRCACHE` block holding `count` records already packed
/// into `records`, chained to `next`.
pub(crate) fn build_dircache_block(
    buf: &mut [u8],
    lba: u64,
    dir: u64,
    records: &[u8],
    count: u32,
    next: u64,
) {
    for b in buf.iter_mut() {
        *b = 0;
    }
    wr32(buf, OFF_TYPE, T_DIRCACHE);
    wr32(buf, OFF_OWN_KEY, lba as u32);
    wr32(buf, OFF_DIRCACHE_PARENT, dir as u32);
    wr32(buf, OFF_DIRCACHE_RECORDS, count);
    wr32(buf, OFF_DIRCACHE_NEXT, next as u32);
    let start = OFF_DIRCACHE_RECORDS_START;
    buf[start..start + records.len()].copy_from_slice(records);
    finish_checksum(buf);
}

/// One directory entry as a dircache remembers it.
pub(crate) struct CacheFacts<'a> {
    /// The entry's header block.
    pub entry: u64,
    /// The file's length, 0 for anything else.
    pub byte_size: u32,
    /// The protection longword.
    pub protection: u32,
    /// The owner longword, UID over GID.
    pub owner: u32,
    /// The entry's DateStamp, narrowed to words on the way in.
    pub date: DateStamp,
    /// What kind of entry it is.
    pub kind: EntryKind,
    /// The name, raw Latin-1.
    pub name: &'a [u8],
    /// The comment as it really is — resolved from an overflow block
    /// where there is one, since a cache records the comment's *text* and
    /// has nowhere to put a pointer.
    pub comment: &'a [u8],
}

/// Pack one dircache record.
///
/// The `DateStamp` is narrowed to three *words* — the format's own
/// narrowing, not this crate's — so a cached date runs out in 2157 where
/// the header block's runs out in 11.7 million. Truncating rather than
/// clamping is what AmigaDOS does, and the cache is advisory anyway:
/// [`Volume::validate`](crate::Volume::validate) never compares dates,
/// and nothing in this crate resolves anything through a cache.
pub(crate) fn dircache_record(f: &CacheFacts<'_>) -> Vec<u8> {
    let name = &f.name[..f.name.len().min(u8::MAX as usize)];
    let comment = &f.comment[..f.comment.len().min(COMMENT_MAX)];
    let mut r = vec![0u8; dircache_record_len(name.len(), comment.len())];
    wr32(&mut r, DC_ENTRY, f.entry as u32);
    wr32(&mut r, DC_SIZE, f.byte_size);
    wr32(&mut r, DC_PROTECTION, f.protection);
    wr16(&mut r, DC_UID, (f.owner >> 16) as u16);
    wr16(&mut r, DC_GID, f.owner as u16);
    wr16(&mut r, DC_DAYS, f.date.days as u16);
    wr16(&mut r, DC_MINS, f.date.mins as u16);
    wr16(&mut r, DC_TICKS, f.date.ticks as u16);
    // xdftool leaves this byte zero on every record it writes; filling it
    // in is strictly more information, and a validator reads a zero as
    // "not recorded" rather than as a disagreement either way.
    r[DC_TYPE] = f.kind.secondary_type() as i8 as u8;
    r[DC_NAME_LEN] = name.len() as u8;
    let n = DIRCACHE_RECORD_FIXED;
    r[n..n + name.len()].copy_from_slice(name);
    r[n + name.len()] = comment.len() as u8;
    let c = n + name.len() + 1;
    r[c..c + comment.len()].copy_from_slice(comment);
    r
}

/// Where the records in a dircache block end: walk `count` of them,
/// because their lengths depend on the two counted strings inside them.
pub(crate) fn dircache_used(block: &[u8], count: u32) -> usize {
    let mut off = OFF_DIRCACHE_RECORDS_START;
    for _ in 0..count {
        if off + DIRCACHE_RECORD_MIN > block.len() {
            return block.len();
        }
        let name_len = block[off + DC_NAME_LEN] as usize;
        if off + DIRCACHE_RECORD_FIXED + name_len >= block.len() {
            return block.len();
        }
        let comment_len = block[off + DIRCACHE_RECORD_FIXED + name_len] as usize;
        off += dircache_record_len(name_len, comment_len);
    }
    off
}
