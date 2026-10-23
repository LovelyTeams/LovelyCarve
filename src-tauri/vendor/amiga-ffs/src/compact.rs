//! Compaction and defragmentation: the block-layout policy
//! (`docs/layout-survey.md`, `src/allocator.rs`'s "Block layout policy"
//! section, `src/populate.rs`'s own documentation) applied *retroactively*
//! to a volume that already exists, and the primitive `resize()` needs to
//! close its own known gap. Wave 2 of the PLAN.md "Block layout policy,
//! and compaction" entry; wave 1 was policy at creation time only.
//!
//! # Two tiers, and why they cost so differently
//!
//! **Tier 1** ([`Mutator::defragment_file`]) relocates a file's data
//! blocks — and its `T_LIST` extension blocks, which the survey's own
//! measurement (§4a's wave-1 addendum) puts *in* the stream, not near the
//! header — into one ascending run, without moving the header. **Tier 2**
//! ([`Mutator::relocate_header`]) moves a directory or file *header* block
//! itself to a new LBA, which is the survey's §6b-3 recommendation and the
//! operation `docs/layout-survey.md` names as "where the bugs would
//! live" — because unlike tier 1, which only ever changes what a header's
//! *own* table points at, tier 2 changes the header's own number, and
//! *everything* that names a block by number has to follow it.
//!
//! # Tier 1's atomicity: one commit, like `edit_file`
//!
//! A file's data blocks are read-only from the header's point of view
//! until the very last write. This mirrors [`Mutator`]'s existing
//! `edit_file` shape exactly, deliberately: allocate the whole destination
//! run first ([`crate::Allocator::allocate_run`]), write every data and extension
//! block at its new home (extension blocks written last-block-first, so no
//! `next` ever names a block that is not there yet), then **one** header
//! write that swaps the table and the extension pointer over — before that
//! write the file is entirely the old one, after it entirely the new one —
//! then free the old blocks. `bitmap_flag` never leaves −1: every
//! intermediate state is exactly the ordinary [`Mutator`] guarantee ("the
//! old volume, the new volume, or the old volume plus leaked blocks"), the
//! same one `edit_file` already gives a resize in miniature, applied to
//! block *position* instead of file *length*. This is strictly better than
//! the bar `docs/layout-survey.md` §1 sets: ReOrg's own author documents a
//! "moving blocks... must not be interrupted" phase with an explicit
//! backup warning; this crate has no such phase, in either tier.
//!
//! # Tier 2's atomicity: everything hard-checked gets a fresh copy first
//!
//! The naive approach — patch a dependent block's owner field in place to
//! point at the header's *new* number — cannot be made atomic, because the
//! block being patched is *shared*: whichever pointer is currently
//! authoritative (the parent's hash slot, naming either the old or the new
//! header number) determines which value that dependent's owner field
//! must already hold for an ordinary read to succeed, and a single block
//! cannot hold both values at once. [`crate::Volume::file_chain`]'s
//! `T_LIST`-block parent check, OFS's `verify_ofs_data` header-key check
//! and [`crate::Volume::comment`]'s comment-block header-key check are all *hard*
//! read errors (unlike a directory child's `parent` longword, which
//! [`crate::resize`]'s own root-move already leaves briefly stale and
//! which only [`Finding::ParentMismatch`](crate::Finding::ParentMismatch)
//! reports) — so patching them in place, whichever side of the commit it
//! happens on, leaves a window in which a crash makes the file
//! unreadable through the *currently authoritative* header number.
//!
//! So tier 2 does not patch in place. Every hard-checked dependent — a
//! file's extension blocks (both variants: [`crate::Finding`] aside, the parent
//! check in [`crate::Volume::file_chain`] is unconditional), a directory's own
//! dircache chain, an LNFS overflow comment block, and — the cost worth
//! documenting plainly — **every OFS data block**, because
//! `verify_ofs_data` checks `header_key` on every one of them — gets a
//! **fresh copy** at a new LBA, built with the new header number already
//! in it, before the header itself is copied to its new LBA and before
//! anything points at any of it. Only then does a single write retarget
//! whichever pointer currently names the header (the parent's hash slot,
//! or the previous entry's hash-chain longword — found and replaced the
//! way [`Mutator`]'s private `unlink` walk finds a splice point, except
//! this write substitutes rather than removes) — and from the instant
//! that lands, the header's new number is *immediately* fully self-
//! consistent, because every dependent it can reach already agrees with
//! it. Soft-checked follow-up (a moved directory's direct children,
//! [`crate::Entry::real_entry`] in every hard link naming a moved target, the
//! parent's own dircache record) happens after, on the same "safe either
//! order" basis [`crate::resize`]'s reparenting already relies on.
//!
//! **The cost, stated plainly, per the survey's own instruction not to
//! bury the number a reader would want:** on FFS, relocating a file's
//! header costs its extension blocks only — the data blocks have no
//! back-pointer to a header at all, so they never move. On OFS, it costs
//! **every data block**, because `header_key` is real and enforced.
//! Relocating a header therefore is not cheap on OFS the way tier 1 is;
//! callers that want directory-header locality without that cost should
//! prefer running tier 1 alone, which is most of the measured benefit
//! (`docs/layout-survey.md` §1, ReOrg's own author's finding, restated in
//! `src/allocator.rs`) for a fraction of tier 2's price.
//!
//! `bitmap_flag` is **not** honest throughout a tier-2 relocation, on
//! purpose: it is forced to 0 for the whole operation, the same choice
//! [`crate::resize`]'s root move and [`crate::populate::Populator`] both
//! already make, and for the same reason — the operation is not a single
//! block write, so there is no single instant to point `Allocator::load`
//! at as "trustworthy," and honestly saying so is better than a bitmap
//! that is only sometimes describable as either "old" or "new." A crash
//! mid-operation leaves the bitmap flagged invalid; [`crate::Volume::repair`]
//! (which never removes reachability, only adds it) always restores a
//! valid bitmap, and — because every hard-checked dependent was staged
//! *before* the commit rather than patched in place — a **retry** of the
//! same relocation (looked up by name, since the header's number may or
//! may not have already changed) always finishes whatever was left
//! half-done, exactly [`crate::resize`]'s own documented "Retrying"
//! contract, generalized from "the root" to "any header." See this
//! module's crash-sweep tests for the two paths pinned: `repair()` alone
//! never leaves `ReachableButFree` and never leaves a corrupt-but-
//! unreachable block un-repairable; `repair()` **then a retry** always
//! reaches a clean, byte-identical volume.
//!
//! # `make_room`: the primitive `resize()`'s shrink actually needs
//!
//! [`Mutator::make_room`] evacuates every *movable* allocated block out of
//! a caller-given LBA range: a header found there is relocated (tier 2);
//! a file's data or extension block found there relocates the *whole*
//! file (tier 1, since a file's data blocks are one unit once any of them
//! has to move); a dircache or comment block found there relocates its
//! *owning* header (tier 2), which — per this module's own design above —
//! always gives that block a fresh copy elsewhere as a side effect. The
//! root, the bitmap's own pages and its extension blocks are **not**
//! movable by this operation — [`crate::resize`] already knows how to
//! relocate exactly those three kinds of block as part of a shrink, and
//! duplicating that here would be two implementations of one piece of
//! reasoning. This is precisely the complement of
//! [`crate::resize`]'s own `movable_metadata`: that function names what a
//! shrink's cut may pass through *without* relocating anything;
//! `make_room` is what relocates everything else out of the way first.
//!
//! # Wiring into `resize`
//!
//! [`crate::resize::ResizeError::RootTargetOccupied`] fires when a
//! shrink's new root midpoint lands on a block resize does not relocate.
//! [`crate::Volume::resize_evacuating`] is a separate, explicitly
//! opt-in method (not a flag on [`crate::Volume::resize`] itself —
//! see its own documentation for why the plain refusal stays what
//! `resize` does by default) that retries the shrink once after an
//! internal `make_room` of the target block.
//! [`crate::Volume::minimum_size_floor`] reports the size a shrink could
//! reach *if* it were willing to relocate user data out of the root's
//! path — the number `docs/layout-survey.md` and PLAN.md both call "the
//! theoretical floor" — alongside the existing, more conservative
//! [`crate::Volume::minimum_size`].
//!
//! # What is deliberately out of scope
//!
//! Full-volume policy compaction ([`Mutator::compact`]) runs tier 1 across
//! every file by default; tier 2 (header clustering toward the root) is
//! opt-in via [`CompactOptions::relocate_headers`], off by default. This
//! is a considered scoping choice, not an oversight: tier 1 alone is most
//! of the measured benefit (the survey's own ReOrg citation and this
//! crate's frag-bench rig agree), tier 2's OFS cost is real, and a
//! default that quietly rewrites every directory header on every compact
//! is a bigger blast radius than a defragmentation pass should have by
//! default. Callers that want full metadata clustering opt in explicitly,
//! or call [`Mutator::relocate_header`] directly, entry by entry.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;
use core::ops::Range;

