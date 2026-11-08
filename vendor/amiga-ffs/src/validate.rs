//! `validate()`: walk the whole volume and report what does not add up.
//!
//! # Report, don't fail fast
//!
//! Every other entry point in this crate refuses on the first thing that
//! is wrong, because a caller asking for a file wants the file or an
//! explanation, not half a file. The validator is the opposite: it is the
//! tool you reach for *because* something is wrong, and stopping at the
//! first bad block would tell you the least useful true thing about the
//! volume. So [`Volume::validate`] returns a [`Report`] — a list of typed
//! [`Finding`]s, each carrying the block it is about — and keeps walking
//! everything still reachable after each one. A directory whose third
//! hash chain is corrupt still yields the other seventy-one.
//!
//! This is also why the findings are a different type from
//! [`Error`] rather than a `Vec` of them. Most of
//! what a validator finds is not a parse failure at all: an entry sitting
//! in the wrong hash slot parses perfectly and is simply unfindable, and
//! a block marked free while a file is using it is the most dangerous
//! state on the disk and involves no malformed block anywhere. Those get
//! named variants. Anything the ordinary reader *would* have refused is
//! kept whole in [`Finding::Unreadable`], typed error and all, rather
//! than flattened into a string.
//!
//! # What it walks
//!
//! From the root, every hash chain of every directory; from every file
//! header, its extension blocks and its data blocks; from every LNFS
//! entry with an overflow comment, its `T_COMMENT` block; from every
//! directory on a `DOS\4`/`DOS\5` volume, its dircache chain. Then the
//! bitmap: its pages, its extension blocks, and the two-way comparison
//! against everything the walk reached.
//!
//! FFS data blocks are counted reachable but not verified: they have no
//! checksum, no type and no header — there is nothing in one to check.
//! OFS data blocks have all three and are checked in full, which is the
//! whole reason OFS pays 24 bytes a block.
//!
//! # The two bitmap disagreements are not the same disagreement
//!
//! A block marked **allocated that nothing reaches** is a leak: space
//! lost until a validator frees it, and otherwise harmless. A block
//! marked **free that something reaches** is the dangerous one: the next
//! allocation will hand it out, and then two files own it. The format has
//! no journal, so the only defence is that mutations are ordered to fail
//! in the first direction rather than the second — and the only way to
//! know they did is to report the two separately, which
//! [`Finding::OrphanBlock`] and [`Finding::ReachableButFree`] do.

use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

use crate::dircache::DircacheRecord;
use crate::read::{Entry, EntryKind, Error, Volume};
use crate::{hash_table_size, name_hash, names_equal, BlockSource};

/// How many findings a report will hold before it stops recording them.
///
/// A volume of random bytes can produce a finding per block, and a
/// validator that allocates one struct per block on a 2 GB volume is a
/// worse failure than the volume's. The count is still exact —
/// [`Report::truncated`] says the list is not.
pub const MAX_FINDINGS: usize = 4096;

