//! `repair()`: the write-side half of [`validate`](crate::validate).
//!
//! What the ROM's disk-validator does, and what has to happen before a
//! volume whose `bitmap_flag` is 0 can be allocated from again: walk the
//! tree, work out what is really in use, write a fresh bitmap describing
//! it, and stamp the flag valid last.
//!
//! # One direction, always
//!
//! The invariant this module is built around, and the one its tests
//! assert: **a repair only ever adds allocation, and only ever removes
//! reachability.** Those are the two directions
//! [`validate`](crate::validate) already distinguishes, and taking each
//! only one way means a repaired volume can be worse than a healthy one
//! but never worse than the damaged one it started as.
//!
//! - A block the walk reaches that the bitmap called free
//!   ([`Finding::ReachableButFree`](crate::Finding::ReachableButFree)) is
//!   marked **allocated**. That is the dangerous finding, and this is the
//!   whole point of the operation.
//! - A block the bitmap calls allocated that the walk did *not* reach
//!   ([`Finding::OrphanBlock`](crate::Finding::OrphanBlock)) stays
//!   allocated. The new bitmap is the **union** of the walk and the old
//!   bits, not the walk alone.
//!
//! That second decision is the one worth defending, because the ROM
//! validator frees orphans and this does not. Freeing a block the walk
//! failed to reach is only safe if the walk was complete — and a walk
//! over a damaged volume is precisely the case where it is not: one
//! unreadable directory block hides its entire subtree, and every file in
//! it looks exactly like a leak. Handing those blocks back is how a
//! recoverable volume becomes an unrecoverable one on the next write. So
//! orphans are reported ([`Action::LeakKept`]) and kept; reclaiming them
//! is a separate operation for a volume that validates clean apart from
//! the leaks, and it is not this one.
//!
//! The same reasoning is why an invalid bitmap is still *read*. Its bits
//! cannot be believed when they say "free" — that is what the flag means
//! — but a bit saying "allocated" is either true or a leak, and taking it
//! at its word is the conservative direction in both cases.
//!
//! # Severing, and what counts as proof
//!
//! Rebuilding the bitmap does not fix a corrupt hash chain: the entries
//! past the corruption are unreachable, so their blocks are (correctly)
//! kept as leaks and the directory keeps producing the same finding
//! forever. [`RepairOptions::sever`] opts in to cutting the damage out —
//! truncating a chain at its last good link, zeroing a comment pointer
//! that leads nowhere — and it is **off by default** because severing
//! throws away the only record of where those entries were. A recovery
//! tool wants to see the dangling pointer; a filesystem that has to mount
//! wants it gone.
//!
//! What may be severed is deliberately narrow: only what the ordinary
//! reader has *proved* it cannot follow. A header block that will not
//! parse, and a chain that revisits a block, are proof; an entry that
//! parses and merely disagrees with its parent is not, and is left alone
//! for a human. Nothing is severed that would drop a readable entry.
//!
//! [`Error::Io`] is not proof of anything about
//! a block's own bytes — only that this one attempt to read it failed. A
//! flaky sector or a transient bus error looks, from `.ok()`, identical
//! to a checksum that is actually wrong, and treating the two the same
//! would sever a live chain because a read glitched once. So an `Io`
//! error cuts nothing: it produces [`Action::Unverified`] and the walk
//! moves on, leaving the pointer exactly as it was for a later attempt.
//!
//! # Order of writes
//!
//! `bitmap_flag = 0` first, then the pages, then the extension blocks,
//! then the root's pointers, then `bitmap_flag = -1`. An interruption at
//! any point therefore leaves a volume that says "my bitmap is
//! mid-update" — which is the state it was already in if it needed
//! repairing, and an honest one if it did not.

use alloc::collections::BTreeSet;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

use crate::allocator::{AllocError, Allocator};
use crate::format::{div_ceil, wr32};
use crate::layout::*;
use crate::read::{Error, Volume};
use crate::validate::{Reached, Report, MAX_FINDINGS};
use crate::{be32, checksum_compute, checksum_ok, BlockMedium, BlockSource};

