//! File data, link resolution, and the comment that needed its own block.
//!
//! Three jobs that share one shape: follow a pointer off the disk to
//! another block, and refuse everything about it that does not add up.
//! They are here rather than in [`crate::read`] because they are the
//! second layer — everything below already works without them — and
//! because the verification each one does is the interesting part, not
//! the pointer-following.
//!
//! # The data-block chain
//!
//! A file header block's table (longword 6 onward, the same space a
//! directory uses for hashing) holds up to
//! [`hash_table_size`](crate::hash_table_size()) data-block pointers,
//! **filled from the end backwards**: the last slot is data block 1.
//! Longword 2, `high_seq`, says how many slots are used. When the table
//! fills, longword −2 points at a `T_LIST` extension block with the same
//! backward-filled table and its own `high_seq`, chained onward through
//! its own longword −2 until zero.
//!
//! At 512 bytes that is 72 pointers per block: ~36 KB of FFS data, ~35 KB
//! of OFS, before the first extension block appears. Which is why a file
//! test that never crosses one proves nothing about extension blocks.
//!
//! # OFS data blocks verify themselves
//!
//! FFS data blocks are raw payload: `block_size` bytes, no header, no
//! checksum, nothing to check them against except the arithmetic of
//! `byte_size`. OFS data blocks carry six longwords of header — type,
//! the owning file header's block, the sequence number, the payload
//! length, the next data block, a checksum — and every one of those is a
//! cross-check the format is *offering*. Taking it is the whole reason
//! OFS costs 24 bytes a block: a reader that ignores the headers pays the
//! space and declines the benefit.
//!
//! So this module checks all of them, and the mismatches are typed errors
//! rather than warnings. The one field deliberately *not* enforced is
//! `next` (longword 4): it duplicates the header's table, and the table
//! is what the filesystem allocates from, so where they disagree the
//! table is right. It is exposed rather than checked.
//!
//! # Ranged reads
//!
//! [`Volume::read_range`] is the other half of the read surface, and the
//! one a FUSE adapter or a trackdisk-level consumer actually wants: nobody
//! at that layer reads whole files. It touches **only the blocks the range
//! covers** — the first is `offset / payload_size`, which is arithmetic
//! rather than a walk — and verifies the OFS headers of exactly those,
//! because a block's expected sequence number falls out of its index and
//! not out of having walked there from block 1. That is what makes a
//! ranged read O(range) rather than O(file), and it is only true because
//! [`FileChain`] was split out of the streaming read in the first place: the
//! chain is collected once, and is immutable data a concurrent consumer can
//! hold outside its `Volume` lock.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::layout::*;
use crate::read::{Entry, EntryKind, Error, Volume};
use crate::{bcpl_str, be32, hash_table_size, BlockSource};

/// One file's data-block chain, as read out of its header and extension
/// blocks: the block numbers in file order, block 1 first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChain {
    /// The file header block.
    pub header_lba: u64,
    /// The file's length in bytes, from longword −47.
    pub byte_size: u32,
    /// The data blocks, in order. `blocks[0]` is data block 1.
    pub blocks: Vec<u32>,
    /// The extension blocks walked to collect them, in order. Empty for a
    /// file that fits in its header's own table.
    pub extensions: Vec<u32>,
}