use crate::allocator::{AllocError, Allocation, Intent};
use crate::build::{build_extension_block, finish_checksum};
use crate::format::wr32;
use crate::layout::*;
use crate::mutate::{MutateError, Mutator};
use crate::read::EntryKind;
use crate::{be32, hash_table_size, BlockMedium, Transport};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Everything compaction refuses, on top of what [`Mutator`] already
/// refuses for the same reasons ([`CompactError::Mutate`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompactError<E> {
    /// An ordinary [`Mutator`] refusal — a bad checksum, a chain that
    /// will not parse, the allocator declining. See [`MutateError`].
    Mutate(MutateError<E>),
    /// [`Mutator::make_room`] (or an internal caller of it) could not find
    /// room to relocate a block *outside* the given range, after the
    /// bounded number of retries this module allows. Distinct from plain
    /// [`AllocError::VolumeFull`]: the volume may well have free space,
    /// just none of it outside the excluded range.
    NoRoomOutsideRange {
        /// The range nothing could be moved out of.
        range: Range<u64>,
    },
    /// [`Mutator::relocate_header`] or [`Mutator::defragment_file`] was
    /// asked to move the volume root, which is not a directory entry and
    /// whose position is derived (`canonical_root_lba`), not chosen.
    /// [`crate::resize`] is the root's own relocation path.
    IsRoot {
        /// The root block.
        lba: u64,
    },
}

impl<E: fmt::Display> fmt::Display for CompactError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mutate(e) => write!(f, "{e}"),
            Self::NoRoomOutsideRange { range } => write!(
                f,
                "no free block outside {}..{} to relocate into",
                range.start, range.end
            ),
            Self::IsRoot { lba } => write!(
                f,
                "block {lba} is the volume root -- its position is derived, not chosen; \
                 relocate it through crate::resize instead"
            ),
        }
    }
}

#[cfg(feature = "std")]
impl<E: std::error::Error + 'static> std::error::Error for CompactError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Mutate(e) => Some(e),
            _ => None,
        }
    }
}

impl<E> From<MutateError<E>> for CompactError<E> {
    fn from(e: MutateError<E>) -> Self {
        Self::Mutate(e)
    }
}

impl<E> From<AllocError<E>> for CompactError<E> {
    fn from(e: AllocError<E>) -> Self {
        Self::Mutate(MutateError::from(e))
    }
}

impl<E> From<crate::read::Error<E>> for CompactError<E> {
    fn from(e: crate::read::Error<E>) -> Self {
        Self::Mutate(MutateError::from(e))
    }
}

// ---------------------------------------------------------------------------
// Reports
// ---------------------------------------------------------------------------

/// What [`Mutator::defragment_file`] did to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDefragReport {
    /// The file's header (unchanged: tier 1 never moves it).
    pub header_lba: u64,
    /// Ascending runs the data-and-extension fetch-order sequence took
    /// before this call.
    pub runs_before: usize,
    /// The same count afterwards. Equal to `runs_before` (and nothing
    /// written) when the file was already one run.
    pub runs_after: usize,
    /// Data and extension blocks relocated. Zero when nothing moved.
    pub blocks_relocated: u64,
}

/// What [`Mutator::relocate_header`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderRelocateReport {
    /// Where the header was.
    pub old_lba: u64,
    /// Where it is now.
    pub new_lba: u64,
    /// Dependent blocks given a fresh copy: extension blocks, OFS data
    /// blocks, dircache blocks, an overflow comment block — see this
    /// module's documentation for which, and why each one has to be.
    pub dependents_relocated: u64,
    /// Direct children reparented (a moved directory only).
    pub children_reparented: u64,
    /// Hard links whose `real_entry` was repointed (a moved link target
    /// only), plus, when the moved entry is itself a link, the one
    /// predecessor in its own link chain that named it.
    pub links_repointed: u64,
}

/// What [`Mutator::make_room`] did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MakeRoomReport {
    /// Headers relocated out of the range (tier 2).
    pub headers_relocated: u64,
    /// Files whose data/extension blocks were relocated because some of
    /// them fell in the range (tier 1).
    pub files_defragmented: u64,
    /// Allocated blocks that were in the range before this call and are
    /// not, one way or another, after it.
    pub blocks_evacuated: u64,
    /// Allocated blocks still in `range` when this returned, because the
    /// survey found no owner for them — deliberately, for the root's own
    /// dircache chain (`Survey`'s own doc comment: excluded from its
    /// `owner_of` reach on purpose, since relocating it is
    /// [`crate::resize`]'s job, not this one's), or incidentally, for a
    /// genuine orphan a `repair` has not reclaimed yet. Empty on an
    /// ordinary success.
    ///
    /// This exists so a caller can tell "evacuated everything the range
    /// asked for" apart from "stopped early, having made zero progress on
    /// a block it cannot move" — both return `Ok`, since neither is this
    /// function's own error to raise, but only the second leaves something
    /// here. [`Volume::resize_evacuating`](crate::Volume::resize_evacuating)
    /// checks this rather than only `is_err()`, so it does not blindly
    /// retry a resize that is guaranteed to hit the same collision again.
    pub not_evacuated: Vec<u64>,
}