/// One thing wrong with the volume, and where.
///
/// Generic over the block source's error for the same reason
/// [`Error`] is: "why could this block not be read"
/// is a question only the transport can answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding<E> {
    /// A block the ordinary reader refused, with its refusal kept whole:
    /// a bad checksum, a wrong block type, a pointer out of range, a file
    /// whose length and extent disagree. The walk records it and carries
    /// on with whatever else is reachable.
    Unreadable {
        /// The block the walk was working on.
        lba: u64,
        /// Exactly what the reader said.
        error: Error<E>,
    },
    /// A block whose longwords do not sum to zero. Broken out of
    /// [`Finding::Unreadable`] because it is the commonest finding by a
    /// wide margin and the one most worth counting.
    Checksum {
        /// The block that failed.
        lba: u64,
    },
    /// A block whose longword 1 does not name itself.
    OwnKeyMismatch {
        /// The block.
        lba: u64,
        /// The own key it holds.
        found: u32,
    },
    /// An entry whose parent pointer is not the directory it was found
    /// in. Reachable, but `Parent()` on it lands somewhere else — which
    /// is how a path that resolves downward fails to resolve back up.
    ParentMismatch {
        /// The entry's header block.
        lba: u64,
        /// The parent it claims.
        found: u32,
        /// The directory that actually holds it.
        expected: u64,
    },
    /// An entry chained into a hash slot other than the one its name
    /// hashes to.
    ///
    /// The entry is in the directory and enumeration finds it; *lookup by
    /// name* never will, because lookup hashes the name and walks one
    /// chain. Files that plainly exist and cannot be opened are this,
    /// and so — much worse — is a directory in which the same name can
    /// be created twice.
    WrongChainSlot {
        /// The entry's header block.
        lba: u64,
        /// The directory holding it.
        dir: u64,
        /// The slot it was found in.
        found: u32,
        /// The slot its name hashes to under this volume's fold table.
        hashes_to: u32,
    },
    /// A directory entry with a zero-length name.
    ///
    /// The filesystem cannot create one. Finding one means either real
    /// damage or — the reason this has its own variant — a volume written
    /// by something that misread the layout, which for `DOS\6`/`DOS\7` is
    /// the signature failure this crate exists to not have.
    EmptyName {
        /// The entry's header block.
        lba: u64,
    },
    /// A name longer than the variant can store, which no lookup will
    /// ever match because no caller can offer a name that long.
    NameTooLong {
        /// The entry's header block.
        lba: u64,
        /// The length found.
        len: usize,
        /// The variant's maximum.
        max: usize,
    },
    /// A block reached twice from two different places. Whichever
    /// reference is wrong, one of them is writing over the other.
    DoublyReachable {
        /// The block reached again.
        lba: u64,
        /// The block that pointed at it the second time.
        from: u64,
    },
    /// A hard link whose target block is zero: the shape an interrupted
    /// delete leaves.
    LinkTargetMissing {
        /// The link's header block.
        lba: u64,
    },
    /// The root's `bitmap_flag` is 0: the volume was not unmounted
    /// cleanly and the bitmap on disk is mid-update.
    ///
    /// Everything else about the bitmap is still reported, but the
    /// allocated/free comparison is **skipped** — an untrusted bitmap
    /// compared against a trustworthy walk produces a finding per
    /// disagreement and not one of them means anything.
    BitmapInvalid,
    /// The bitmap's pages do not have a bit for every block in the
    /// volume. Not corruption: the blocks past the end simply cannot be
    /// allocated. Worth saying, because a volume that quietly lost its
    /// last few megabytes looks exactly like this.
    BitmapIncomplete {
        /// Blocks the bitmap has bits for.
        covered: u64,
        /// Blocks in the volume.
        block_count: u64,
    },
    /// Marked allocated, reached by nothing: leaked space. Recoverable
    /// by rewriting the bitmap, and harmless until then.
    OrphanBlock {
        /// The leaked block.
        lba: u64,
    },
    /// Reached from the root, marked **free**: the next allocation will
    /// hand this block to a second owner. The dangerous direction.
    ReachableButFree {
        /// The block at risk.
        lba: u64,
    },
    /// The dircache and the hash chains disagree about the directory.
    ///
    /// Always a finding against the *cache*: the chains are the
    /// filesystem, and this crate never resolves a name through a cache.
    DircacheStale {
        /// The directory.
        dir: u64,
        /// The cache block the record came from, or the directory itself
        /// for a record that is missing entirely.
        block: u64,
        /// What specifically disagrees.
        detail: DircacheDiscrepancy,
    },
}