/// What a repair is allowed to do beyond rebuilding the bitmap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RepairOptions {
    /// Cut out the damage the walk proved it cannot follow: truncate a
    /// hash chain at its last good link, zero a comment pointer whose
    /// block will not read. **Off by default** — see this module's
    /// documentation. Entries beyond a truncated link stay on the disk
    /// and stay allocated; they are leaked, not freed.
    pub sever: bool,
}

impl RepairOptions {
    /// The default: rebuild the bitmap, sever nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Allow severing.
    pub fn sever(mut self, sever: bool) -> Self {
        self.sever = sever;
        self
    }
}

/// One thing a repair did, mirroring the [`Finding`](crate::Finding) it
/// answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// A block the walk reached that the bitmap called free is now
    /// allocated. The answer to
    /// [`Finding::ReachableButFree`](crate::Finding::ReachableButFree).
    Allocated {
        /// The block.
        lba: u64,
    },
    /// A block the bitmap calls allocated that nothing reaches, kept
    /// allocated on purpose. The *non*-answer to
    /// [`Finding::OrphanBlock`](crate::Finding::OrphanBlock), recorded so
    /// that "the leak is still there" is a thing the report says rather
    /// than a thing the caller discovers.
    LeakKept {
        /// The block.
        lba: u64,
    },
    /// A bitmap page whose contents could not be read and were therefore
    /// rebuilt from the walk alone. Anything that was allocated in the
    /// region it covered and is not reachable is lost with it — the one
    /// place a repair cannot preserve allocation, and so the one worth
    /// naming.
    PageUnreadable {
        /// The page block.
        lba: u64,
    },
    /// A bitmap page pointer that named nothing usable — zero, out of
    /// range, a duplicate, or a block the tree itself is using — replaced
    /// with a freshly allocated block.
    PageReplaced {
        /// Which page, in coverage order.
        index: usize,
        /// What the root (or an extension block) said before.
        was: u32,
        /// The block now holding it.
        lba: u64,
    },
    /// A bitmap extension block replaced, on the same terms.
    ExtReplaced {
        /// Which extension block, in chain order.
        index: usize,
        /// What was there before.
        was: u32,
        /// The block now holding it.
        lba: u64,
    },
    /// A hash chain truncated at its last good link: everything from
    /// `dropped` onward is no longer in the directory. Only ever emitted
    /// under [`RepairOptions::sever`].
    ChainTruncated {
        /// The directory.
        dir: u64,
        /// Which hash slot.
        slot: u32,
        /// The block the chain was cut *after*, or 0 when the slot itself
        /// was cleared.
        after: u64,
        /// The block that could not be followed.
        dropped: u64,
    },
    /// An entry's overflow-comment pointer zeroed because the block it
    /// named would not read as this entry's comment.
    CommentPointerCleared {
        /// The entry's header block.
        lba: u64,
        /// The pointer that was there.
        was: u32,
    },
    /// A chain link, or an entry's overflow comment, that could not be
    /// read because of a transport failure rather than a structural
    /// refusal.
    ///
    /// [`Error::Io`] proves nothing about the
    /// block's own bytes -- only that this one read attempt failed -- so
    /// nothing is cut and nothing is cleared. The block is left exactly
    /// as it was, unlike every other member of this enum.
    Unverified {
        /// The block that would not read.
        lba: u64,
    },
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allocated { lba } => {
                write!(f, "block {lba}: in use and marked free, now allocated")
            }
            Self::LeakKept { lba } => write!(
                f,
                "block {lba}: allocated and unreachable, kept allocated (a leak is recoverable; \
                 freeing a block an incomplete walk missed is not)"
            ),
            Self::PageUnreadable { lba } => write!(
                f,
                "bitmap page {lba} could not be read; its region was rebuilt from the walk alone"
            ),
            Self::PageReplaced { index, was, lba } => write!(
                f,
                "bitmap page {index} was block {was}, which is not usable; now block {lba}"
            ),
            Self::ExtReplaced { index, was, lba } => write!(
                f,
                "bitmap extension block {index} was block {was}, which is not usable; now block {lba}"
            ),
            Self::ChainTruncated {
                dir,
                slot,
                after,
                dropped,
            } => write!(
                f,
                "directory {dir} slot {slot}: chain truncated after block {after}, dropping \
                 block {dropped} and everything behind it (leaked, not freed)"
            ),
            Self::CommentPointerCleared { lba, was } => write!(
                f,
                "block {lba}: comment pointer to block {was} zeroed; the block is not its comment"
            ),
            Self::Unverified { lba } => write!(
                f,
                "block {lba} could not be read (transport failure); left exactly as it was, \
                 not proof of corruption"
            ),
        }
    }
}