/// One relocation, for [`Mutator::compact`]'s progress callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompactEvent {
    /// A tier-1 file relocation happened (or would have, in a dry run).
    FileDefragged(FileDefragReport),
    /// A tier-2 header relocation happened (or would have).
    HeaderRelocated(HeaderRelocateReport),
}

/// What a full [`Mutator::compact`] pass did, or would do.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompactReport {
    /// Whether this was [`CompactOptions::dry_run`] — nothing written.
    pub dry_run: bool,
    /// Files looked at.
    pub files_examined: u64,
    /// Files whose data was not already one run (moved, or — in a dry
    /// run — would have been).
    pub files_relocated: u64,
    /// Headers relocated — always 0 unless
    /// [`CompactOptions::relocate_headers`] was set.
    pub headers_relocated: u64,
    /// Sum of every examined file's fetch-order run count before.
    pub runs_before_total: u64,
    /// Sum of every examined file's fetch-order run count after (or, in a
    /// dry run, what it would be — 1 per file, since tier 1 always
    /// achieves a single run when it moves anything at all and a volume
    /// with enough free space to hold the file once holds it as one run).
    pub runs_after_total: u64,
}

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

/// [`Mutator::compact`]'s knobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CompactOptions {
    /// Compute and return a [`CompactReport`] without writing anything.
    /// The report undercounts nothing tier 1 would actually achieve
    /// (see [`CompactReport::runs_after_total`]'s documentation) but does
    /// not simulate a full disk layout, so it is a preview of *how much
    /// work there is*, not a bit-exact prediction of final placement.
    pub dry_run: bool,
    /// Also run tier 2 (header relocation) across every directory,
    /// clustering headers back toward the root the way
    /// [`crate::populate::Populator`] places them at creation time.
    /// **Off by default** — see this module's documentation for why.
    pub relocate_headers: bool,
}

impl CompactOptions {
    /// The default: full tier-1 pass, nothing dry, tier 2 off.
    pub fn new() -> Self {
        Self::default()
    }

    /// Preview mode: compute the report, write nothing.
    pub fn dry_run(mut self, dry_run: bool) -> Self {
        self.dry_run = dry_run;
        self
    }

    /// Also relocate headers (tier 2).
    pub fn relocate_headers(mut self, relocate_headers: bool) -> Self {
        self.relocate_headers = relocate_headers;
        self
    }
}

// ---------------------------------------------------------------------------
// Small shared arithmetic
// ---------------------------------------------------------------------------

/// How many ascending runs a sequence of blocks takes. Zero blocks is
/// zero runs. Mirrors `tests/volumes.rs`'s and `examples/frag-bench.rs`'s
/// own `count_runs`, kept in step by definition rather than by
/// cross-reference: all three exist to answer exactly "does the next
/// block follow the last one."
fn count_runs(blocks: &[u64]) -> usize {
    if blocks.is_empty() {
        return 0;
    }
    let mut runs = 1;
    for w in blocks.windows(2) {
        if w[1] != w[0] + 1 {
            runs += 1;
        }
    }
    runs
}