impl<S: BlockSource> Volume<S> {
    /// Collect a file's data-block chain, walking every extension block
    /// and verifying each one.
    ///
    /// The consistency check that matters is the last one: the number of
    /// data blocks the chain holds must equal the number `byte_size`
    /// implies, exactly. Too few and a reader silently returns a short
    /// file; too many and it reads blocks the file does not own. Both
    /// happen on real damaged volumes, and neither is something to
    /// paper over — the file's length and its extent are two independent
    /// records of the same fact, and when they disagree there is nothing
    /// to prefer.
    pub fn file_chain(&mut self, header_lba: u64) -> Result<FileChain, Error<S::Error>> {
        let bs = self.block_size;
        self.read_checked(header_lba)?;
        let ty = be32(&self.buf, OFF_TYPE);
        if ty != T_HEADER {
            return Err(Error::NotHeader {
                lba: header_lba,
                found: ty,
            });
        }
        let st = be32(&self.buf, tail(bs, TL_SECONDARY_TYPE)) as i32;
        if st != ST_FILE {
            return Err(Error::WrongSecondaryType {
                lba: header_lba,
                found: st,
                expected: ST_FILE,
            });
        }
        let byte_size = be32(&self.buf, tail(bs, TL_BYTE_SIZE));
        let payload = data_payload_size(bs, self.variant.is_ffs()) as u64;

        let mut blocks = Vec::new();
        let mut extensions = Vec::new();
        collect_data_pointers(&self.buf, header_lba, &mut blocks, byte_size, payload)?;
        let mut next = be32(&self.buf, tail(bs, TL_EXTENSION));

        // The header counts as visited: an extension block pointing back
        // at it is a cycle like any other.
        let mut visited = BTreeSet::from([header_lba]);
        while next != 0 {
            let lba = next as u64;
            self.guard_chain(&mut visited, lba)?;
            self.read_checked(lba)?;
            let ty = be32(&self.buf, OFF_TYPE);
            if ty != T_LIST {
                return Err(Error::WrongBlockType {
                    lba,
                    found: ty,
                    expected: T_LIST,
                });
            }
            // An extension block's own key, parent and secondary type all
            // say where it belongs. A block that agrees with none of them
            // is somebody else's; a block that agrees with all of them
            // could still be corrupt, but not in a way a pointer walk can
            // see.
            let own = be32(&self.buf, OFF_OWN_KEY);
            if own as u64 != lba {
                return Err(Error::OwnKeyMismatch { lba, found: own });
            }
            let st = be32(&self.buf, tail(bs, TL_SECONDARY_TYPE)) as i32;
            if st != ST_FILE {
                return Err(Error::WrongSecondaryType {
                    lba,
                    found: st,
                    expected: ST_FILE,
                });
            }
            let parent = be32(&self.buf, tail(bs, TL_PARENT));
            if parent as u64 != header_lba {
                return Err(Error::BlockOwnerMismatch {
                    lba,
                    found: parent,
                    expected: header_lba as u32,
                });
            }
            collect_data_pointers(&self.buf, lba, &mut blocks, byte_size, payload)?;
            extensions.push(next);
            next = be32(&self.buf, tail(bs, TL_EXTENSION));
        }

        let expected = (byte_size as u64).div_ceil_(payload);
        if blocks.len() as u64 != expected {
            return Err(Error::FileSizeMismatch {
                lba: header_lba,
                byte_size,
                blocks: blocks.len() as u64,
                expected,
            });
        }

        Ok(FileChain {
            header_lba,
            byte_size,
            blocks,
            extensions,
        })
    }

    /// Read a file, handing each block's payload to `chunk` as it comes.
    ///
    /// The incremental form: nothing larger than one block is ever held,
    /// so a 200 MB file costs a 512-byte buffer. Returns the total bytes
    /// delivered, which is [`FileChain::byte_size`] on success — the walk
    /// stops with an error before it can be anything else.
    ///
    /// `chunk` cannot fail. A sink that can (a socket, a file) captures
    /// its own error and stops looking at the bytes; threading a second
    /// error type through here would put the caller's failure inside this
    /// crate's [`Error`], which is a shape the transport error already
    /// occupies for a different reason.
    pub fn read_file_with<F>(&mut self, header_lba: u64, chunk: F) -> Result<u64, Error<S::Error>>
    where
        F: FnMut(&[u8]),
    {
        let chain = self.file_chain(header_lba)?;
        self.read_chain_with(&chain, chunk)
    }

    /// Stream a chain already collected by [`Volume::file_chain`] — so
    /// that reading a whole file walks the extension blocks once, not
    /// once per entry point.
    pub fn read_chain_with<F>(
        &mut self,
        chain: &FileChain,
        mut chunk: F,
    ) -> Result<u64, Error<S::Error>>
    where
        F: FnMut(&[u8]),
    {
        let header_lba = chain.header_lba;
        let bs = self.block_size;
        let ffs = self.variant.is_ffs();
        let payload = data_payload_size(bs, ffs) as u64;
        let mut remaining = chain.byte_size as u64;

        for (i, &block) in chain.blocks.iter().enumerate() {
            let lba = block as u64;
            let seq = i as u32 + 1;
            // Every block is full but the last, whose length falls out of
            // byte_size. This is the *only* record of an FFS file's final
            // block length — there is no per-block size to consult.
            let want = remaining.min(payload) as usize;

            if ffs {
                self.read_raw(lba)?;
                chunk(&self.buf[..want]);
            } else {
                self.read_checked(lba)?;
                self.verify_ofs_data(lba, header_lba, seq, want)?;
                chunk(&self.buf[OFF_DATA_PAYLOAD..OFF_DATA_PAYLOAD + want]);
            }
            remaining -= want as u64;
        }
        debug_assert_eq!(remaining, 0);
        Ok(chain.byte_size as u64)
    }