/// What a repair did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RepairReport {
    /// Everything done, in the order it was done.
    pub actions: Vec<Action>,
    /// The action list hit [`MAX_FINDINGS`] and stopped growing; the
    /// counts below are still exact.
    pub truncated: bool,
    /// Blocks that were reachable and marked free, now allocated.
    pub allocated: u64,
    /// Blocks left allocated and unreachable: the leaks that stay.
    pub leaked: u64,
    /// Bitmap pages written.
    pub pages_written: u64,
    /// Bitmap pages and extension blocks that had to be moved.
    pub blocks_replaced: u64,
    /// Chains truncated plus comment pointers cleared.
    pub severed: u64,
}

impl RepairReport {
    fn push(&mut self, action: Action) {
        if self.actions.len() < MAX_FINDINGS {
            self.actions.push(action);
        } else {
            self.truncated = true;
        }
    }
}

/// The bitmap as it is on the disk, read without believing any of it.
struct OldBitmap {
    /// Page *i*'s block, or 0 where the pointer named nothing usable.
    pages: Vec<u64>,
    /// What the root or an extension block said, for the report.
    page_was: Vec<u32>,
    /// Extension block *i*'s block, or 0.
    ext: Vec<u64>,
    /// What the chain said.
    ext_was: Vec<u32>,
    /// Bits as read, `u32::MAX` (all free — that is, no information)
    /// wherever a page could not be read.
    words: Vec<u32>,
}