/// The full sequence of blocks a streaming read actually touches, in
/// fetch order: data blocks, with each `T_LIST` extension block spliced
/// in exactly where a reader fetches it. Mirrors
/// `tests/volumes.rs`'s `read_path_sequence`; see this module's
/// documentation and `docs/layout-survey.md` §4a's wave-1 addendum for
/// why this, not [`crate::FileChain::blocks`] alone, is the sequence tier
/// 1 relocates as one run. Each entry is `(is_extension, old_lba)`.
fn fetch_order(chain: &crate::FileChain, block_size: usize) -> Vec<(bool, u64)> {
    let slots = hash_table_size(block_size) as usize;
    let mut out = Vec::with_capacity(chain.blocks.len() + chain.extensions.len());
    for (i, &b) in chain.blocks.iter().enumerate() {
        out.push((false, b as u64));
        if (i + 1) % slots == 0 {
            let ext_index = (i + 1) / slots - 1;
            if let Some(&e) = chain.extensions.get(ext_index) {
                out.push((true, e as u64));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tier 1: file-data relocation
// ---------------------------------------------------------------------------

impl<S: BlockMedium> Mutator<S> {
    /// Relocate a file's data-and-extension-block sequence into one
    /// ascending run, without moving the header. See this module's
    /// documentation for the single-commit atomicity this achieves and
    /// why it is `edit_file`'s shape applied to position instead of
    /// length.
    ///
    /// A no-op — nothing written, an unchanged report — when the file's
    /// fetch-order sequence (`docs/layout-survey.md` §4a's wave-1
    /// addendum: data blocks with `T_LIST` extension blocks spliced in at
    /// their natural stream position, not the data pointers alone) is
    /// already one run.
    ///
    /// Refuses [`MutateError::NotAFile`] for anything that is not a plain
    /// file (a directory, a link, a soft link — none of these have a data
    /// chain of their own to defragment).
    pub fn defragment_file(
        &mut self,
        header_lba: u64,
    ) -> Result<FileDefragReport, CompactError<Transport<S>>> {
        self.defragment_file_avoiding(header_lba, None)
    }

    pub(crate) fn defragment_file_avoiding(
        &mut self,
        header_lba: u64,
        avoid: Option<&Range<u64>>,
    ) -> Result<FileDefragReport, CompactError<Transport<S>>> {
        let entry = self.vol.entry_at(header_lba)?;
        if entry.kind != EntryKind::File {
            return Err(MutateError::NotAFile {
                lba: header_lba,
                found: entry.kind.secondary_type(),
            }
            .into());
        }
        let bs = self.bs();
        let ffs = self.vol.variant().is_ffs();
        let slots = hash_table_size(bs) as usize;
        let chain = self.vol.file_chain(header_lba)?;

        let seq = fetch_order(&chain, bs);
        let old_lbas: Vec<u64> = seq.iter().map(|&(_, l)| l).collect();
        let runs_before = count_runs(&old_lbas);
        let n = old_lbas.len();

        // Which positions actually need a fresh block. Plain
        // defragmentation (`avoid` is `None`) always moves everything --
        // that is tier 1's whole point, one run out of however many. A
        // range-avoiding evacuation (`make_room`'s caller) only *needs*
        // the blocks actually inside the excluded range off it; moving a
        // whole multi-hundred-block file to clear one block out of its
        // way would cost a second full copy this volume may well not
        // have room for, for no benefit `make_room` asked for.
        // Extension blocks are the one exception, moved unconditionally
        // whenever anything else moves: an extension block's table names
        // data blocks by LBA, so a data block moving anywhere requires
        // every extension block whose table lists it to be rewritten
        // regardless -- and since there are few of them (~1 per 36 KB),
        // giving every one a fresh copy keeps this function to the one
        // already-proven "prepare everything, one commit" shape instead
        // of a second, subtler one for the in-place case.
        let must_move: Vec<bool> = match avoid {
            None => vec![runs_before > 1; n],
            Some(r) => seq
                .iter()
                .map(|&(is_ext, l)| is_ext || r.contains(&l))
                .collect(),
        };
        if !must_move.iter().any(|&m| m) {
            return Ok(FileDefragReport {
                header_lba,
                runs_before,
                runs_after: runs_before,
                blocks_relocated: 0,
            });
        }

        // Allocate every position that has to move. Plain defragmentation
        // reserves one run up front (`crate::Allocator::allocate_run`'s
        // own documented fallback -- the longest run available, called
        // again for the remainder -- means a badly fragmented volume
        // still finishes, just not in one piece, reported honestly via
        // `runs_after`); an evacuation allocates the (typically few)
        // positions individually, since they are not contiguous with each
        // other to begin with and there is no run to preserve.
        let mut new_lbas = old_lbas.clone();
        if avoid.is_none() {
            let mut allocs: Vec<Allocation> = Vec::with_capacity(n);
            let mut hint = header_lba;
            let mut got = 0u64;
            while got < n as u64 {
                let run = self.alloc_run_avoiding(
                    hint,
                    n as u64 - got,
                    Intent::DataFor { header_lba },
                    avoid,
                )?;
                if run.is_empty() {
                    return Err(CompactError::NoRoomOutsideRange { range: 0..0 });
                }
                hint = run.last().map(|a| a.block() + 1).unwrap_or(hint);
                got += run.len() as u64;
                allocs.extend(run);
            }
            self.flush()?;
            for (i, a) in allocs.iter().enumerate() {
                new_lbas[i] = self.alloc.reference(a).map_err(MutateError::from)?;
            }
        } else {
            let mut allocs: Vec<(usize, Allocation)> = Vec::new();
            let mut hint = header_lba;
            for (i, &m) in must_move.iter().enumerate() {
                if !m {
                    continue;
                }
                let a = self.alloc_near_avoiding(hint, Intent::DataFor { header_lba }, avoid)?;
                hint = a.block();
                allocs.push((i, a));
            }
            self.flush()?;
            for (i, a) in &allocs {
                new_lbas[*i] = self.alloc.reference(a).map_err(MutateError::from)?;
            }
        }
        let runs_after = count_runs(&new_lbas);

        let mut new_data: Vec<u64> = Vec::with_capacity(chain.blocks.len());
        let mut new_ext: Vec<u64> = Vec::with_capacity(chain.extensions.len());
        for (&(is_ext, _), &nl) in seq.iter().zip(new_lbas.iter()) {
            if is_ext {
                new_ext.push(nl);
            } else {
                new_data.push(nl);
            }
        }

        // Data blocks: bytes are unchanged (the header did not move), so
        // an FFS block is copied verbatim -- read unchecked, since a raw
        // FFS data block carries no checksum of its own and is not
        // expected to pass one. An OFS block only needs its redundant
        // (unenforced, per `crate::file`'s own documentation) `next`
        // pointer updated to the block's own new neighbour. A block that
        // was not in `must_move` keeps `new_data[i] == old`: nothing to
        // do, and nothing here touches it, so a partial evacuation costs
        // exactly the blocks it had to move and no others.
        for i in 0..new_data.len() {
            let old = chain.blocks[i] as u64;
            if new_data[i] == old {
                continue;
            }
            if ffs {
                let buf = self.read_raw_block(old)?;
                self.write(new_data[i], &buf)?;
            } else {
                let mut buf = self.get(old)?;
                let next = new_data.get(i + 1).copied().unwrap_or(0) as u32;
                wr32(&mut buf, OFF_DATA_NEXT, next);
                finish_checksum(&mut buf);
                self.write(new_data[i], &buf)?;
            }
        }
        // Extension blocks: rebuilt (their table entries now name the
        // relocated data blocks), written last-block-first so no `next`
        // ever names a block that is not on the disk yet.
        for k in (0..new_ext.len()).rev() {
            let first = slots + k * slots;
            let count = (new_data.len().saturating_sub(first)).min(slots);
            let pointers: Vec<u32> = new_data[first..first + count]
                .iter()
                .map(|&d| d as u32)
                .collect();
            let next = new_ext.get(k + 1).copied().unwrap_or(0) as u32;
            let mut eb = vec![0u8; bs];
            build_extension_block(&mut eb, new_ext[k], header_lba, &pointers, next);
            self.write(new_ext[k], &eb)?;
        }

        // The commit: the header's table and extension pointer, in one
        // write. Everything else in the header (byte_size, high_seq,
        // dates, name, comment) is untouched.
        let in_header = new_data.len().min(slots);
        let mut hdr = self.get(header_lba)?;
        wr32(
            &mut hdr,
            OFF_FIRST_DATA,
            new_data.first().copied().unwrap_or(0) as u32,
        );
        for (i, &lba) in new_data.iter().take(in_header).enumerate() {
            wr32(&mut hdr, data_pointer_offset(bs, i as u32 + 1), lba as u32);
        }
        wr32(
            &mut hdr,
            tail(bs, TL_EXTENSION),
            new_ext.first().copied().unwrap_or(0) as u32,
        );
        self.put(header_lba, &mut hdr)?;

        let mut blocks_relocated = 0u64;
        for (i, &old) in chain.blocks.iter().enumerate() {
            if new_data[i] != old as u64 {
                self.alloc.free(old as u64).map_err(MutateError::from)?;
                blocks_relocated += 1;
            }
        }
        for &old in &chain.extensions {
            self.alloc.free(old as u64).map_err(MutateError::from)?;
            blocks_relocated += 1;
        }
        self.flush()?;
        self.stamp_blocks_used()?;

        Ok(FileDefragReport {
            header_lba,
            runs_before,
            runs_after,
            blocks_relocated,
        })
    }

    /// Read a block's raw bytes without a checksum check. Only for FFS
    /// data-block content, which carries no checksum to check.
    fn read_raw_block(&mut self, lba: u64) -> Result<Vec<u8>, MutateError<Transport<S>>> {
        self.vol.read_raw(lba)?;
        Ok(self.vol.buf.clone())
    }

    // -- shared allocation helpers, honouring an excluded range ----------

    fn alloc_near_avoiding(
        &mut self,
        hint: u64,
        intent: Intent,
        avoid: Option<&Range<u64>>,
    ) -> Result<Allocation, CompactError<Transport<S>>> {
        let mut h = hint;
        for _ in 0..4 {
            let a = self.alloc.allocate_for_hinted(intent, h)?;
            match avoid {
                Some(r) if r.contains(&a.block()) => {
                    self.alloc.free(a.block()).map_err(MutateError::from)?;
                    h = r.end;
                }
                _ => return Ok(a),
            }
        }
        Err(CompactError::NoRoomOutsideRange {
            range: avoid.cloned().unwrap_or(0..0),
        })
    }

    fn alloc_run_avoiding(
        &mut self,
        hint: u64,
        n: u64,
        intent: Intent,
        avoid: Option<&Range<u64>>,
    ) -> Result<Vec<Allocation>, CompactError<Transport<S>>> {
        let range = match avoid {
            Some(range) => range,
            None => return Ok(self.alloc.allocate_run(n, intent)?),
        };
        let mut h = hint;
        for _ in 0..4 {
            let run = self.alloc.allocate_run_hinted(n, intent, h)?;
            if run.iter().any(|a| range.contains(&a.block())) {
                for a in &run {
                    self.alloc.free(a.block()).map_err(MutateError::from)?;
                }
                h = range.end;
                continue;
            }
            return Ok(run);
        }
        Err(CompactError::NoRoomOutsideRange {
            range: range.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// Tier 2: header relocation
// ---------------------------------------------------------------------------

/// Where [`Mutator::relocate_header`]'s core picks the destination.
enum DestPick {
    /// Near the parent directory, the same locality
    /// [`Intent::HeaderIn`] already states the reasoning for.
    Auto,
    /// A caller-chosen block, already known free (used by
    /// [`Mutator::make_room`], which has already scanned the bitmap).
    Exact(u64),
}

impl<S: BlockMedium> Mutator<S> {
    /// Move a directory or file **header** block to a new LBA. See this
    /// module's documentation for everything that has to be re-pointed
    /// and why the header's dependents get fresh copies rather than an
    /// in-place patch.
    ///
    /// Refuses [`CompactError::IsRoot`] for the volume root — its
    /// position is derived from `block_count`, not chosen; see
    /// [`crate::resize`] to move it.
    pub fn relocate_header(
        &mut self,
        lba: u64,
    ) -> Result<HeaderRelocateReport, CompactError<Transport<S>>> {
        self.relocate_header_core(lba, DestPick::Auto, None)
    }

    /// [`Mutator::relocate_header`], to a specific already-known-free
    /// destination.
    pub fn relocate_header_to(
        &mut self,
        lba: u64,
        dest: u64,
    ) -> Result<HeaderRelocateReport, CompactError<Transport<S>>> {
        self.relocate_header_core(lba, DestPick::Exact(dest), None)
    }

    pub(crate) fn relocate_header_avoiding(
        &mut self,
        lba: u64,
        avoid: &Range<u64>,
    ) -> Result<HeaderRelocateReport, CompactError<Transport<S>>> {
        self.relocate_header_core(lba, DestPick::Auto, Some(avoid))
    }

    fn relocate_header_core(
        &mut self,
        old_lba: u64,
        dest: DestPick,
        avoid: Option<&Range<u64>>,
    ) -> Result<HeaderRelocateReport, CompactError<Transport<S>>> {
        let root_lba = self.vol.root_lba();
        if old_lba == root_lba {
            return Err(CompactError::IsRoot { lba: old_lba });
        }
        let entry = self.vol.entry_at(old_lba)?;
        let bs = self.bs();
        let variant = self.vol.variant();
        let ffs = variant.is_ffs();
        let slots = hash_table_size(bs) as usize;

        // Everything read-only, before a single byte is written.
        let chain = if entry.kind == EntryKind::File {
            Some(self.vol.file_chain(old_lba)?)
        } else {
            None
        };
        let old_dircache = if entry.kind == EntryKind::Directory && variant.has_dircache() {
            self.vol.read_dircache(old_lba)?.blocks
        } else {
            Vec::new()
        };
        let has_comment = entry.comment_block != 0;

        // Not a single commit -- see this module's documentation for why
        // -- so, like `crate::resize`'s root move and `Populator`, this
        // says so honestly for the duration.
        self.alloc
            .mark_bitmap_invalid(&mut self.vol.src, root_lba)
            .map_err(MutateError::from)?;

        // -- allocate every fresh block up front, one flush -----------
        let hdr_dest = match dest {
            DestPick::Auto => self.alloc_near_avoiding(
                entry.parent as u64,
                Intent::HeaderIn {
                    dir_lba: entry.parent as u64,
                },
                avoid,
            )?,
            DestPick::Exact(d) => self.alloc.allocate_exact(d).map_err(MutateError::from)?,
        };
        let mut hint = hdr_dest.block();

        let mut ext_allocs: Vec<Allocation> = Vec::new();
        let mut data_allocs: Vec<Allocation> = Vec::new();
        if let Some(ch) = &chain {
            for _ in &ch.extensions {
                let a = self.alloc_near_avoiding(
                    hint,
                    Intent::HeaderIn {
                        dir_lba: hdr_dest.block(),
                    },
                    avoid,
                )?;
                hint = a.block();
                ext_allocs.push(a);
            }
            if !ffs {
                // OFS only: every data block needs a fresh copy, because
                // `header_key` is a hard-checked field on every one of
                // them. Laid out as a run near the new header, which
                // makes this an incidental tier-1 defragmentation too --
                // the honest cost this move has on OFS, spent usefully.
                let n = ch.blocks.len() as u64;
                let mut got = 0u64;
                while got < n {
                    let run = self.alloc_run_avoiding(
                        hint,
                        n - got,
                        Intent::DataFor {
                            header_lba: hdr_dest.block(),
                        },
                        avoid,
                    )?;
                    if run.is_empty() {
                        return Err(CompactError::NoRoomOutsideRange {
                            range: avoid.cloned().unwrap_or(0..0),
                        });
                    }
                    hint = run.last().map(|a| a.block() + 1).unwrap_or(hint);
                    got += run.len() as u64;
                    data_allocs.extend(run);
                }
            }
        }
        let mut dc_allocs: Vec<Allocation> = Vec::new();
        for _ in &old_dircache {
            let a = self.alloc_near_avoiding(hint, Intent::MetadataNearRoot { root_lba }, avoid)?;
            hint = a.block();
            dc_allocs.push(a);
        }
        let cb_alloc = if has_comment {
            Some(self.alloc_near_avoiding(hint, Intent::HeaderIn { dir_lba: hint }, avoid)?)
        } else {
            None
        };

        self.flush()?;
        let new_lba = self.alloc.reference(&hdr_dest).map_err(MutateError::from)?;
        let new_ext: Vec<u64> = ext_allocs
            .iter()
            .map(|a| self.alloc.reference(a))
            .collect::<Result<_, _>>()
            .map_err(MutateError::from)?;
        let new_data: Vec<u64> = data_allocs
            .iter()
            .map(|a| self.alloc.reference(a))
            .collect::<Result<_, _>>()
            .map_err(MutateError::from)?;
        let new_dc: Vec<u64> = dc_allocs
            .iter()
            .map(|a| self.alloc.reference(a))
            .collect::<Result<_, _>>()
            .map_err(MutateError::from)?;
        let new_cb = match &cb_alloc {
            Some(a) => Some(self.alloc.reference(a).map_err(MutateError::from)?),
            None => None,
        };

        // -- write every fresh dependent, already naming `new_lba` -----
        if let Some(ch) = &chain {
            for k in (0..new_ext.len()).rev() {
                let first = slots + k * slots;
                let count = (ch.blocks.len().saturating_sub(first)).min(slots);
                let pointers: Vec<u32> = (0..count)
                    .map(|i| {
                        let old_d = ch.blocks[first + i] as u64;
                        if ffs {
                            old_d as u32
                        } else {
                            // `first + i` is exactly the index `old_d` was
                            // just read from above -- a search for it is
                            // always going to land back here, so this is
                            // the index directly rather than an O(n) scan
                            // repeated once per pointer (an O(n^2) rebuild
                            // for a file with many data/extension blocks).
                            new_data[first + i] as u32
                        }
                    })
                    .collect();
                let next = new_ext.get(k + 1).copied().unwrap_or(0) as u32;
                let mut eb = vec![0u8; bs];
                build_extension_block(&mut eb, new_ext[k], new_lba, &pointers, next);
                self.write(new_ext[k], &eb)?;
            }
            if !ffs {
                for i in (0..new_data.len()).rev() {
                    let mut buf = self.get(ch.blocks[i] as u64)?;
                    wr32(&mut buf, OFF_DATA_HEADER_KEY, new_lba as u32);
                    let next = new_data.get(i + 1).copied().unwrap_or(0) as u32;
                    wr32(&mut buf, OFF_DATA_NEXT, next);
                    finish_checksum(&mut buf);
                    self.write(new_data[i], &buf)?;
                }
            }
        }
        for i in (0..new_dc.len()).rev() {
            let mut buf = self.get(old_dircache[i])?;
            wr32(&mut buf, OFF_OWN_KEY, new_dc[i] as u32);
            wr32(&mut buf, OFF_DIRCACHE_PARENT, new_lba as u32);
            let next = new_dc.get(i + 1).copied().unwrap_or(0) as u32;
            wr32(&mut buf, OFF_DIRCACHE_NEXT, next);
            finish_checksum(&mut buf);
            self.write(new_dc[i], &buf)?;
        }
        if let Some(cb) = new_cb {
            let mut buf = self.get(entry.comment_block as u64)?;
            wr32(&mut buf, OFF_OWN_KEY, cb as u32);
            wr32(&mut buf, OFF_COMMENT_HEADER_KEY, new_lba as u32);
            finish_checksum(&mut buf);
            self.write(cb, &buf)?;
        }

        // The header itself: a full copy of the old bytes -- every field
        // this operation does not touch (name, dates, protection, owner,
        // hash_chain, byte_size, comment text) carries over verbatim --
        // with only what must reflect the new address patched in.
        // Nothing points at it yet: still a leak if interrupted here.
        let mut hdr = self.get(old_lba)?;
        wr32(&mut hdr, OFF_OWN_KEY, new_lba as u32);
        if let Some(ch) = &chain {
            let in_header = ch.blocks.len().min(slots);
            let first_data = if ffs {
                ch.blocks.first().copied().unwrap_or(0)
            } else {
                new_data.first().copied().unwrap_or(0) as u32
            };
            wr32(&mut hdr, OFF_FIRST_DATA, first_data);
            for (i, &old_d) in ch.blocks.iter().take(in_header).enumerate() {
                let v = if ffs { old_d } else { new_data[i] as u32 };
                wr32(&mut hdr, data_pointer_offset(bs, i as u32 + 1), v);
            }
            wr32(
                &mut hdr,
                tail(bs, TL_EXTENSION),
                new_ext.first().copied().unwrap_or(0) as u32,
            );
        }
        if !old_dircache.is_empty() {
            wr32(
                &mut hdr,
                tail(bs, TL_EXTENSION),
                new_dc.first().copied().unwrap_or(0) as u32,
            );
        }
        if let Some(cb) = new_cb {
            if variant.has_long_names() {
                wr32(&mut hdr, tail(bs, TL_COMMENT_BLOCK), cb as u32);
            }
        }
        finish_checksum(&mut hdr);
        self.write(new_lba, &hdr)?;

        // -- the commit: retarget whichever pointer names `old_lba` -----
        self.retarget_hash_chain(entry.parent as u64, &entry.name, old_lba, new_lba)?;

        // -- post-commit, soft -----------------------------------------
        let mut children_reparented = 0u64;
        if entry.kind == EntryKind::Directory {
            let table = self.vol.hash_table(new_lba)?;
            let mut visited: BTreeSet<u64> = BTreeSet::new();
            for &head in &table {
                let mut next = head;
                while next != 0 {
                    let clba = next as u64;
                    self.vol.guard_chain(&mut visited, clba)?;
                    let child = self.vol.entry_at(clba)?;
                    let mut buf = self.get(clba)?;
                    wr32(&mut buf, tail(bs, TL_PARENT), new_lba as u32);
                    self.put(clba, &mut buf)?;
                    children_reparented += 1;
                    next = child.hash_chain;
                }
            }
        }
        let mut links_repointed = 0u64;
        // `entry.next_link` means two different things depending on what
        // moved, per `read.rs`'s own documentation of the field: on the
        // *target* of a link chain it is the chain's head, and every link
        // reached from it must have its `real_entry` repointed at this
        // entry's new address. On a *link itself* it is instead the next
        // link after this one in the chain to the (unmoved, unrelated)
        // target -- a fact about that next link's position, not about
        // where this entry lives, so this entry moving must never touch
        // it. `entry.real_entry == 0` is exactly the target case (see
        // `parse_entry`: only a link block ever has a nonzero
        // `real_entry`), so it is the right guard here, not
        // `entry.next_link != 0` alone.
        if entry.real_entry == 0 {
            if entry.next_link != 0 {
                links_repointed += self.retarget_link_chain(entry.next_link as u64, new_lba)?;
            }
        } else {
            // The moved entry is itself a link: only its predecessor in
            // the chain (the target, or an earlier link) names it and
            // needs fixing up -- never `entry.next_link`, which names the
            // *next* link and does not care where this one now lives.
            self.retarget_link_predecessor(entry.real_entry as u64, old_lba, new_lba)?;
            links_repointed += 1;
        }

        self.refresh_dircache(entry.parent as u64)?;

        // -- free the old set, now unreachable ---------------------------
        if let Some(ch) = &chain {
            for &b in &ch.extensions {
                self.alloc.free(b as u64).map_err(MutateError::from)?;
            }
            if !ffs {
                for &b in &ch.blocks {
                    self.alloc.free(b as u64).map_err(MutateError::from)?;
                }
            }
        }
        for &b in &old_dircache {
            self.alloc.free(b).map_err(MutateError::from)?;
        }
        if has_comment {
            self.alloc
                .free(entry.comment_block as u64)
                .map_err(MutateError::from)?;
        }
        self.alloc.free(old_lba).map_err(MutateError::from)?;
        self.flush()?;

        // `bitmap_flag` back to valid, last of all -- the same "flag
        // last" rule `crate::repair` and `crate::resize` both use.
        self.alloc
            .mark_bitmap_valid(&mut self.vol.src, root_lba, variant)
            .map_err(MutateError::from)?;
        self.vol.reload_root()?;

        let dependents_relocated =
            (new_ext.len() + new_data.len() + new_dc.len() + usize::from(new_cb.is_some())) as u64;
        Ok(HeaderRelocateReport {
            old_lba,
            new_lba,
            dependents_relocated,
            children_reparented,
            links_repointed,
        })
    }

    /// Find whichever pointer currently names `old` in `dir`'s hash chain
    /// for `name` -- the directory's own slot, or the previous entry's
    /// hash-chain longword -- and replace it with `new`, preserving the
    /// entry's position in the chain. The commit: before this write
    /// `old` is authoritative and `new` is an unreachable full copy;
    /// after it, the reverse.
    ///
    /// The walk itself is [`Mutator::locate_in_hash_chain`], shared with
    /// [`Mutator::splice_from_hash_chain`] and [`Mutator::release`] --
    /// see that function's documentation for why the same "find the
    /// predecessor" logic serves a redirect here, a removal there and a
    /// membership check in `release`. There is no successor to re-read
    /// the way a splice has to: retargeting never removes a node from
    /// the chain, it only changes which address the predecessor names,
    /// so nothing about the chain's *shape* is derived from a value that
    /// could have gone stale.
    fn retarget_hash_chain(
        &mut self,
        dir: u64,
        name: &[u8],
        old: u64,
        new: u64,
    ) -> Result<(), MutateError<Transport<S>>> {
        let bs = self.bs();
        let slot = self.slot_of(name);
        let prev = self
            .locate_in_hash_chain(dir, slot, old)?
            .ok_or(MutateError::NotInChain { dir, lba: old })?;
        if prev == 0 {
            let mut buf = self.get(dir)?;
            wr32(&mut buf, OFF_HASH_TABLE + slot * 4, new as u32);
            self.put(dir, &mut buf)
        } else {
            let mut buf = self.get(prev)?;
            wr32(&mut buf, tail(bs, TL_HASH_CHAIN), new as u32);
            self.put(prev, &mut buf)
        }
    }

    /// Walk the chain of hard links naming a *target* whose header just
    /// moved (starting at the target's own longword −10, the same field
    /// [`Entry::next_link`] reads), and repoint every link's `real_entry`
    /// (longword −11) at the target's new number. Soft: unlike the
    /// hard-checked fields this module gives fresh copies, `real_entry`
    /// is only consulted when a link is actually resolved, so patching it
    /// in place carries no window in which an *unrelated* read fails.
    fn retarget_link_chain(
        &mut self,
        head: u64,
        new_target: u64,
    ) -> Result<u64, MutateError<Transport<S>>> {
        let bs = self.bs();
        let block_count = self.vol.block_count();
        let mut cur = head;
        let mut steps = 0u64;
        let mut count = 0u64;
        while cur != 0 {
            steps += 1;
            if steps > block_count {
                return Err(MutateError::Read(crate::read::Error::ChainTooLong {
                    lba: cur,
                }));
            }
            let mut buf = self.get(cur)?;
            wr32(&mut buf, tail(bs, TL_REAL_ENTRY), new_target as u32);
            let next_link = be32(&buf, tail(bs, TL_NEXT_LINK));
            self.put(cur, &mut buf)?;
            count += 1;
            cur = next_link as u64;
        }
        Ok(count)
    }

    /// The moved entry is itself a hard link: find whichever block (the
    /// target, or an earlier link) currently threads the chain through
    /// `old_link` via its own longword −10, and repoint that one longword
    /// at `new_link`.
    ///
    /// The walk is [`Mutator::locate_link_predecessor`], shared with
    /// [`Mutator::unlink_from_link_chain`] — see that function's
    /// documentation for why a redirect (this) needs no re-read of
    /// anything the way a removal's successor promotion does.
    fn retarget_link_predecessor(
        &mut self,
        target: u64,
        old_link: u64,
        new_link: u64,
    ) -> Result<(), MutateError<Transport<S>>> {
        let bs = self.bs();
        let prev = self.locate_link_predecessor(target, old_link)?;
        let mut buf = self.get(prev)?;
        wr32(&mut buf, tail(bs, TL_NEXT_LINK), new_link as u32);
        self.put(prev, &mut buf)
    }
}

// ---------------------------------------------------------------------------
// Whole-volume survey: every header, and who owns every dependent block
// ---------------------------------------------------------------------------

/// One pass over the reachable tree, recorded rather than merely walked —
/// [`Mutator::make_room`] and [`Mutator::compact`] both need "who owns
/// this block" answered many times, and a fresh directory-by-directory
/// walk per question would be the same O(volume) work paid repeatedly.
struct Survey {
    /// Every header found, in the order the walk reached it (root's
    /// direct children first, then theirs) — [`Mutator::compact`]'s
    /// "root outward" ordering.
    order: Vec<(u64, EntryKind)>,
    /// Header LBA -> its kind, for an O(log n) "is this LBA a header"
    /// check.
    headers: BTreeMap<u64, EntryKind>,
    /// A data block, extension block, dircache block or comment block's
    /// LBA -> the header LBA that owns it. Deliberately excludes the
    /// root's own dircache chain — relocating that is `crate::resize`'s
    /// job, on the same "root furniture" boundary this module's own
    /// documentation draws.
    owner_of: BTreeMap<u64, u64>,
}

impl<S: BlockMedium> Mutator<S> {
    fn survey(&mut self) -> Result<Survey, MutateError<Transport<S>>> {
        let root = self.vol.root_lba();
        let has_dircache = self.vol.variant().has_dircache();
        let mut order = Vec::new();
        let mut headers = BTreeMap::new();
        let mut owner_of = BTreeMap::new();
        let mut stack = vec![root];
        // A `BTreeSet`, not a `Vec`: a directory tree with many
        // directories would otherwise make this an O(n^2) scan the same
        // way `guard_chain`'s `Vec` did (see `read.rs`'s doc comment on
        // it) — one `.contains` per popped directory, each one scanning
        // everything seen so far.
        let mut visited: BTreeSet<u64> = BTreeSet::new();
        let block_count = self.vol.block_count();

        while let Some(dir) = stack.pop() {
            if visited.contains(&dir) {
                continue;
            }
            visited.insert(dir);
            if visited.len() as u64 > block_count {
                break; // a cycle a damaged volume already has; not this
                       // walk's job to diagnose further than stopping.
            }
            for e in self.vol.read_dir(dir)? {
                order.push((e.lba, e.kind));
                headers.insert(e.lba, e.kind);
                if e.comment_block != 0 {
                    owner_of.insert(e.comment_block as u64, e.lba);
                }
                match e.kind {
                    EntryKind::Directory => {
                        stack.push(e.lba);
                        if has_dircache {
                            for b in self.vol.read_dircache(e.lba)?.blocks {
                                owner_of.insert(b, e.lba);
                            }
                        }
                    }
                    EntryKind::File => {
                        let chain = self.vol.file_chain(e.lba)?;
                        for &b in &chain.blocks {
                            owner_of.insert(b as u64, e.lba);
                        }
                        for &b in &chain.extensions {
                            owner_of.insert(b as u64, e.lba);
                        }
                    }
                    EntryKind::LinkFile | EntryKind::LinkDir | EntryKind::SoftLink => {}
                }
            }
        }
        Ok(Survey {
            order,
            headers,
            owner_of,
        })
    }
}

// ---------------------------------------------------------------------------
// make_room: what resize's shrink actually needs
// ---------------------------------------------------------------------------

impl<S: BlockMedium> Mutator<S> {
    /// Evacuate every movable allocated block out of `range`. See this
    /// module's documentation for exactly what counts as movable (not the
    /// root, not a bitmap page or bitmap extension block — those are
    /// `crate::resize`'s own job) and how each kind of block found in the
    /// range is relocated.
    pub fn make_room(
        &mut self,
        range: Range<u64>,
    ) -> Result<MakeRoomReport, CompactError<Transport<S>>> {
        let root_lba = self.vol.root_lba();
        let bitmap = self.vol.read_bitmap()?;
        let before = range
            .clone()
            .filter(|&lba| self.alloc.is_allocated(lba) == Some(true))
            .count() as u64;

        let mut report = MakeRoomReport::default();
        let block_count = self.vol.block_count();
        let mut guard = 0u64;
        loop {
            guard += 1;
            if guard > block_count + 1 {
                // Every relocation this module performs frees the exact
                // block it was asked to move out of the range (or, for a
                // whole-file tier-1 move, several at once) -- this bound
                // exists only so a bug here fails loudly instead of
                // hanging, and should never actually bite.
                break;
            }
            let survey = self.survey()?;
            let mut target: Option<u64> = None;
            for lba in range.clone() {
                if self.alloc.is_allocated(lba) != Some(true) {
                    continue;
                }
                if lba == root_lba
                    || bitmap.pages().contains(&lba)
                    || bitmap.ext_blocks().contains(&lba)
                {
                    continue; // not this operation's to move.
                }
                target = Some(lba);
                break;
            }
            let lba = match target {
                Some(lba) => lba,
                None => break,
            };

            if let Some(&kind) = survey.headers.get(&lba) {
                let _ = kind;
                self.relocate_header_avoiding(lba, &range)?;
                report.headers_relocated += 1;
            } else if let Some(&owner) = survey.owner_of.get(&lba) {
                match survey.headers.get(&owner) {
                    Some(EntryKind::File) => {
                        self.defragment_file_avoiding(owner, Some(&range))?;
                        report.files_defragmented += 1;
                    }
                    _ => {
                        self.relocate_header_avoiding(owner, &range)?;
                        report.headers_relocated += 1;
                    }
                }
            } else {
                // Allocated, in range, not root furniture, and this
                // survey found no owner for it (an orphaned leak, or the
                // root's own dircache, deliberately excluded above) --
                // nothing here to relocate it through. Left in place;
                // the caller's own resize path, or a `repair`, is where
                // an orphan gets reclaimed.
                break;
            }
        }

        let after: Vec<u64> = range
            .filter(|&lba| self.alloc.is_allocated(lba) == Some(true))
            .collect();
        report.blocks_evacuated = before.saturating_sub(after.len() as u64);
        report.not_evacuated = after;
        Ok(report)
    }
}

// ---------------------------------------------------------------------------
// Full-volume compaction
// ---------------------------------------------------------------------------

impl<S: BlockMedium> Mutator<S> {
    /// Apply the block-layout policy retroactively: tier 1 (file-data
    /// contiguity) across every file by default, and tier 2 (header
    /// clustering) too when [`CompactOptions::relocate_headers`] is set.
    /// See this module's documentation for why tier 2 defaults off.
    pub fn compact(
        &mut self,
        options: &CompactOptions,
    ) -> Result<CompactReport, CompactError<Transport<S>>> {
        self.compact_with(options, |_| {})
    }

    /// [`Mutator::compact`], with a progress callback invoked once per
    /// relocation (or, in a dry run, once per relocation that *would*
    /// happen).
    pub fn compact_with<F: FnMut(CompactEvent)>(
        &mut self,
        options: &CompactOptions,
        mut progress: F,
    ) -> Result<CompactReport, CompactError<Transport<S>>> {
        let survey = self.survey()?;
        let mut report = CompactReport {
            dry_run: options.dry_run,
            ..Default::default()
        };

        for &(lba, kind) in &survey.order {
            if kind == EntryKind::File {
                report.files_examined += 1;
                if options.dry_run {
                    let entry = self.vol.entry_at(lba)?;
                    if entry.kind == EntryKind::File {
                        let chain = self.vol.file_chain(lba)?;
                        let seq = fetch_order(&chain, self.bs());
                        let before = count_runs(&seq.iter().map(|&(_, l)| l).collect::<Vec<_>>());
                        report.runs_before_total += before as u64;
                        report.runs_after_total += if before > 1 { 1 } else { before as u64 };
                        if before > 1 {
                            report.files_relocated += 1;
                        }
                    }
                } else {
                    let r = self.defragment_file(lba)?;
                    report.runs_before_total += r.runs_before as u64;
                    report.runs_after_total += r.runs_after as u64;
                    if r.blocks_relocated > 0 {
                        report.files_relocated += 1;
                    }
                    progress(CompactEvent::FileDefragged(r));
                }
            }
            if options.relocate_headers {
                if options.dry_run {
                    report.headers_relocated += 1;
                } else {
                    let r = self.relocate_header(lba)?;
                    report.headers_relocated += 1;
                    progress(CompactEvent::HeaderRelocated(r));
                }
            }
        }

        Ok(report)
    }
}