/// The ways a dircache can be wrong about its directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DircacheDiscrepancy {
    /// An entry is in the hash chains and has no record in the cache. A
    /// `List` served from this cache would not show the file.
    Missing {
        /// The entry's header block.
        entry: u64,
    },
    /// A record names a block that is not in the directory's chains. A
    /// `List` served from this cache shows a file that is not there —
    /// and, if the block has since been reused, shows it with somebody
    /// else's contents.
    Extra {
        /// The block the record names.
        entry: u32,
    },
    /// Two records for the same entry.
    Duplicate {
        /// The block named twice.
        entry: u32,
    },
    /// The record and the entry disagree about the name. Under the
    /// volume's own fold table, so a case difference alone is not a
    /// finding — the filesystem considers those the same name.
    NameMismatch {
        /// The entry's header block.
        entry: u64,
    },
    /// The record and the entry disagree about the file's length.
    SizeMismatch {
        /// The entry's header block.
        entry: u64,
        /// What the cache says.
        cached: u32,
        /// What the header block says.
        actual: u32,
    },
    /// The record and the entry disagree about what kind of thing it is.
    ///
    /// A cached type of 0 is *not* reported: it is not a valid secondary
    /// type, and some writers (xdftool among them) lay down the whole
    /// record and never fill the byte in. Treating "not recorded" as
    /// "wrong" would flag every entry of every volume those tools make.
    TypeMismatch {
        /// The entry's header block.
        entry: u64,
        /// The secondary type the cache narrowed into a byte.
        cached: i8,
        /// The secondary type the header block carries.
        actual: i32,
    },
}

/// What the walk counted. Every field is a fact about blocks actually
/// reached, not about what the volume's own records claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Summary {
    /// Directories reached, the root included.
    pub directories: u64,
    /// File header blocks reached.
    pub files: u64,
    /// Hard links (`ST_LINKFILE`/`ST_LINKDIR`) reached.
    pub hard_links: u64,
    /// Soft links reached.
    pub soft_links: u64,
    /// `T_LIST` file-extension blocks reached.
    pub extension_blocks: u64,
    /// Data blocks reached, OFS and FFS alike.
    pub data_blocks: u64,
    /// `T_DIRCACHE` blocks reached.
    pub dircache_blocks: u64,
    /// `T_COMMENT` overflow-comment blocks reached.
    pub comment_blocks: u64,
    /// Bitmap pages and bitmap extension blocks.
    pub bitmap_blocks: u64,
    /// Distinct blocks reached from the root, all kinds together.
    pub reachable: u64,
    /// Blocks the bitmap marks allocated.
    pub allocated: u64,
    /// Blocks the bitmap marks free.
    pub free: u64,
    /// Allocated-but-unreachable blocks: leaks.
    pub orphans: u64,
    /// Reachable-but-free blocks: double-allocation risks.
    pub reachable_but_free: u64,
}

/// The result of a walk: what was found, and what was counted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report<E> {
    /// Everything wrong, in the order it was found.
    pub findings: Vec<Finding<E>>,
    /// The findings list hit [`MAX_FINDINGS`] and stopped growing. The
    /// counts in [`Report::summary`] are still complete.
    pub truncated: bool,
    /// What the walk counted.
    pub summary: Summary,
}

impl<E> Default for Report<E> {
    fn default() -> Self {
        Self {
            findings: Vec::new(),
            truncated: false,
            summary: Summary::default(),
        }
    }
}

impl<E> Report<E> {
    /// Nothing wrong at all.
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty() && !self.truncated
    }

    fn push(&mut self, finding: Finding<E>) {
        if self.findings.len() < MAX_FINDINGS {
            self.findings.push(finding);
        } else {
            self.truncated = true;
        }
    }
}

/// One bit per block, for "has the walk reached this?".
///
/// A `Vec<u32>` rather than a set of LBAs on purpose: the walk touches
/// every allocated block of the volume, so a set would grow to the same
/// size with a hash table's constant on top. At 2 GB and 512-byte blocks
/// this is 512 KB, which is the memory bound the validator is willing to
/// spend.
pub(crate) struct Reached {
    words: Vec<u32>,
    pub(crate) count: u64,
}

impl Reached {
    pub(crate) fn new(block_count: u64) -> Self {
        Self {
            words: vec![0u32; (block_count / 32 + 1) as usize],
            count: 0,
        }
    }

