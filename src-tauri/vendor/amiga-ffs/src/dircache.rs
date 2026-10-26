//! Directory cache blocks (`DOS\4`/`DOS\5`): read, reported, **not
//! believed**.
//!
//! A dircache block is a denormalised copy of a directory's listing —
//! every entry's block, size, protection, owner, date, type, name and
//! comment, packed end to end — so that `List` can print a directory
//! from one or two block reads instead of one read per entry. On a
//! floppy that is the difference between a directory listing taking a
//! second and taking twenty.
//!
//! It is also a *cache*, with everything that implies. The hash chains
//! remain the filesystem's actual directory; the cache is a second
//! record of the same fact, written by whatever last modified the
//! directory. A filesystem that crashed between updating the chains and
//! updating the cache leaves them disagreeing, and every mutation done by
//! a tool that does not know about `DOS\4` (there are several) leaves
//! them disagreeing permanently.
//!
//! So this module reads the cache and hands it back marked advisory.
//! Nothing in this crate *resolves* a name through it, and
//! [`Volume::validate`](crate::validate) compares it against the chains
//! and reports the differences as findings. A reader that trusts a stale
//! cache invents a directory that isn't there — files that were deleted
//! still listed, files that were created still invisible — and does it
//! silently, which is worse than being slow.
//!
//! # Where the pointer lives
//!
//! Longword −2, [`TL_EXTENSION`] — the same field a *file* header uses
//! for its first `T_LIST` extension block. They cannot collide: the
//! field's meaning follows the block's secondary type, and a directory
//! has no data chain while a file has no directory to cache. The root's
//! copy is [`RootBlock::dircache`](crate::read::RootBlock::dircache),
//! read from the same longword.
//!
//! # The record layout
//!
//! Transcribed from ADFlib's `bDirCacheBlock`/dircache-entry structures
//! and confirmed byte for byte against `DOS\5` images built by xdftool:
//! six longwords of head (type 33, own key, the parent directory, the
//! record count, the next cache block, the checksum at the usual
//! longword 5), then packed records from byte 24. Each record is
//!
//! ```text
//!   0  entry block   long     8  protection  long    16  days   word
//!   4  size          long    12  UID         word    18  mins   word
//!                            14  GID         word    20  ticks  word
//!  22  secondary type  signed byte
//!  23  name length     byte      24            name
//!  24+name_len  comment length byte, then the comment
//! ```
//!
//! …rounded **up to an even length**, because the fields inside are
//! words. Two facts here are easy to get wrong and both are checked in
//! [`crate::layout`]'s tests: the `DateStamp` is three *words* rather than
//! three longwords (so a dircache date runs out in 2157 where a header
//! block's runs out in 11.7 million), and the padding is what makes the
//! record offsets 24, 54, 82 come out of names of 4, 2 and 6 characters.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::layout::*;
use crate::read::{DateStamp, Error, Volume};
use crate::{be16, be32, BlockSource};

/// One dircache record: a directory entry as the cache remembers it.
///
/// Every field here has an authoritative twin in the entry's own header
/// block, and the two are free to disagree. Nothing in this struct is a
/// fact about the volume; it is a fact about what the cache *says*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DircacheRecord {
    /// The dircache block this record was read from, and its byte offset
    /// within it — so a finding can name the record, not just the block.
    pub block: u64,
    /// Byte offset of the record inside `block`.
    pub offset: usize,
    /// The entry's header block, as cached.
    pub entry: u32,
    /// The file's length in bytes, as cached. 0 for a directory.
    pub size: u32,
    /// The protection longword, as cached.
    pub protection: u32,
    /// The owner UID, as cached — a *word* here, where the header block
    /// holds UID and GID as the two halves of one longword.
    pub uid: u16,
    /// The owner GID, as cached.
    pub gid: u16,
    /// The date, as cached, widened back out to a [`DateStamp`]. The
    /// narrowing is the cache's, not this crate's: all three fields are
    /// stored as words.
    pub date: DateStamp,
    /// The entry's secondary type narrowed to a signed byte — `2`
    /// ([`ST_USERDIR`]), `-3` ([`ST_FILE`]) and so on.
    ///
    /// Zero is not a valid secondary type and is what some writers leave
    /// here (amitools' xdftool, for one, writes the whole record and
    /// never fills this byte in). [`Volume::validate`](crate::validate)
    /// therefore treats a zero as *not recorded* rather than as a
    /// mismatch — reporting every entry of every xdftool-made volume as
    /// corrupt would make the validator useless on the images most people
    /// have.
    pub entry_type: i8,
    /// The name, raw Latin-1, as cached.
    pub name: Vec<u8>,
    /// The comment, raw Latin-1, as cached.
    pub comment: Vec<u8>,
}

impl DircacheRecord {
    /// The owner UID and GID packed the way a header block stores them,
    /// for comparing a record against the entry it describes.
    pub fn owner(&self) -> u32 {
        (self.uid as u32) << 16 | self.gid as u32
    }
}

/// A directory's whole dircache: the chain of blocks, and the records
/// they held, in the order the cache stores them.
///
/// Both halves matter. The records are what a listing would use; the
/// block numbers are what a validator needs, because a dircache block is
/// as much a part of the volume's allocated extent as any header block
/// and an unreported one shows up as an orphan.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Dircache {
    /// The directory these blocks cache.
    pub dir_lba: u64,
    /// The cache blocks, in chain order. Empty when the directory has no
    /// cache — which is every directory on a variant without them, and a
    /// freshly-made empty directory on one with them.
    pub blocks: Vec<u64>,
    /// Every record from every block in the chain, in order.
    pub records: Vec<DircacheRecord>,
}