impl<S: BlockMedium> Volume<S> {
    /// Rebuild this volume's bitmap from a walk of its own tree, and
    /// stamp it valid.
    ///
    /// The operation `DiskValidator` performs, and the prerequisite for
    /// allocating from a volume whose flag is 0. See this module's
    /// documentation for the one-direction invariant it keeps and for
    /// what [`RepairOptions::sever`] adds.
    ///
    /// The volume's cached root is re-read before this returns, so the
    /// `Volume` reflects what is now on the disk.
    pub fn repair(
        &mut self,
        opts: &RepairOptions,
    ) -> Result<RepairReport, AllocError<<S as BlockSource>::Error>> {
        let mut rep = RepairReport::default();
        let bs = self.block_size();
        let reserved = self.reserved();
        let block_count = self.block_count();
        let root_lba = self.root_lba();

        if opts.sever {
            self.sever(&mut rep)?;
            self.reload_root().map_err(AllocError::Read)?;
        }

        // The definitive statement of what is in use, from the same
        // walker `validate()` runs — deliberately the same one, because
        // the bitmap this writes is *defined* as the set that walk
        // returns and a second implementation of it would be a second
        // answer.
        let mut walked = Report::default();
        let reached = self.walk_reachable(&mut walked);

        let per_page = bitmap_bits_per_block(bs);
        let per_ext = bitmap_ext_pointers(bs);
        let need_pages = div_ceil(block_count.saturating_sub(reserved), per_page) as usize;
        let need_ext = if need_pages > BITMAP_PAGES {
            div_ceil((need_pages - BITMAP_PAGES) as u64, per_ext as u64) as usize
        } else {
            0
        };
        let old = self.read_bitmap_tolerantly(&reached, need_pages, need_ext, &mut rep)?;

        // Say out loud that the bitmap is mid-update before touching a
        // page of it. Every write from here to the last one leaves a
        // volume that refuses to be allocated from, which is the only
        // safe thing a half-written bitmap can be.
        let mut alloc: Allocator<<S as BlockSource>::Error> =
            Allocator::rebuilt(reserved, block_count, bs, old.pages.clone());
        alloc.mark_bitmap_invalid(&mut self.src, root_lba)?;

        // The union, in two passes so each one's effect is countable:
        // what the old bitmap claimed, then what the walk reached.
        for (i, &w) in old.words.iter().enumerate() {
            if w == u32::MAX {
                continue;
            }
            let base = reserved + i as u64 * 32;
            for b in 0..32u64 {
                if w >> b & 1 == 0 {
                    // Ignore a bit for a block past the end of the
                    // volume: padding, whatever it holds.
                    let _ = alloc.mark_in_use(base + b);
                }
            }
        }
        for lba in reserved..block_count {
            if reached.get(lba) && alloc.mark_in_use(lba)? {
                rep.allocated += 1;
                rep.push(Action::Allocated { lba });
            }
        }
        for lba in reserved..block_count {
            if !reached.get(lba)
                && alloc.is_allocated(lba) == Some(true)
                && !old.pages.contains(&lba)
                && !old.ext.contains(&lba)
            {
                rep.leaked += 1;
                rep.push(Action::LeakKept { lba });
            }
        }

        // Replacement pages and extension blocks, allocated out of the
        // set just computed — which is the only set that could be used,
        // since the thing being replaced is the volume's own record of
        // what is free.
        let mut pages: Vec<u64> = old.pages.clone();
        pages.resize(need_pages, 0);
        for (i, page) in pages.iter_mut().enumerate() {
            if *page != 0 {
                alloc.mark_in_use(*page)?;
                alloc.set_page(i, *page);
                continue;
            }
            let a = alloc.allocate_near(root_lba + 1)?;
            *page = a.block();
            alloc.set_page(i, a.block());
            rep.blocks_replaced += 1;
            rep.push(Action::PageReplaced {
                index: i,
                was: old.page_was.get(i).copied().unwrap_or(0),
                lba: a.block(),
            });
        }
        let mut ext: Vec<u64> = old.ext.clone();
        ext.resize(need_ext, 0);
        for (i, block) in ext.iter_mut().enumerate() {
            if *block != 0 {
                alloc.mark_in_use(*block)?;
                continue;
            }
            let a = alloc.allocate_near(root_lba + 1)?;
            *block = a.block();
            rep.blocks_replaced += 1;
            rep.push(Action::ExtReplaced {
                index: i,
                was: old.ext_was.get(i).copied().unwrap_or(0),
                lba: a.block(),
            });
        }

        // Every page, not only the changed ones: a rebuilt bitmap is
        // dirty by definition, and a page left with its old contents is
        // the half-repaired state this exists to end.
        rep.pages_written = alloc.flush(&mut self.src)? as u64;

        for (i, &lba) in ext.iter().enumerate() {
            let mut buf = vec![0u8; bs];
            let first = BITMAP_PAGES + i * per_ext;
            for (k, &page) in pages.iter().skip(first).take(per_ext).enumerate() {
                wr32(&mut buf, k * 4, page as u32);
            }
            wr32(
                &mut buf,
                bitmap_ext_next(bs),
                ext.get(i + 1).copied().unwrap_or(0) as u32,
            );
            // No type, no own key, no checksum: a bitmap extension block
            // is a bare pointer array, and there is nothing in one to
            // compute.
            self.src.write_block(lba, &buf).map_err(AllocError::Io)?;
        }

        // The root's pointers, with the flag still 0...
        let mut buf = self.get_block(root_lba)?;
        for i in 0..BITMAP_PAGES {
            let page = pages.get(i).copied().unwrap_or(0);
            wr32(&mut buf, tail(bs, TL_BITMAP_PAGES) + i * 4, page as u32);
        }
        wr32(
            &mut buf,
            tail(bs, TL_BITMAP_EXT),
            ext.first().copied().unwrap_or(0) as u32,
        );
        self.put_block(root_lba, &mut buf)?;
        // ...and only now the flag, in a write of its own, after every
        // page it vouches for is on the disk.
        alloc.mark_bitmap_valid(&mut self.src, root_lba, self.variant)?;

        self.reload_root().map_err(AllocError::Read)?;
        Ok(rep)
    }