    /// Mark `lba`; false if it was already marked.
    fn set(&mut self, lba: u64) -> bool {
        let (w, b) = ((lba / 32) as usize, lba % 32);
        match self.words.get_mut(w) {
            Some(word) if *word >> b & 1 == 0 => {
                *word |= 1 << b;
                self.count += 1;
                true
            }
            _ => false,
        }
    }

    pub(crate) fn get(&self, lba: u64) -> bool {
        let (w, b) = ((lba / 32) as usize, lba % 32);
        self.words.get(w).map_or(false, |word| word >> b & 1 == 1)
    }
}

impl<S: BlockSource> Volume<S> {
    /// Walk the volume from the root and report everything that does not
    /// add up.
    ///
    /// Reads every reachable metadata block once (plus every OFS data
    /// block, and every FFS data block's *pointer* but not its contents),
    /// so the cost is proportional to the volume's used space. It never
    /// errors: an unreadable block is a finding, and the walk continues
    /// with whatever else is reachable.
    pub fn validate(&mut self) -> Report<S::Error> {
        let mut report = Report::default();
        let mut reached = self.walk_reachable(&mut report);
        self.validate_bitmap(&mut reached, &mut report);
        report.summary.reachable = reached.count;
        report
    }

    /// The tree walk alone: everything reachable from the root, recorded
    /// in a bitset, with what did not add up appended to `report`.
    ///
    /// Split out of [`Volume::validate`] because
    /// [`repair`](crate::repair) needs the identical walk and must not
    /// have a second copy of it: the set of blocks a repair marks
    /// allocated is *defined* as the set this returns, and a rebuild
    /// computed from a slightly different walk than the validator's would
    /// produce a volume that validates worse than it started. The bitmap
    /// is deliberately not touched here — a repair is about to replace
    /// it, and the validator compares against it afterwards.
    pub(crate) fn walk_reachable(&mut self, report: &mut Report<S::Error>) -> Reached {
        let block_count = self.block_count();
        let mut reached = Reached::new(block_count);

        let root = self.root_lba();
        reached.set(root);
        report.summary.directories += 1;

        // Depth-first, explicit stack: a deep directory tree is a real
        // shape and recursion here would put the volume's shape in charge
        // of this process's stack depth.
        let mut queue = vec![root];
        while let Some(dir) = queue.pop() {
            self.validate_dir(dir, &mut reached, report, &mut queue);
        }
        reached
    }

    fn validate_dir(
        &mut self,
        dir: u64,
        reached: &mut Reached,
        report: &mut Report<S::Error>,
        queue: &mut Vec<u64>,
    ) {
        let bs = self.block_size();
        let slots = hash_table_size(bs);
        let fold = self.variant().fold();
        let max_name = self.max_name_len();

        let table = match self.hash_table(dir) {
            Ok(t) => t,
            Err(e) => {
                record(report, dir, e);
                return;
            }
        };

        // Kept for the dircache comparison: the chains are the truth the
        // cache is checked against, so they have to be collected first.
        let mut entries: Vec<Entry> = Vec::new();

        for (slot, head) in table.into_iter().enumerate() {
            let mut next = head;
            // One visited list per directory would be the other choice;
            // per chain plus the volume-wide `reached` bitmap catches
            // both a chain that loops and a block in two chains, without
            // a second allocation the size of the directory.
            let mut visited: Vec<u64> = Vec::new();
            while next != 0 {
                let lba = next as u64;
                if visited.contains(&lba) {
                    report.push(Finding::Unreadable {
                        lba,
                        error: Error::ChainCycle { lba },
                    });
                    break;
                }
                visited.push(lba);

                let entry = match self.entry_at(lba) {
                    Ok(e) => e,
                    Err(e) => {
                        record(report, lba, e);
                        break;
                    }
                };
                next = entry.hash_chain;

                if !reached.set(lba) {
                    report.push(Finding::DoublyReachable { lba, from: dir });
                    continue;
                }

                if entry.own_key as u64 != lba {
                    report.push(Finding::OwnKeyMismatch {
                        lba,
                        found: entry.own_key,
                    });
                }
                if entry.parent as u64 != dir {
                    report.push(Finding::ParentMismatch {
                        lba,
                        found: entry.parent,
                        expected: dir,
                    });
                }
                if entry.name.is_empty() {
                    report.push(Finding::EmptyName { lba });
                } else if entry.name.len() > max_name {
                    report.push(Finding::NameTooLong {
                        lba,
                        len: entry.name.len(),
                        max: max_name,
                    });
                } else {
                    let hashes_to = name_hash(&entry.name, fold, slots);
                    if hashes_to != slot as u32 {
                        report.push(Finding::WrongChainSlot {
                            lba,
                            dir,
                            found: slot as u32,
                            hashes_to,
                        });
                    }
                }

                self.validate_entry(&entry, reached, report, queue);
                entries.push(entry);
            }
        }

        self.validate_dircache(dir, &entries, reached, report);
    }