impl<S: BlockSource> Volume<S> {
    /// The dircache block a directory's longword −2 points at, or 0.
    ///
    /// Reads the root's parsed copy without a block read; anything else
    /// costs one. Returns 0 — meaning "no cache" — on a variant that has
    /// no dircaches at all, rather than reporting whatever that longword
    /// happens to hold, because on `DOS\0`–`DOS\3` and `DOS\6`/`DOS\7` it
    /// is not a dircache pointer and following it would be a guess.
    pub fn dircache_head(&mut self, dir_lba: u64) -> Result<u32, Error<S::Error>> {
        if !self.variant().has_dircache() {
            return Ok(0);
        }
        if dir_lba == self.root_lba() {
            return Ok(self.root().dircache);
        }
        let entry = self.entry_at(dir_lba)?;
        if !entry.kind.is_directory() {
            return Err(Error::NotADirectory {
                lba: dir_lba,
                found: entry.kind.secondary_type(),
            });
        }
        Ok(entry.extension)
    }

    /// Read a directory's dircache chain.
    ///
    /// **Advisory.** What comes back is what the cache claims, verified
    /// only for *structural* soundness — every block is `T_DIRCACHE`,
    /// checksums, names itself, names this directory as its parent, and
    /// no record runs past the end of its block. Whether the records
    /// describe the directory that is actually there is a different
    /// question, and the one [`Volume::validate`](crate::validate)
    /// answers.
    ///
    /// An empty result is the normal state for a variant without
    /// dircaches; this returns it rather than erroring, so a caller can
    /// ask any volume without first asking what variant it is.
    pub fn read_dircache(&mut self, dir_lba: u64) -> Result<Dircache, Error<S::Error>> {
        let mut out = Dircache {
            dir_lba,
            ..Default::default()
        };
        let mut next = self.dircache_head(dir_lba)?;
        let mut visited: BTreeSet<u64> = BTreeSet::new();
        while next != 0 {
            let lba = next as u64;
            self.guard_chain(&mut visited, lba)?;
            self.read_checked(lba)?;

            let ty = be32(&self.buf, OFF_TYPE);
            if ty != T_DIRCACHE {
                return Err(Error::WrongBlockType {
                    lba,
                    found: ty,
                    expected: T_DIRCACHE,
                });
            }
            let own = be32(&self.buf, OFF_OWN_KEY);
            if own as u64 != lba {
                return Err(Error::OwnKeyMismatch { lba, found: own });
            }
            // A cache block that names a different directory is not this
            // directory's cache, whatever pointed at it.
            let parent = be32(&self.buf, OFF_DIRCACHE_PARENT);
            if parent as u64 != dir_lba {
                return Err(Error::BlockOwnerMismatch {
                    lba,
                    found: parent,
                    expected: dir_lba as u32,
                });
            }
            next = be32(&self.buf, OFF_DIRCACHE_NEXT);
            parse_records(&self.buf, lba, &mut out.records)?;
            out.blocks.push(lba);
        }
        Ok(out)
    }
}

/// Unpack one block's records, refusing every way the count and the two
/// length bytes can point past the block's end.
///
/// The count is not trusted to be right: a record is only read when the
/// bytes for it are demonstrably inside the block, and a count that
/// promises more than fits is [`Error::DircacheRecordOverflow`] rather
/// than a short read of whatever follows in memory.
fn parse_records<E>(block: &[u8], lba: u64, out: &mut Vec<DircacheRecord>) -> Result<(), Error<E>> {
    let count = be32(block, OFF_DIRCACHE_RECORDS);
    let end = block.len();
    let mut off = OFF_DIRCACHE_RECORDS_START;
    for index in 0..count {
        let overflow = || Error::DircacheRecordOverflow { lba, index, off };
        if off + DIRCACHE_RECORD_MIN > end {
            return Err(overflow());
        }
        let name_len = block[off + DC_NAME_LEN] as usize;
        if off + DIRCACHE_RECORD_FIXED + name_len + 1 > end {
            return Err(overflow());
        }
        let comment_len = block[off + DIRCACHE_RECORD_FIXED + name_len] as usize;
        let len = dircache_record_len(name_len, comment_len);
        // The padded length, not the raw one: a record whose last byte
        // is the block's last byte is fine, but the next record's start
        // must still land inside.
        if off + DIRCACHE_RECORD_MIN + name_len + comment_len > end {
            return Err(overflow());
        }
        let name = off + DIRCACHE_RECORD_FIXED;
        let comment = name + name_len + 1;
        out.push(DircacheRecord {
            block: lba,
            offset: off,
            entry: be32(block, off + DC_ENTRY),
            size: be32(block, off + DC_SIZE),
            protection: be32(block, off + DC_PROTECTION),
            uid: be16(block, off + DC_UID),
            gid: be16(block, off + DC_GID),
            date: DateStamp {
                days: be16(block, off + DC_DAYS) as u32,
                mins: be16(block, off + DC_MINS) as u32,
                ticks: be16(block, off + DC_TICKS) as u32,
            },
            entry_type: block[off + DC_TYPE] as i8,
            name: block[name..name + name_len].to_vec(),
            comment: block[comment..comment + comment_len].to_vec(),
        });
        off += len;
    }
    Ok(())
}