    /// Read the bitmap's pages without trusting a pointer or a checksum.
    ///
    /// [`Volume::read_bitmap`](crate::Volume) refuses a page whose
    /// checksum does not balance, which is right for a reader and useless
    /// here: the pages a repair most needs to look at are exactly the
    /// broken ones. Every pointer is range-checked, checked for being a
    /// duplicate, and checked against the walk — a "bitmap page" the tree
    /// is using is not a bitmap page, whatever the root says — and a page
    /// that fails any of those, or whose checksum does not balance,
    /// contributes no bits rather than wrong ones.
    fn read_bitmap_tolerantly(
        &mut self,
        reached: &Reached,
        need_pages: usize,
        need_ext: usize,
        rep: &mut RepairReport,
    ) -> Result<OldBitmap, AllocError<<S as BlockSource>::Error>> {
        let bs = self.block_size();
        let reserved = self.reserved();
        let block_count = self.block_count();
        let root_lba = self.root_lba();
        let words_per_page = (bs - OFF_BITMAP_BITS) / 4;
        let per_ext = bitmap_ext_pointers(bs);

        let mut page_was: Vec<u32> = self
            .root()
            .bitmap_pages
            .iter()
            .copied()
            .take(need_pages)
            .collect();
        let mut ext_was: Vec<u32> = Vec::new();
        let mut next = self.root().bitmap_ext;
        let mut seen: Vec<u64> = Vec::new();
        while next != 0 && ext_was.len() < need_ext {
            let lba = next as u64;
            if lba >= block_count || lba < reserved || seen.contains(&lba) {
                break;
            }
            seen.push(lba);
            ext_was.push(next);
            if self.read_raw(lba).is_err() {
                break;
            }
            for i in 0..per_ext {
                if page_was.len() >= need_pages {
                    break;
                }
                page_was.push(be32(&self.buf, i * 4));
            }
            next = be32(&self.buf, bitmap_ext_next(bs));
        }
        page_was.resize(need_pages, 0);
        ext_was.resize(need_ext, 0);

        // A pointer is usable only if it is in the volume, is not already
        // claimed by another pointer, is not the root, and is not a block
        // the tree reaches.
        let mut claimed: Vec<u64> = vec![root_lba];
        let usable = |lba: u64, claimed: &mut Vec<u64>| -> bool {
            if lba < reserved || lba >= block_count || claimed.contains(&lba) || reached.get(lba) {
                return false;
            }
            claimed.push(lba);
            true
        };
        let mut ext: Vec<u64> = Vec::with_capacity(need_ext);
        for &was in &ext_was {
            let lba = was as u64;
            ext.push(if usable(lba, &mut claimed) { lba } else { 0 });
        }
        let mut pages: Vec<u64> = Vec::with_capacity(need_pages);
        for &was in &page_was {
            let lba = was as u64;
            pages.push(if usable(lba, &mut claimed) { lba } else { 0 });
        }

        let mut words = vec![u32::MAX; need_pages * words_per_page];
        for (i, &page) in pages.iter().enumerate() {
            if page == 0 {
                continue;
            }
            let ok = self.read_raw(page).is_ok() && checksum_ok(&self.buf);
            if !ok {
                rep.push(Action::PageUnreadable { lba: page });
                continue;
            }
            for w in 0..words_per_page {
                words[i * words_per_page + w] = be32(&self.buf, OFF_BITMAP_BITS + w * 4);
            }
        }

        Ok(OldBitmap {
            pages,
            page_was,
            ext,
            ext_was,
            words,
        })
    }