    /// Read `buf.len()` bytes from `offset` into a file, touching only the
    /// blocks that range covers.
    ///
    /// The read(2) shape, and deliberately so — this is what a FUSE
    /// adapter, a trackdisk transport or anything else block-oriented
    /// actually calls. Returns how many bytes were delivered:
    ///
    /// - **`offset` at or past the file's end returns `Ok(0)`.** Not an
    ///   error: end of file is a length, not a failure, and a caller
    ///   looping until it gets zero is the idiom this has to support.
    ///   (AmigaDOS's own `Seek` refuses to *position* past the end, which
    ///   is a different question — this is a read, and it has no cursor to
    ///   leave anywhere.)
    /// - **A range that runs off the end is clamped**, so the count comes
    ///   back short. `byte_size` is the authority for where the file ends,
    ///   not the blocks: the last block has payload capacity past the last
    ///   byte and returning it would invent data.
    /// - **A zero-length `buf` returns `Ok(0)`** at any offset before the
    ///   end, which falls out of the clamping rather than being special.
    ///
    /// The first block is `offset / payload_size` and every OFS header
    /// from there on is verified exactly as [`Volume::read_chain_with`]
    /// verifies it — the expected sequence number is `index + 1`, which is
    /// arithmetic, so nothing has to be walked from block 1 to know it.
    /// That is the whole point: reading 512 bytes out of the middle of a
    /// 200 MB file costs one block read, and still refuses an OFS block
    /// that belongs to another file or sits in the wrong place.
    pub fn read_range(
        &mut self,
        chain: &FileChain,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, Error<S::Error>> {
        let bs = self.block_size;
        let ffs = self.variant.is_ffs();
        let payload = data_payload_size(bs, ffs) as u64;
        let size = chain.byte_size as u64;
        if offset >= size {
            return Ok(0);
        }
        let want = ((size - offset).min(buf.len() as u64)) as usize;
        let header_lba = chain.header_lba;

        let mut done = 0usize;
        let mut index = (offset / payload) as usize;
        // Only the first block starts part-way in; every one after it is
        // entered at its own byte 0.
        let mut skip = (offset % payload) as usize;
        while done < want {
            let block = match chain.blocks.get(index) {
                Some(&b) => b as u64,
                // Unreachable via `file_chain`, which refuses a chain whose
                // length and `byte_size` disagree — but this takes a chain
                // by reference and a caller can hold a stale one, so it
                // stops rather than indexing off the end.
                None => break,
            };
            // How much of this block the *file* uses: full but for the
            // last, whose length falls out of byte_size — the same rule
            // the streaming read applies, and the OFS size field is
            // checked against it here too.
            let full = ((size - index as u64 * payload).min(payload)) as usize;
            let take = (full - skip).min(want - done);
            if ffs {
                self.read_raw(block)?;
                buf[done..done + take].copy_from_slice(&self.buf[skip..skip + take]);
            } else {
                self.read_checked(block)?;
                self.verify_ofs_data(block, header_lba, index as u32 + 1, full)?;
                let at = OFF_DATA_PAYLOAD + skip;
                buf[done..done + take].copy_from_slice(&self.buf[at..at + take]);
            }
            done += take;
            skip = 0;
            index += 1;
        }
        Ok(done)
    }

    /// Check the four cross-references an OFS data block carries, against
    /// the block already in [`Volume::buf`].
    ///
    /// Shared by the streaming read and the ranged one so that a range
    /// cannot verify *less* than a whole-file read of the same bytes
    /// would: two copies of this would be two chances for one of them to
    /// stop checking the sequence number.
    fn verify_ofs_data(
        &self,
        lba: u64,
        header_lba: u64,
        seq: u32,
        want: usize,
    ) -> Result<(), Error<S::Error>> {
        let ty = be32(&self.buf, OFF_TYPE);
        if ty != T_DATA {
            return Err(Error::WrongBlockType {
                lba,
                found: ty,
                expected: T_DATA,
            });
        }
        let owner = be32(&self.buf, OFF_DATA_HEADER_KEY);
        if owner as u64 != header_lba {
            return Err(Error::BlockOwnerMismatch {
                lba,
                found: owner,
                expected: header_lba as u32,
            });
        }
        let found = be32(&self.buf, OFF_DATA_SEQ);
        if found != seq {
            return Err(Error::DataBlockSequence {
                lba,
                found,
                expected: seq,
            });
        }
        // One comparison covers both traps: a size past the block's
        // capacity, and a short block anywhere but at the end of the file.
        let size = be32(&self.buf, OFF_DATA_SIZE);
        if size as usize != want {
            return Err(Error::DataBlockSize {
                lba,
                found: size,
                expected: want as u32,
            });
        }
        Ok(())
    }

    /// Read a whole file into one `Vec`.
    ///
    /// The convenient form, and the one to think twice about: the file's
    /// length comes off the disk, so this allocates whatever a header
    /// block claims. For anything that might be hostile or merely large,
    /// [`Volume::read_file_with`] streams it a block at a time.
    pub fn read_file(&mut self, header_lba: u64) -> Result<Vec<u8>, Error<S::Error>> {
        // Sized from the chain, not from byte_size alone: file_chain has
        // already refused a length its blocks cannot back, so by here the
        // number is one the volume can actually produce.
        let chain = self.file_chain(header_lba)?;
        let mut out = Vec::with_capacity(chain.byte_size as usize);
        self.read_chain_with(&chain, |bytes| out.extend_from_slice(bytes))?;
        Ok(out)
    }

    /// Follow a hard link to the object it really names.
    ///
    /// Hard links (`ST_LINKFILE`, `ST_LINKDIR`) are header blocks with
    /// their own name, dates and protection and no content of their own:
    /// longword −11 points at the canonical header, and longword −10
    /// chains this link to the next link naming the same object. This
    /// follows the first; the second is [`Entry::next_link`], exposed for
    /// a caller that wants every name an object has.
    ///
    /// A link to a link is legal and followed, with the usual refusal:
    /// a chain that revisits a block is [`Error::ChainCycle`], not a
    /// hang. Files and directories pass through unchanged, so this
    /// composes — `resolve_link` is safe to call on anything a listing
    /// produced. Soft links are the exception and are refused with
    /// [`Error::SoftLinkNotResolved`]: they hold a *path*, and see
    /// [`Volume::read_softlink`] for why resolving one is not this
    /// crate's job.
    pub fn resolve_link(&mut self, entry: &Entry) -> Result<Entry, Error<S::Error>> {
        let mut here = entry.clone();
        let mut visited = BTreeSet::from([here.lba]);
        loop {
            match here.kind {
                EntryKind::File | EntryKind::Directory => return Ok(here),
                EntryKind::SoftLink => {
                    return Err(Error::SoftLinkNotResolved { lba: here.lba });
                }
                EntryKind::LinkFile | EntryKind::LinkDir => {
                    if here.real_entry == 0 {
                        return Err(Error::LinkTargetMissing { lba: here.lba });
                    }
                    let target = here.real_entry as u64;
                    self.guard_chain(&mut visited, target)?;
                    here = self.entry_at(target)?;
                }
            }
        }
    }

    /// The path a soft link stores, as raw bytes.
    ///
    /// An `ST_SOFTLINK` block holds a NUL-terminated path string where a
    /// directory holds its hash table — longword 6 to the start of the
    /// tail. The path may be absolute (`Work:Tools/foo`), relative, or
    /// name a volume that is not mounted; it is a string AmigaDOS feeds
    /// back through path resolution, not a pointer.
    ///
    /// # Why `lookup_path` does not follow these
    ///
    /// Resolving a soft link means re-entering path resolution from an
    /// arbitrary point — possibly on a *different volume*, via an assign
    /// or a device name this crate has never heard of. That is
    /// `DosPacket` territory (`ERROR_IS_SOFT_LINK` and the caller's
    /// retry), and PLAN.md puts the handler layer outside this crate on
    /// purpose. So [`Volume::lookup_path`] returns the soft link entry
    /// itself and the consumer — which is the only thing that knows what
    /// `Work:` means — decides what to do with the path.
    pub fn read_softlink(&mut self, lba: u64) -> Result<Vec<u8>, Error<S::Error>> {
        let bs = self.block_size;
        self.read_checked(lba)?;
        let ty = be32(&self.buf, OFF_TYPE);
        if ty != T_HEADER {
            return Err(Error::NotHeader { lba, found: ty });
        }
        let st = be32(&self.buf, tail(bs, TL_SECONDARY_TYPE)) as i32;
        if st != ST_SOFTLINK {
            return Err(Error::WrongSecondaryType {
                lba,
                found: st,
                expected: ST_SOFTLINK,
            });
        }
        let start = OFF_SOFTLINK_PATH;
        let end = start + softlink_path_capacity(bs);
        let field = &self.buf[start..end];
        let len = field.iter().position(|&c| c == 0).unwrap_or(field.len());
        Ok(field[..len].to_vec())
    }

    /// An entry's comment, from wherever it actually lives.
    ///
    /// On classic variants, and on LNFS entries whose name left room, the
    /// comment is in the header block and [`Entry::comment`] already has
    /// it. On an LNFS entry whose name and comment did not both fit in
    /// the 112-byte merged field, [`Entry::comment`] is *empty* and the
    /// comment is in a `T_COMMENT` block named by longword −18 — which is
    /// the trap: an empty inline comment does not mean no comment. This
    /// resolves the two cases into one answer.
    ///
    /// The overflow block is verified as belonging to this entry: its
    /// longword 2 is the header block it was written for, and a comment
    /// block that names a different one is not this entry's comment
    /// whatever the pointer says.
    pub fn comment(&mut self, entry: &Entry) -> Result<Vec<u8>, Error<S::Error>> {
        if entry.comment_block == 0 {
            return Ok(entry.comment.clone());
        }
        let lba = entry.comment_block as u64;
        self.read_checked(lba)?;
        let ty = be32(&self.buf, OFF_TYPE);
        if ty != T_COMMENT {
            return Err(Error::WrongBlockType {
                lba,
                found: ty,
                expected: T_COMMENT,
            });
        }
        let own = be32(&self.buf, OFF_OWN_KEY);
        if own as u64 != lba {
            return Err(Error::OwnKeyMismatch { lba, found: own });
        }
        let owner = be32(&self.buf, OFF_COMMENT_HEADER_KEY);
        if owner as u64 != entry.lba {
            return Err(Error::BlockOwnerMismatch {
                lba,
                found: owner,
                expected: entry.lba as u32,
            });
        }
        Ok(bcpl_str(&self.buf, OFF_COMMENT_TEXT, COMMENT_MAX).to_vec())
    }
}

/// Read one block's data-pointer table into `out`, in file order.
///
/// `high_seq` (longword 2) says how many of the table's slots are used,
/// and they are the *last* `high_seq` of them: slot `n-1` holds the first
/// data block this table contributes, counting down. Both bounds are
/// checked — a `high_seq` past the table's end would read into the tail,
/// and a zero slot inside the claimed range is a hole the format cannot
/// express.
fn collect_data_pointers<E>(
    block: &[u8],
    lba: u64,
    out: &mut Vec<u32>,
    byte_size: u32,
    payload: u64,
) -> Result<(), Error<E>> {
    let bs = block.len();
    let slots = hash_table_size(bs);
    let high_seq = be32(block, OFF_HIGH_SEQ);
    if high_seq > slots {
        return Err(Error::DataPointerCount {
            lba,
            high_seq,
            max: slots,
        });
    }
    let maximum = (byte_size as u64).div_ceil_(payload);
    let claimed = out.len() as u64 + u64::from(high_seq);
    if claimed > maximum {
        return Err(Error::FileSizeMismatch {
            lba,
            byte_size,
            blocks: claimed,
            expected: maximum,
        });
    }
    out.reserve(high_seq as usize);
    for seq in 1..=high_seq {
        let ptr = be32(block, data_pointer_offset(bs, seq));
        if ptr == 0 {
            return Err(Error::DataPointerHole { lba, seq });
        }
        out.push(ptr);
    }
    Ok(())
}

/// `u64::div_ceil` in a 1.63-compatible form (it stabilised in 1.73).
trait DivCeil {
    fn div_ceil_(self, rhs: u64) -> u64;
}

impl DivCeil for u64 {
    fn div_ceil_(self, rhs: u64) -> u64 {
        self / rhs + u64::from(self % rhs != 0)
    }
}