    /// Everything hanging off one entry: its content blocks, its overflow
    /// comment, and — for a directory — a place in the queue.
    fn validate_entry(
        &mut self,
        entry: &Entry,
        reached: &mut Reached,
        report: &mut Report<S::Error>,
        queue: &mut Vec<u64>,
    ) {
        match entry.kind {
            EntryKind::Directory => {
                report.summary.directories += 1;
                queue.push(entry.lba);
            }
            EntryKind::File => {
                report.summary.files += 1;
                self.validate_file(entry.lba, reached, report);
            }
            EntryKind::SoftLink => report.summary.soft_links += 1,
            EntryKind::LinkFile | EntryKind::LinkDir => {
                report.summary.hard_links += 1;
                if entry.real_entry == 0 {
                    report.push(Finding::LinkTargetMissing { lba: entry.lba });
                }
                // The target is not marked reached from here: it has its
                // own directory entry, and marking it twice would report
                // every hard link as a double reference.
            }
        }

        if entry.comment_block != 0 {
            let lba = entry.comment_block as u64;
            if mark(reached, report, lba, entry.lba, self.block_count()) {
                report.summary.comment_blocks += 1;
                // `comment` verifies the block's type, own key and the
                // header it was written for; the text is not our business.
                if let Err(e) = self.comment(entry) {
                    record(report, lba, e);
                }
            }
        }
    }

    fn validate_file(&mut self, lba: u64, reached: &mut Reached, report: &mut Report<S::Error>) {
        let chain = match self.file_chain(lba) {
            Ok(c) => c,
            Err(e) => {
                record(report, lba, e);
                return;
            }
        };
        let block_count = self.block_count();
        for &ext in &chain.extensions {
            if mark(reached, report, ext as u64, lba, block_count) {
                report.summary.extension_blocks += 1;
            }
        }
        for &data in &chain.blocks {
            if mark(reached, report, data as u64, lba, block_count) {
                report.summary.data_blocks += 1;
            }
        }
        // FFS data blocks are raw payload: no type, no owner, no
        // sequence, no checksum. There is nothing in one to verify, so
        // they are counted and left alone. OFS blocks carry all four,
        // and `read_chain_with` checks every one of them.
        if !self.variant().is_ffs() {
            if let Err(e) = self.read_chain_with(&chain, |_| {}) {
                record(report, lba, e);
            }
        }
    }