    /// Cut out what the reader has proved it cannot follow.
    ///
    /// Every directory reachable from the root, every hash slot, every
    /// chain: a link whose header block will not parse, or which revisits
    /// a block already in the chain, is removed by writing 0 into
    /// whichever longword pointed at it — the slot itself for a head, the
    /// previous entry's chain longword otherwise. The entries behind the
    /// cut stay on the disk and stay allocated: this removes reachability
    /// and nothing else, which is the other half of the invariant.
    fn sever(
        &mut self,
        rep: &mut RepairReport,
    ) -> Result<(), AllocError<<S as BlockSource>::Error>> {
        let bs = self.block_size();
        let slots = crate::hash_table_size(bs) as usize;
        let mut queue = vec![self.root_lba()];
        // `BTreeSet`s, not `Vec`s: both `visited` (every directory found
        // so far) and each slot's own `chain` can be as long as a
        // hostile volume likes, and a `Vec::contains` scan per step is
        // the same O(n^2) shape `guard_chain` had — see `read.rs`'s doc
        // comment on it.
        let mut visited: BTreeSet<u64> = BTreeSet::from([self.root_lba()]);

        while let Some(dir) = queue.pop() {
            let table = match self.hash_table(dir) {
                Ok(t) => t,
                // A directory whose own block will not read has already
                // been severed by whoever pointed at it, or is the root
                // and beyond this operation's help.
                Err(_) => continue,
            };
            for (slot, head) in table.iter().enumerate().take(slots) {
                let mut prev: u64 = 0;
                let mut next = *head;
                let mut chain: BTreeSet<u64> = BTreeSet::new();
                while next != 0 {
                    let lba = next as u64;
                    // A cycle or an out-of-range pointer is proof on its
                    // own -- no read needed to know this link cannot be
                    // followed.
                    if chain.contains(&lba) || lba >= self.block_count() {
                        self.cut(dir, slot as u32, prev)?;
                        rep.severed += 1;
                        rep.push(Action::ChainTruncated {
                            dir,
                            slot: slot as u32,
                            after: prev,
                            dropped: lba,
                        });
                        break;
                    }
                    // `Error::Io` is not proof about this block's own
                    // bytes, only that this attempt to read it failed --
                    // a flaky sector or a transient bus error says
                    // nothing about whether the chain is intact. Every
                    // other error here (`Checksum`, `NotHeader`,
                    // `UnknownSecondaryType`, ...) is the reader having
                    // actually looked at the block's content and found
                    // it wrong, which is what this module's own
                    // documentation means by "proved it cannot follow".
                    let entry = match self.entry_at(lba) {
                        Ok(e) => e,
                        Err(Error::Io(_)) => {
                            rep.push(Action::Unverified { lba });
                            break;
                        }
                        Err(_) => {
                            self.cut(dir, slot as u32, prev)?;
                            rep.severed += 1;
                            rep.push(Action::ChainTruncated {
                                dir,
                                slot: slot as u32,
                                after: prev,
                                dropped: lba,
                            });
                            break;
                        }
                    };
                    chain.insert(lba);

                    if entry.comment_block != 0 {
                        match self.comment(&entry) {
                            Ok(_) => {}
                            Err(Error::Io(_)) => {
                                rep.push(Action::Unverified {
                                    lba: entry.comment_block as u64,
                                });
                            }
                            Err(_) => {
                                let mut buf = self.get_block(lba)?;
                                wr32(&mut buf, tail(bs, TL_COMMENT_BLOCK), 0);
                                self.put_block(lba, &mut buf)?;
                                rep.severed += 1;
                                rep.push(Action::CommentPointerCleared {
                                    lba,
                                    was: entry.comment_block,
                                });
                            }
                        }
                    }
                    if entry.kind.is_directory() && visited.insert(entry.lba) {
                        queue.push(entry.lba);
                    }
                    prev = lba;
                    next = entry.hash_chain;
                }
            }
        }
        Ok(())
    }

    /// Write a 0 where the broken link was: into the directory's hash
    /// slot when it was the head of the chain, into the previous entry's
    /// chain longword otherwise.
    fn cut(
        &mut self,
        dir: u64,
        slot: u32,
        prev: u64,
    ) -> Result<(), AllocError<<S as BlockSource>::Error>> {
        let bs = self.block_size();
        if prev == 0 {
            let mut buf = self.get_block(dir)?;
            wr32(&mut buf, OFF_HASH_TABLE + slot as usize * 4, 0);
            self.put_block(dir, &mut buf)
        } else {
            let mut buf = self.get_block(prev)?;
            wr32(&mut buf, tail(bs, TL_HASH_CHAIN), 0);
            self.put_block(prev, &mut buf)
        }
    }

    /// Read a metadata block whole, checksum verified.
    fn get_block(&mut self, lba: u64) -> Result<Vec<u8>, AllocError<<S as BlockSource>::Error>> {
        self.read_raw(lba).map_err(AllocError::Read)?;
        if !checksum_ok(&self.buf) {
            return Err(AllocError::Read(Error::Checksum { lba }));
        }
        Ok(self.buf.clone())
    }

    /// Fix a metadata block's checksum at longword 5 and write it back.
    fn put_block(
        &mut self,
        lba: u64,
        buf: &mut [u8],
    ) -> Result<(), AllocError<<S as BlockSource>::Error>> {
        let ck = checksum_compute(buf, CHECKSUM_INDEX);
        wr32(buf, OFF_CHECKSUM, ck);
        self.src.write_block(lba, buf).map_err(AllocError::Io)
    }
}