    /// Compare a directory's dircache against the chains just walked.
    ///
    /// Nothing here trusts the cache. Every finding is against the cache
    /// and names the entry the chains actually hold, so a caller acting
    /// on the report rebuilds the cache from the chains rather than the
    /// other way round.
    fn validate_dircache(
        &mut self,
        dir: u64,
        entries: &[Entry],
        reached: &mut Reached,
        report: &mut Report<S::Error>,
    ) {
        if !self.variant().has_dircache() {
            return;
        }
        let cache = match self.read_dircache(dir) {
            Ok(c) => c,
            Err(e) => {
                record(report, dir, e);
                return;
            }
        };
        let block_count = self.block_count();
        for &b in &cache.blocks {
            if mark(reached, report, b, dir, block_count) {
                report.summary.dircache_blocks += 1;
            }
        }

        let fold = self.variant().fold();
        let mut covered = vec![false; entries.len()];
        for record_ in &cache.records {
            let i = match entries.iter().position(|e| e.lba == record_.entry as u64) {
                Some(i) => i,
                None => {
                    report.push(Finding::DircacheStale {
                        dir,
                        block: record_.block,
                        detail: DircacheDiscrepancy::Extra {
                            entry: record_.entry,
                        },
                    });
                    continue;
                }
            };
            if covered[i] {
                report.push(Finding::DircacheStale {
                    dir,
                    block: record_.block,
                    detail: DircacheDiscrepancy::Duplicate {
                        entry: record_.entry,
                    },
                });
                continue;
            }
            covered[i] = true;
            compare_record(dir, record_, &entries[i], fold, report);
        }

        for (i, done) in covered.iter().enumerate() {
            if !done {
                report.push(Finding::DircacheStale {
                    dir,
                    block: dir,
                    detail: DircacheDiscrepancy::Missing {
                        entry: entries[i].lba,
                    },
                });
            }
        }
    }

    fn validate_bitmap(&mut self, reached: &mut Reached, report: &mut Report<S::Error>) {
        let root = self.root_lba();
        let bitmap = match self.read_bitmap() {
            Ok(b) => b,
            Err(e) => {
                record(report, root, e);
                return;
            }
        };
        let block_count = self.block_count();
        for &b in bitmap.pages().iter().chain(bitmap.ext_blocks()) {
            if mark(reached, report, b, root, block_count) {
                report.summary.bitmap_blocks += 1;
            }
        }
        // A page's own checksum failing is a fact about that page alone:
        // its block range is unknown (`Bitmap::covers` already excludes
        // it from every comparison below), but every other page's bits
        // are still compared against the walk normally.
        for &lba in bitmap.bad_pages() {
            report.push(Finding::Checksum { lba });
        }
        report.summary.allocated = bitmap.allocated_count();
        report.summary.free = bitmap.free_count();

        if !bitmap.covers_whole_volume() {
            report.push(Finding::BitmapIncomplete {
                covered: bitmap.covered_count(),
                block_count,
            });
        }
        if !bitmap.valid() {
            // Reported, and then the comparison is skipped: every
            // disagreement with an untrusted bitmap is noise.
            report.push(Finding::BitmapInvalid);
            return;
        }

        for lba in bitmap.covered() {
            match (bitmap.is_allocated(lba), reached.get(lba)) {
                (Some(true), false) => {
                    report.summary.orphans += 1;
                    report.push(Finding::OrphanBlock { lba });
                }
                (Some(false), true) => {
                    report.summary.reachable_but_free += 1;
                    report.push(Finding::ReachableButFree { lba });
                }
                _ => {}
            }
        }
    }
}

/// Compare one cache record against the entry it claims to describe.
fn compare_record<E>(
    dir: u64,
    cached: &DircacheRecord,
    entry: &Entry,
    fold: fn(u8) -> u8,
    report: &mut Report<E>,
) {
    let stale = |detail| Finding::DircacheStale {
        dir,
        block: cached.block,
        detail,
    };
    // Under the volume's own fold table: the filesystem considers `Foo`
    // and `FOO` the same name, so a case difference between the cache and
    // the header block is not a disagreement about *which* entry it is.
    if !names_equal(&cached.name, &entry.name, fold) {
        report.push(stale(DircacheDiscrepancy::NameMismatch {
            entry: entry.lba,
        }));
    }
    if cached.size != entry.byte_size {
        report.push(stale(DircacheDiscrepancy::SizeMismatch {
            entry: entry.lba,
            cached: cached.size,
            actual: entry.byte_size,
        }));
    }
    let actual = entry.kind.secondary_type();
    if cached.entry_type != 0 && cached.entry_type as i32 != actual {
        report.push(stale(DircacheDiscrepancy::TypeMismatch {
            entry: entry.lba,
            cached: cached.entry_type,
            actual,
        }));
    }
}

/// Mark a block reached, reporting the two ways that can fail: a pointer
/// outside the volume, and a block something else already claimed.
fn mark<E>(
    reached: &mut Reached,
    report: &mut Report<E>,
    lba: u64,
    from: u64,
    block_count: u64,
) -> bool {
    if lba >= block_count {
        report.push(Finding::Unreadable {
            lba: from,
            error: Error::LbaOutOfRange { lba, block_count },
        });
        return false;
    }
    if !reached.set(lba) {
        report.push(Finding::DoublyReachable { lba, from });
        return false;
    }
    true
}

/// File a read-side refusal, lifting the one that has its own variant.
fn record<E>(report: &mut Report<E>, lba: u64, error: Error<E>) {
    match error {
        Error::Checksum { lba } => report.push(Finding::Checksum { lba }),
        Error::OwnKeyMismatch { lba, found } => report.push(Finding::OwnKeyMismatch { lba, found }),
        error => report.push(Finding::Unreadable { lba, error }),
    }
}

impl<E: fmt::Display> fmt::Display for Finding<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { lba, error } => write!(f, "block {lba}: {error}"),
            Self::Checksum { lba } => write!(f, "block {lba}: bad checksum"),
            Self::OwnKeyMismatch { lba, found } => {
                write!(f, "block {lba}: calls itself block {found}")
            }
            Self::ParentMismatch {
                lba,
                found,
                expected,
            } => write!(
                f,
                "block {lba}: parent is {found}, but block {expected} holds it"
            ),
            Self::WrongChainSlot {
                lba,
                dir,
                found,
                hashes_to,
            } => write!(
                f,
                "block {lba}: in slot {found} of directory {dir}, but its name hashes to {hashes_to} \
                 -- enumeration finds it, lookup never will"
            ),
            Self::EmptyName { lba } => write!(f, "block {lba}: empty name"),
            Self::NameTooLong { lba, len, max } => {
                write!(f, "block {lba}: name of {len} bytes exceeds {max}")
            }
            Self::DoublyReachable { lba, from } => {
                write!(f, "block {lba}: reached a second time, from block {from}")
            }
            Self::LinkTargetMissing { lba } => write!(f, "block {lba}: hard link to nothing"),
            Self::BitmapInvalid => f.write_str(
                "the root's bitmap flag is 0: the bitmap is mid-update and must not be trusted",
            ),
            Self::BitmapIncomplete {
                covered,
                block_count,
            } => write!(
                f,
                "the bitmap covers {covered} of {block_count} blocks; the rest can never be allocated"
            ),
            Self::OrphanBlock { lba } => {
                write!(f, "block {lba}: allocated but unreachable (leaked)")
            }
            Self::ReachableButFree { lba } => write!(
                f,
                "block {lba}: in use but marked free -- the next allocation will hand it out twice"
            ),
            Self::DircacheStale { dir, block, detail } => {
                write!(f, "dircache block {block} of directory {dir}: {detail}")
            }
        }
    }
}

impl fmt::Display for DircacheDiscrepancy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { entry } => write!(f, "entry {entry} is not cached"),
            Self::Extra { entry } => write!(f, "caches entry {entry}, which is not in the chains"),
            Self::Duplicate { entry } => write!(f, "caches entry {entry} twice"),
            Self::NameMismatch { entry } => write!(f, "entry {entry} is cached under another name"),
            Self::SizeMismatch {
                entry,
                cached,
                actual,
            } => write!(f, "entry {entry} is {actual} bytes, cached as {cached}"),
            Self::TypeMismatch {
                entry,
                cached,
                actual,
            } => write!(f, "entry {entry} is type {actual}, cached as {cached}"),
        }
    }
}
