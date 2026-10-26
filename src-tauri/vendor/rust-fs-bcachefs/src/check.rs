//! A read-only checker: what a filesystem's own structures say about each
//! other, without repairing anything.
//!
//! Checked: the superblock and its checksum; every btree this reader knows,
//! node by node (magic, sequence, checksums, key order, keys inside each
//! node's range); a size, a snapshot, a position or value lengths a key's
//! btree does not allow; every directory entry against the inode it names (it
//! exists, its type agrees, its directory is a directory); every inode's
//! link count against the entries that name it; every extent against its
//! inode (it exists, the extent does not start past the end of the file,
//! it does not overlap the extent before it); every data extent's checksum;
//! that every key is at a snapshot the snapshots btree has; every snapshot
//! tree against its master subvolume, every snapshot against its tree's
//! root and its skiplist against its ancestors; every xattr
//! against its inode (it exists, the xattr is reachable from its name's
//! hash slot); every inode's back-reference to the entry naming it; every
//! inode's has-options flag against its option fields; every directory's
//! size (0), every inode's journal sequence against the journal's newest,
//! and every inode's sector count against what its extents add up to. Entries, inodes
//! and link counts are judged as each subvolume sees them through its
//! snapshot's chain, and an entry naming a subvolume against the
//! subvolumes btree. Every reflinked extent is followed into the reflink
//! btree: its range is covered, each indirect extent's refcount is the
//! number of pointers into it, and its data is checked as an extent's is.
//! A filesystem not shut down cleanly is checked through the replay of its
//! journal, as it would be read.
//!
//! Of the allocation btrees, each bucket's generation in bucket_gens is
//! checked against its alloc key, each data extent has its backpointer with
//! matching value (a piece of a compressed region aside), each backpointer
//! is in a bucket with an alloc key, each bucket holding data is a user
//! bucket in its alloc key, the freespace btree holds exactly the buckets
//! with no alloc key or a free one,
//! the fragmentation lru holds an entry for each partly filled bucket and
//! nothing else, the need_discard btree a key for each bucket waiting for
//! discard and nothing else, and the device's accounting by data type is
//! what its alloc keys add up to. These are made on a filesystem shut down
//! cleanly only: after a crash the allocation btrees can lag the rest, as
//! the reference leaves them, and the reference rebuilds them in recovery.
//!
//! It is a subset of what the reference checker checks (the rest of
//! allocation, accounting, backpointers, freespace and lru are not), and
//! the tests hold it to that: wherever it reports a problem, so does the
//! reference (tests in the guest), and on every clean fixture it reports
//! none.

use std::collections::BTreeMap;

use crate::bkey::{key_type, Bkey, Bpos};
use crate::btree::{self, btree_id};
use crate::error::{Error, Result};
use crate::extent::DataExtent;
use crate::inode::{Dirent, Inode, InodeV3Raw, INODE_HAS_OPTIONS, OPTION_FIELDS, ROOT_INO};
use crate::journal::{self, Replay};
use crate::superblock::Superblock;
use crate::util::{le32, le64};
use fs_core::BlockRead;

/// The snapshots and subvolumes btrees (S1 11.3) and their key types (S1
/// 11.5), as the reader reads them (src/fs.rs).
const SUBVOLUMES: u8 = 8;
const SNAPSHOTS: u8 = 9;
const KEY_SUBVOLUME: u8 = 21;
const KEY_SNAPSHOT: u8 = 22;
/// The snapshot_trees btree and its key type, at `0:tree`: the master
/// subvolume in the low half of the value's word, the root snapshot in the
/// high half. In a snapshot's value, `tree` is the high half of the third
/// word, and the three skiplist entries follow the depth, from the high
/// half of the fourth word (S3 + S4: the lister beside the key bytes of
/// the kernel image, docs/clean-room.md).
const SNAPSHOT_TREES: u8 = 15;
const KEY_SNAPSHOT_TREE: u8 = 31;
const TREE_ROOT: usize = 4;
const SNAP_TREE: usize = 20;
const SNAP_SKIPLIST: usize = 28;
/// The bucket_gens btree, and its key type: 256 buckets' generations a
/// key, one byte each, at `dev:bucket >> 8` (S8).
const BUCKET_GENS: u8 = 14;
const KEY_BUCKET_GENS: u8 = 30;
/// The backpointers btree, and its key type: a data pointer's backpointer
/// sits at `dev:sector << 16` (S8).
const BACKPOINTERS: u8 = 13;
const KEY_BACKPOINTER: u8 = 28;
/// An alloc_v4 key's data type, byte 6 of its second word, and the type of
/// a bucket holding file data (docs/clean-room.md, "Allocating space").
const ALLOC_DATA_TYPE: usize = 14;
const DATA_BTREE: u8 = 3;
const DATA_USER: u8 = 4;
/// The lru btree, and the key type of its entries: a partly filled bucket's
/// fragmentation entry sits at `(1 << 61 | dirty * 2^31 / bucket_size) :
/// bucket`, the top three bits of the inode field naming the lru (S8).
const LRU: u8 = 10;
const KEY_SET: u8 = 25;
const LRU_FRAGMENTATION: u64 = 1;
/// The need_discard btree, and the data type of a bucket waiting for
/// discard: its key sits at `journal_seq_empty : bucket`, the sequence
/// number word 6 of the bucket's alloc key (S8; every need_discard bucket
/// of the reference's fixtures has its key there).
const NEED_DISCARD: u8 = 12;
const DATA_NEED_DISCARD: u8 = 9;
/// The freespace btree: a run of free buckets is a `set` key at
/// `dev:end`, `size` buckets long (docs/clean-room.md, "Allocating
/// space"). Whether a run's position carries a bucket's generation once it
/// reaches 16 is unobserved, so free space is not judged past that.
const FREESPACE: u8 = 11;
const FREESPACE_GEN_UNOBSERVED: u8 = 16;
/// The accounting btree, its key type, and the kind of key that accounts
/// for a device's buckets of one data type: the position's first byte is
/// the kind, then the device and the data type, byte by byte; the value
/// is buckets, sectors, fragmented (S8).
const ACCOUNTING: u8 = 20;
const KEY_ACCOUNTING: u8 = 34;
const ACCT_DEV_DATA_TYPE: u64 = 3;
/// The logged_ops and reconcile_work_phys btrees.
const LOGGED_OPS: u8 = 17;
const RECONCILE_WORK_PHYS: u8 = 24;
const KEY_COOKIE: u8 = 3;
/// The key types the reference lets every btree hold: deleted, whiteout,
/// error and cookie. It judges a key's type before its value size (an
/// inode_generation key with no value is refused for its type in
/// reconcile_work_phys, for its value size in inodes), and of the last
/// three it refuses none for its type in any btree, only for its value
/// size, size or snapshot (#163, run 39020187551).
const KEY_TYPES_ANYWHERE: [u8; 4] = [
    key_type::DELETED,
    key_type::WHITEOUT,
    key_type::ERROR,
    KEY_COOKIE,
];
/// The key types a btree may hold beyond those. The reference has refused
/// logged_op_truncate and lru in freespace (#257, #264), lru in xattrs
/// (#259), and inode_v3 in logged_ops (#263); logged_ops holds only its
/// logged operations and the inode allocation cursor, accounting holds
/// accounting keys, and those three btrees hold set keys.
/// Run 39037096192 also reports no type refusal for unnamed type 40 in
/// these three btrees; no other unnamed type has been judged.
/// Every other btree is #305.
const KEY_TYPES_ALLOWED: &[(u8, &[u8])] = &[
    (btree_id::XATTRS, &[key_type::XATTR]),
    (FREESPACE, &[KEY_SET]),
    (LOGGED_OPS, &[KEY_SET, 32, 33, 35, 37, 39, 40]),
    (ACCOUNTING, &[KEY_SET, KEY_ACCOUNTING, 40]),
    (RECONCILE_WORK_PHYS, &[KEY_SET, 40]),
];
/// A dirent's type when it names a subvolume (the lister's `type subvol`).
const DT_SUBVOL: u8 = 16;
/// Deeper than any snapshot tree; a parent cycle stops here.
const MAX_SNAPSHOT_DEPTH: usize = 1024;
/// The superblock's clean field: flags u32, two u16 clocks, then the
/// journal sequence of the clean shutdown (src/superblock.rs).
const FIELD_CLEAN: u32 = 6;
/// The key types of the extents btree whose size an inode's sector count
/// adds up: extent, reservation, reflink_p, inline_data (S3: the lister's
/// `bi_sectors` against its extents, for every inode of every listed
/// fixture, #221).
const SECTOR_KEYS: [u8; 4] = [
    key_type::EXTENT,
    key_type::RESERVATION,
    key_type::REFLINK_P,
    key_type::INLINE_DATA,
];
/// How far past the newest journal sequence written an inode's may be
/// before it is in the future. The reference compares with the sequence
/// its own journal is at while it checks: after an unclean shutdown it
/// skips 64 past the last entry read (S1 1.3), and its own run advanced at
/// most 6 more on any fixture (S3: each fixture's `fsck` record, from
/// "recovering from ... journal seq N" to "clean shutdown complete").
/// Beyond 128 past, no journal of the reference's is there yet.
const JOURNAL_SEQ_SLACK: u64 = 128;

/// One thing found wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// A short, stable name for the kind of problem.
    pub kind: &'static str,
    /// What and where.
    pub detail: String,
}

/// What the checker found.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub problems: Vec<Problem>,
    /// Inodes, directory entries and extents looked at.
    pub inodes: u64,
    pub dirents: u64,
    pub extents: u64,
    /// Whether the journal was replayed to check the filesystem.
    pub replayed: bool,
}

impl Report {
    pub fn clean(&self) -> bool {
        self.problems.is_empty()
    }

    fn add(&mut self, kind: &'static str, detail: impl Into<String>) {
        self.problems.push(Problem {
            kind,
            detail: detail.into(),
        });
    }
}

/// Check the filesystem on `dev`. An `Err` is a filesystem this checker
/// cannot check at all (not bcachefs, encrypted, several devices); every
/// problem it can name is in the report.
pub fn check(dev: &dyn BlockRead) -> Result<Report> {
    let mut r = Report::default();
    let sb = match Superblock::read_copies(dev) {
        Ok((sb, failed)) => {
            // A copy that does not read is damage even when another copy
            // does: the next torn write has one fewer to fall back on.
            for (sector, e) in failed {
                r.add("superblock_copy", format!("copy at sector {sector}: {e}"));
            }
            sb
        }
        Err(e @ (Error::BadChecksum { .. } | Error::BadMagic { .. } | Error::Corrupt(_))) => {
            r.add("superblock", format!("no usable superblock: {e}"));
            return Ok(r);
        }
        Err(e) => return Err(e),
    };
    if sb.field(2).is_some() {
        return Err(Error::Unsupported("the filesystem is encrypted".into()));
    }
    if sb.nr_devices != 1 {
        return Err(Error::Unsupported(format!("{} devices", sb.nr_devices)));
    }
    // Through the journal after an unclean shutdown, and after a clean one
    // whose superblock records a root that will not read (#166).
    let replay = match btree::journal_for(dev, &sb) {
        Ok(rp) => {
            r.replayed = rp.is_some();
            rp
        }
        Err(e) => {
            r.add("journal", e.to_string());
            return Ok(r);
        }
    };

    // Every node of every btree with a root.
    let roots: Vec<u8> = match &replay {
        Some(rp) => rp.roots.iter().map(|(id, _, _)| *id).collect(),
        None => sb.btree_roots()?.iter().map(|x| x.btree_id).collect(),
    };
    let mut trees: BTreeMap<u8, Vec<Bkey>> = BTreeMap::new();
    for id in roots {
        match walk_checked(dev, &sb, id, replay.as_ref(), &mut r) {
            Some(keys) => {
                trees.insert(id, keys);
            }
            None => continue,
        }
    }
    for (id, allowed) in KEY_TYPES_ALLOWED {
        for k in trees.get(id).into_iter().flatten() {
            if !KEY_TYPES_ANYWHERE.contains(&k.key_type) && !allowed.contains(&k.key_type) {
                r.add(
                    "invalid_bkey",
                    format!(
                        "btree {}: key {} is of type {}, which the btree does not hold",
                        journal::btree_name(*id),
                        k.pos,
                        k.key_type
                    ),
                );
            }
        }
    }

    // Snapshots (#139): each snapshot's parent, from the snapshots btree,
    // and each subvolume's root inode and snapshot. A key is seen by a
    // snapshot when it sits on that snapshot's chain, the nearest one
    // winning and a whiteout hiding; everything below is judged as each
    // subvolume sees it.
    let parents: BTreeMap<u32, u32> = trees
        .get(&SNAPSHOTS)
        .into_iter()
        .flatten()
        .filter(|k| k.key_type == KEY_SNAPSHOT && k.value.len() >= 8)
        .map(|k| (k.pos.offset as u32, (le64(&k.value, 0) >> 32) as u32))
        .collect();
    let subvolumes: BTreeMap<u32, (u64, u32)> = trees
        .get(&SUBVOLUMES)
        .into_iter()
        .flatten()
        .filter(|k| k.key_type == KEY_SUBVOLUME && k.value.len() >= 16)
        .map(|k| {
            (
                k.pos.offset as u32,
                (le64(&k.value, 8), (le64(&k.value, 0) >> 32) as u32),
            )
        })
        .collect();
    let root_snapshot = trees
        .get(&btree_id::INODES)
        .into_iter()
        .flatten()
        .find(|k| k.pos.inode == 0 && k.pos.offset == ROOT_INO && k.key_type == key_type::INODE_V3)
        .map(|k| k.pos.snapshot);
    // A key at a snapshot the snapshots btree does not have cannot be
    // placed; without a snapshots btree, every key is at the root's.
    let known = |snap: u32| {
        if parents.is_empty() {
            root_snapshot.is_none_or(|r| r == snap)
        } else {
            parents.contains_key(&snap)
        }
    };
    let unplaced = [
        btree_id::EXTENTS,
        btree_id::INODES,
        btree_id::DIRENTS,
        btree_id::XATTRS,
    ]
    .iter()
    .filter_map(|id| trees.get(id).map(|keys| (*id, keys)))
    .flat_map(|(id, keys)| keys.iter().map(move |k| (id, k)))
    .find(|(_, k)| !known(k.pos.snapshot));
    if let Some((id, k)) = unplaced {
        r.add(
            "snapshots",
            format!(
                "btree {}: key {} is at snapshot {}, which the snapshots btree does not have",
                journal::btree_name(id),
                k.pos,
                k.pos.snapshot
            ),
        );
    }
    let chain = |snap: u32| {
        let mut out = vec![snap];
        let mut at = snap;
        while let Some(&p) = parents.get(&at) {
            if p == 0 || out.len() >= MAX_SNAPSHOT_DEPTH || out.contains(&p) {
                break;
            }
            out.push(p);
            at = p;
        }
        out
    };
    snapshot_links(&trees, &parents, &subvolumes, &mut r);
    // Each subvolume's snapshot, and the root directories it holds; with no
    // subvolumes btree, the root inode's snapshot alone.
    let mut views: BTreeMap<u32, Vec<u64>> = BTreeMap::new();
    for (root, snap) in subvolumes.values() {
        views.entry(*snap).or_default().push(*root);
    }
    if views.is_empty() {
        views.insert(root_snapshot.unwrap_or(u32::MAX), vec![ROOT_INO]);
    }

    // Every version of each inode and each dirent slot, by position.
    let mut inode_keys: BTreeMap<u64, Vec<&Bkey>> = BTreeMap::new();
    for k in trees.get(&btree_id::INODES).into_iter().flatten() {
        if k.pos.inode == 0 {
            inode_keys.entry(k.pos.offset).or_default().push(k);
        }
    }
    let mut dirent_keys: BTreeMap<(u64, u64), Vec<&Bkey>> = BTreeMap::new();
    for k in trees.get(&btree_id::DIRENTS).into_iter().flatten() {
        dirent_keys
            .entry((k.pos.inode, k.pos.offset))
            .or_default()
            .push(k);
    }
    let mut xattr_keys: BTreeMap<(u64, u64), Vec<&Bkey>> = BTreeMap::new();
    for k in trees.get(&btree_id::XATTRS).into_iter().flatten() {
        xattr_keys
            .entry((k.pos.inode, k.pos.offset))
            .or_default()
            .push(k);
    }
    // A subvolume's root is named by an entry in its parent that names
    // the subvolume, not the inode.
    let subvolume_roots: std::collections::BTreeSet<u64> =
        subvolumes.values().map(|(root, _)| *root).collect();
    // Every inode key decoded once; a key that does not decode is named.
    let mut decoded: BTreeMap<Bpos, Inode> = BTreeMap::new();
    for k in trees.get(&btree_id::INODES).into_iter().flatten() {
        if k.key_type != key_type::INODE_V3 {
            continue;
        }
        match Inode::from_key(k) {
            Ok(i) => {
                decoded.insert(k.pos, i);
            }
            Err(e) => r.add("inode", format!("{}: {e}", k.pos)),
        }
    }
    r.inodes = decoded.len() as u64;
    // What the extents of each inode add up to (#221), and the newest
    // journal sequence written: the replayed journal's, or the clean
    // shutdown's.
    let extent_sums = ExtentSums::of(trees.get(&btree_id::EXTENTS).into_iter().flatten());
    let newest_seq = match &replay {
        Some(rp) => Some(rp.seq),
        None => sb
            .field(FIELD_CLEAN)
            .filter(|f| f.body.len() >= 16)
            .map(|f| le64(&f.body, 8)),
    };
    // The has-options flag (#163): bit 12 of bi_flags is set exactly when
    // the inode carries an option, set on it or inherited (S3 + S4, #142).
    // A nocow inode may carry it with none of the option fields set.
    for k in trees.get(&btree_id::INODES).into_iter().flatten() {
        if k.key_type != key_type::INODE_V3 || !decoded.contains_key(&k.pos) {
            continue;
        }
        let Ok(raw) = InodeV3Raw::parse(&k.value) else {
            continue;
        };
        let has = OPTION_FIELDS.iter().any(|f| raw.field(f).unwrap_or(0) != 0);
        let flagged = raw.flags & INODE_HAS_OPTIONS != 0;
        let nocow = raw.field("nocow").unwrap_or(0) != 0;
        if has != flagged && !(flagged && nocow) {
            r.add(
                "inode_has_inode_opts_flag_wrong",
                format!(
                    "inode {}: the has-options flag is {}, and it {} options",
                    k.pos,
                    if flagged { "set" } else { "clear" },
                    if has { "has" } else { "has no" }
                ),
            );
        }
        inode_metadata(k, &raw, &decoded[&k.pos], &extent_sums, newest_seq, &mut r);
    }
    // The inode a snapshot sees at `ino`.
    let inode_seen = |chain: &[u32], ino: u64| -> Option<&Inode> {
        let k = nearest(chain, inode_keys.get(&ino)?)?;
        decoded.get(&k.pos)
    };
    if root_snapshot.is_none() {
        r.add("root_inode", "the root directory's inode is missing");
    }

    // Inodes, dirents and link counts, as each subvolume sees them.
    let both = trees.contains_key(&btree_id::INODES) && trees.contains_key(&btree_id::DIRENTS);
    let mut found: Vec<Problem> = Vec::new();
    let mut dirents_seen = std::collections::BTreeSet::new();
    for (snap, roots) in &views {
        let c = chain(*snap);
        let mut add = |kind: &'static str, detail: String| {
            let p = Problem { kind, detail };
            if !found.contains(&p) {
                found.push(p);
            }
        };
        let mut links: BTreeMap<u64, u32> = BTreeMap::new();
        let mut subdirs: BTreeMap<u64, u32> = BTreeMap::new();
        for (slot, keys) in &dirent_keys {
            let Some(k) = nearest(&c, keys) else { continue };
            if k.key_type != key_type::DIRENT {
                continue;
            }
            if dirents_seen.insert(k.pos) {
                r.dirents += 1;
            }
            let d = match Dirent::from_key(k) {
                Ok(d) => d,
                Err(e) => {
                    add("dirent", format!("{}: {e}", k.pos));
                    continue;
                }
            };
            let name = String::from_utf8_lossy(&d.name);
            match inode_seen(&c, slot.0) {
                Some(dir) if dir.is_dir() => {
                    // Reachable from its name's hash slot along a run of
                    // taken slots, as a lookup walks it (#221, src/fs.rs):
                    // a collision or a whiteout between is valid, an empty
                    // slot is not.
                    let key = match crate::inode::dirent_folded_name(k) {
                        Ok(Some(f)) => f,
                        _ => d.name.clone(),
                    };
                    if let Some(at) = crate::inode::name_hash(dir.hash_type(), dir.hash_seed, &key)
                    {
                        let off = slot.1;
                        let taken = |o: u64| {
                            dirent_keys
                                .get(&(slot.0, o))
                                .and_then(|ks| nearest(&c, ks))
                                .is_some_and(|k| {
                                    matches!(k.key_type, key_type::DIRENT | key_type::HASH_WHITEOUT)
                                })
                        };
                        let reachable = at <= off
                            && off - at <= dirent_keys.len() as u64
                            && (at..off).all(taken);
                        if !reachable {
                            add(
                                "hash_table_key_wrong_offset",
                                format!(
                                    "dirent {name:?} in {} is at offset {off}, and its name \
                                     hashes to {at} with an empty slot between",
                                    d.dir
                                ),
                            );
                        }
                    }
                }
                Some(_) => add(
                    "dirent_in_non_directory",
                    format!("{name:?} in inode {}", d.dir),
                ),
                None => add(
                    "dirent_in_missing_directory",
                    format!("{name:?} in inode {}", d.dir),
                ),
            }
            // An entry naming a subvolume: its id in the low half of the
            // target (S3 + S4: the kernel oracle's `sv` and `snap`).
            if d.d_type == DT_SUBVOL {
                let id = d.inum as u32;
                if !subvolumes.contains_key(&id) {
                    add(
                        "dirent_to_missing_subvolume",
                        format!("{name:?} in {} names subvolume {id}", d.dir),
                    );
                }
                continue;
            }
            match inode_seen(&c, d.inum) {
                None => add(
                    "dirent_to_missing_inode",
                    format!("{name:?} in {} names inode {}", d.dir, d.inum),
                ),
                Some(i) => {
                    let want = match i.mode & 0o170000 {
                        0o040000 => 4,
                        0o100000 => 8,
                        0o120000 => 10,
                        0o020000 => 2,
                        0o060000 => 6,
                        0o010000 => 1,
                        0o140000 => 12,
                        _ => 0,
                    };
                    if want != d.d_type {
                        add(
                            "dirent_type",
                            format!(
                                "{name:?} says type {}, inode {} is {:o}",
                                d.d_type, d.inum, i.mode
                            ),
                        );
                    }
                    *links.entry(d.inum).or_default() += 1;
                    if i.is_dir() {
                        *subdirs.entry(d.dir).or_default() += 1;
                    }
                }
            }
        }
        // Link counts and reachability only mean something when both
        // btrees were read whole; a btree that failed is already reported.
        for ino in inode_keys.keys().filter(|_| both) {
            let Some(i) = inode_seen(&c, *ino) else {
                continue;
            };
            let want = if i.is_dir() {
                subdirs.get(ino).copied().unwrap_or(0) + 2
            } else {
                links.get(ino).copied().unwrap_or(0)
            };
            let root = roots.contains(ino);
            if !root && links.get(ino).copied().unwrap_or(0) == 0 {
                add(
                    "unreachable_inode",
                    format!("inode {ino} has no directory entry"),
                );
            } else if i.link_count() != want {
                add(
                    "link_count",
                    format!("inode {ino} counts {} links; {want} found", i.link_count()),
                );
            }
        }

        // Back-references (#163): an inode one entry names says which, in
        // bi_dir and bi_dir_offset, and that entry names it back. A file
        // with several names, a subvolume's root and an inode with no
        // back-reference at all are not judged.
        for (ino, ks) in inode_keys.iter().filter(|_| both) {
            if subvolume_roots.contains(ino) || roots.contains(ino) {
                continue;
            }
            let Some(k) = nearest(&c, ks) else { continue };
            let Some(i) = decoded.get(&k.pos) else {
                continue;
            };
            if !i.is_dir() && i.link_count() != 1 {
                continue;
            }
            let Ok(raw) = InodeV3Raw::parse(&k.value) else {
                continue;
            };
            let dir = raw.field("dir").unwrap_or(0);
            let off = raw.field("dir_offset").unwrap_or(0);
            if dir == 0 {
                continue;
            }
            let entry = dirent_keys
                .get(&(dir, off))
                .and_then(|ks| nearest(&c, ks))
                .filter(|d| d.key_type == key_type::DIRENT)
                .and_then(|d| Dirent::from_key(d).ok());
            match entry {
                None => add(
                    "inode_points_to_missing_dirent",
                    format!("inode {ino} names entry {off} of directory {dir}, which is not there"),
                ),
                Some(d) if d.d_type != DT_SUBVOL && d.inum != *ino => add(
                    "inode_points_to_wrong_dirent",
                    format!(
                        "inode {ino} names entry {off} of directory {dir}, which names inode {}",
                        d.inum
                    ),
                ),
                Some(_) => {}
            }
        }

        // Xattrs (#163): each one's inode is there, and the xattr is
        // reachable from its name's hash slot along a run of taken slots,
        // as a lookup walks it (S8, src/xattr.rs).
        for ((owner_ino, off), ks) in &xattr_keys {
            let Some(k) = nearest(&c, ks) else { continue };
            if k.key_type != key_type::XATTR {
                continue;
            }
            let Some(owner) = inode_seen(&c, *owner_ino) else {
                add(
                    "key_in_missing_inode",
                    format!("xattr {} of inode {owner_ino}, which is not there", k.pos),
                );
                continue;
            };
            let v = &k.value;
            if v.len() < 4 + v.get(1).copied().unwrap_or(0) as usize {
                continue;
            }
            let short = &v[4..4 + v[1] as usize];
            let Some(slot) = crate::xattr::slot(owner.hash_type(), owner.hash_seed, v[0], short)
            else {
                continue;
            };
            let taken = |o: u64| {
                xattr_keys
                    .get(&(*owner_ino, o))
                    .and_then(|ks| nearest(&c, ks))
                    .is_some_and(|k| {
                        matches!(k.key_type, key_type::XATTR | key_type::HASH_WHITEOUT)
                    })
            };
            let reachable =
                slot <= *off && *off - slot <= xattr_keys.len() as u64 && (slot..*off).all(taken);
            if !reachable {
                add(
                    "hash_table_key_wrong_offset",
                    format!(
                        "xattr {} is at offset {off}, and its name hashes to {slot} with an \
                         empty slot between",
                        k.pos
                    ),
                );
            }
        }
    }
    r.problems.extend(found);

    // Pointers are judged against the allocator: the bucket's generation
    // from its alloc_v4 key, this device's index, and no flags.
    let bucket_size = sb
        .members()
        .ok()
        .and_then(|m| m.get(sb.dev_idx as usize).map(|d| u64::from(d.bucket_size)))
        .filter(|&b| b > 0);
    let gens: BTreeMap<u64, u8> = trees
        .get(&btree_id::ALLOC)
        .into_iter()
        .flatten()
        .filter(|k| k.key_type == key_type::ALLOC_V4 && k.pos.inode == u64::from(sb.dev_idx))
        .filter_map(|k| crate::extent::alloc_v4_gen(&k.value).map(|g| (k.pos.offset, g)))
        .collect();
    let data_types: BTreeMap<u64, u8> = trees
        .get(&btree_id::ALLOC)
        .into_iter()
        .flatten()
        .filter(|k| k.key_type == key_type::ALLOC_V4 && k.pos.inode == u64::from(sb.dev_idx))
        .filter_map(|k| k.value.get(ALLOC_DATA_TYPE).map(|&t| (k.pos.offset, t)))
        .collect();
    // The allocation btrees are derived data, which the reference rebuilds
    // in its recovery (S1 1.3.1): after an unclean shutdown they can lag the
    // rest, as the reference leaves them (its `aged-unclean` crash images in
    // CI runs 38505641616 and 38507148919: a bucket's bucket_gens one
    // generation ahead of its alloc key). So they are judged against the
    // rest only on a filesystem shut down cleanly (#163).
    let judge_alloc = sb.is_clean();
    // A written extent's bucket is a user bucket in its alloc key; a bucket
    // with no alloc key is free (#163). An unwritten extent's is not judged:
    // no fixture shows what the reference gives it.
    let data_type_wrong = |what: String, ptr: &crate::extent::Ptr| -> Option<String> {
        let b = bucket_size.filter(|_| judge_alloc)?;
        if ptr.is_unwritten() {
            return None;
        }
        let bucket = ptr.offset / b;
        let t = data_types.get(&bucket).copied().unwrap_or(0);
        (t != DATA_USER).then(|| {
            format!(
                "{what}: bucket {bucket} holds its data, and its alloc key gives it data type {t}"
            )
        })
    };
    let have_alloc = trees.contains_key(&btree_id::ALLOC) && bucket_size.is_some();
    // Where the device's backpointers are, when the filesystem keeps them.
    let backpointers: Option<BTreeMap<u64, &Bkey>> = trees
        .get(&BACKPOINTERS)
        .filter(|_| judge_alloc)
        .map(|keys| {
            keys.iter()
                .filter(|k| k.key_type == KEY_BACKPOINTER && k.pos.inode == u64::from(sb.dev_idx))
                .map(|k| (k.pos.offset, k))
                .collect()
        });

    // Every bucket's generation as bucket_gens records it, against its
    // alloc key's; a bucket with no alloc key is at generation 0 (#163: the
    // reference's "incorrect gen in bucket_gens").
    if judge_alloc && trees.contains_key(&btree_id::ALLOC) {
        for k in trees
            .get(&BUCKET_GENS)
            .into_iter()
            .flatten()
            .filter(|k| k.key_type == KEY_BUCKET_GENS && k.pos.inode == u64::from(sb.dev_idx))
        {
            for (i, &recorded) in k.value.iter().take(256).enumerate() {
                let bucket = k.pos.offset << 8 | i as u64;
                let alloc = gens.get(&bucket).copied().unwrap_or(0);
                if recorded != alloc {
                    r.add(
                        "incorrect_gen_in_bucket_gens",
                        format!(
                            "bucket {bucket}: bucket_gens says generation {recorded}, its alloc \
                             key {alloc}"
                        ),
                    );
                }
            }
        }
    }

    // Every partly filled user or btree bucket has its fragmentation lru
    // entry, and every fragmentation lru entry is one of those (#163: the
    // reference's "missing fragmentation lru entry" and "incorrect lru
    // entry"). Judged on one device only: where another device's entries
    // sit is unobserved.
    if let Some(b) = bucket_size
        .filter(|_| judge_alloc && trees.contains_key(&btree_id::ALLOC) && sb.nr_devices == 1)
    {
        let want: std::collections::BTreeMap<(u64, u64), u64> = trees
            .get(&btree_id::ALLOC)
            .into_iter()
            .flatten()
            .filter(|k| k.key_type == key_type::ALLOC_V4 && k.pos.inode == u64::from(sb.dev_idx))
            .filter(|k| {
                matches!(
                    k.value.get(ALLOC_DATA_TYPE),
                    Some(&DATA_USER) | Some(&DATA_BTREE)
                )
            })
            .filter_map(|k| {
                let dirty = le64(k.value.get(16..24)?, 0) & 0xffff_ffff;
                (dirty > 0 && dirty < b).then_some((
                    (
                        (LRU_FRAGMENTATION << 61) | ((dirty << 31) / b),
                        k.pos.offset,
                    ),
                    dirty,
                ))
            })
            .collect();
        let have: std::collections::BTreeSet<(u64, u64)> = trees
            .get(&LRU)
            .into_iter()
            .flatten()
            .filter(|k| k.key_type == KEY_SET && k.pos.inode >> 61 == LRU_FRAGMENTATION)
            .map(|k| (k.pos.inode, k.pos.offset))
            .collect();
        for (&(at, bucket), dirty) in &want {
            if !have.contains(&(at, bucket)) {
                r.add(
                    "missing_fragmentation_lru_entry",
                    format!(
                        "bucket {bucket}: {dirty} of {b} sectors in use and no fragmentation lru \
                         entry at {at}:{bucket}"
                    ),
                );
            }
        }
        for &(at, bucket) in &have {
            if !want.contains_key(&(at, bucket)) {
                r.add(
                    "incorrect_lru_entry",
                    format!(
                        "lru entry {at}:{bucket}: bucket {bucket}'s fill gives it no entry there"
                    ),
                );
            }
        }
    }

    // Every bucket waiting for discard has its need_discard key, and every
    // need_discard key is one of those (#163). On one device only, as the
    // lru: where another device's keys sit is unobserved.
    if judge_alloc && trees.contains_key(&btree_id::ALLOC) && sb.nr_devices == 1 {
        let want: std::collections::BTreeSet<(u64, u64)> = trees
            .get(&btree_id::ALLOC)
            .into_iter()
            .flatten()
            .filter(|k| k.key_type == key_type::ALLOC_V4 && k.pos.inode == u64::from(sb.dev_idx))
            .filter(|k| k.value.get(ALLOC_DATA_TYPE) == Some(&DATA_NEED_DISCARD))
            .filter_map(|k| Some((le64(k.value.get(48..56)?, 0), k.pos.offset)))
            .collect();
        let have: std::collections::BTreeSet<(u64, u64)> = trees
            .get(&NEED_DISCARD)
            .into_iter()
            .flatten()
            .filter(|k| k.key_type == KEY_SET)
            .map(|k| (k.pos.inode, k.pos.offset))
            .collect();
        for &(seq, bucket) in want.difference(&have) {
            r.add(
                "missing_need_discard_entry",
                format!("bucket {bucket}: waiting for discard and no need_discard key at {seq}:{bucket}"),
            );
        }
        for &(seq, bucket) in have.difference(&want) {
            r.add(
                "incorrect_need_discard_entry",
                format!(
                    "need_discard key {seq}:{bucket}: bucket {bucket}'s alloc key gives it no key there"
                ),
            );
        }
    }

    // The device's accounting by data type is what its alloc keys add up
    // to (#163: the reference's "accounting mismatch"): each data type's
    // buckets, their dirty sectors, and the unused part of each bucket with
    // any in use; free, every bucket from the first usable one with no
    // alloc key of another type. Every fixture the reference made agrees,
    // all 51 of them. On one device only, as the lru.
    let member = sb
        .members()
        .ok()
        .and_then(|m| m.get(sb.dev_idx as usize).cloned());
    if let (true, 1, Some(b), Some(m)) = (
        judge_alloc && trees.contains_key(&btree_id::ALLOC),
        sb.nr_devices,
        bucket_size,
        &member,
    ) {
        let mut want: BTreeMap<u8, [u64; 3]> = BTreeMap::new();
        for k in trees
            .get(&btree_id::ALLOC)
            .into_iter()
            .flatten()
            .filter(|k| k.key_type == key_type::ALLOC_V4 && k.pos.inode == u64::from(sb.dev_idx))
            .filter(|k| k.pos.offset >= u64::from(m.first_bucket))
        {
            let (Some(&t), Some(d)) = (k.value.get(ALLOC_DATA_TYPE), k.value.get(16..24)) else {
                continue;
            };
            let dirty = le64(d, 0) & 0xffff_ffff;
            let e = want.entry(t).or_default();
            e[0] += 1;
            e[1] += dirty;
            if dirty > 0 {
                e[2] += b.saturating_sub(dirty);
            }
        }
        let used: u64 = want
            .iter()
            .filter(|(&t, _)| t != 0)
            .map(|(_, e)| e[0])
            .sum();
        let total = m.nbuckets.saturating_sub(u64::from(m.first_bucket));
        want.insert(0, [total.saturating_sub(used), 0, 0]);
        let have: BTreeMap<u8, [u64; 3]> = trees
            .get(&ACCOUNTING)
            .into_iter()
            .flatten()
            .filter(|k| {
                k.key_type == KEY_ACCOUNTING
                    && k.pos.inode >> 56 == ACCT_DEV_DATA_TYPE
                    && (k.pos.inode >> 48) & 0xff == u64::from(sb.dev_idx)
            })
            .map(|k| {
                let mut v = [0u64; 3];
                for (i, x) in v.iter_mut().enumerate() {
                    if let Some(w) = k.value.get(i * 8..i * 8 + 8) {
                        *x = le64(w, 0);
                    }
                }
                (((k.pos.inode >> 40) & 0xff) as u8, v)
            })
            .collect();
        let types: std::collections::BTreeSet<u8> =
            want.keys().chain(have.keys()).copied().collect();
        for t in types {
            let (w, h) = (
                want.get(&t).copied().unwrap_or_default(),
                have.get(&t).copied().unwrap_or_default(),
            );
            if w != h {
                r.add(
                    "accounting_mismatch",
                    format!(
                        "data type {t} on device {}: the alloc keys add up to {} buckets, {} \
                         sectors, {} fragmented; the accounting says {}, {}, {}",
                        sb.dev_idx, w[0], w[1], w[2], h[0], h[1], h[2]
                    ),
                );
            }
        }
    }

    // Every alloc key of this device, by bucket: its data type, or `None`
    // for a key this checker does not read.
    let alloc_keys: BTreeMap<u64, Option<u8>> = trees
        .get(&btree_id::ALLOC)
        .into_iter()
        .flatten()
        .filter(|k| k.pos.inode == u64::from(sb.dev_idx))
        .filter(|k| !matches!(k.key_type, key_type::DELETED | key_type::WHITEOUT))
        .map(|k| {
            let t = match k.key_type {
                key_type::ALLOC_V4 => k.value.get(ALLOC_DATA_TYPE).copied(),
                _ => None,
            };
            (k.pos.offset, t)
        })
        .collect();

    // Every backpointer sits in a bucket with an alloc key (#163: the
    // reference's "backpointer for nonexistent alloc key"): a bucket holding
    // data or a node has one.
    if let (Some(bps), Some(b), true) = (
        &backpointers,
        bucket_size,
        trees.contains_key(&btree_id::ALLOC),
    ) {
        for (&at, bp) in bps {
            let bucket = (at >> 16) / b;
            if !alloc_keys.contains_key(&bucket) {
                r.add(
                    "backpointer_for_nonexistent_alloc_key",
                    format!("backpointer {}: bucket {bucket} has no alloc key", bp.pos),
                );
            }
        }
    }

    // Free space (#163): the freespace btree's runs hold exactly the
    // buckets from the first usable one to the last whose alloc key is
    // missing or of data type free (docs/clean-room.md, "Allocating
    // space"; the reference's "bucket incorrectly set in freespace btree",
    // "bucket incorrectly unset in freespace btree", "hole in alloc btree
    // missing in freespace btree" and "entry in freespace btree for
    // nonexistant dev:bucket"). On one device only, as the lru, and not at
    // all once a free bucket reaches the generation no run has been seen at.
    let high_gen = trees
        .get(&btree_id::ALLOC)
        .into_iter()
        .flatten()
        .filter(|k| k.key_type == key_type::ALLOC_V4 && k.pos.inode == u64::from(sb.dev_idx))
        .filter(|k| k.value.get(ALLOC_DATA_TYPE) == Some(&0))
        .any(|k| crate::extent::alloc_v4_gen(&k.value).unwrap_or(0) >= FREESPACE_GEN_UNOBSERVED);
    if let (true, 1, Some(runs), Some(m)) = (
        judge_alloc && trees.contains_key(&btree_id::ALLOC) && !high_gen,
        sb.nr_devices,
        trees.get(&FREESPACE),
        &member,
    ) {
        free_space(runs, &alloc_keys, u64::from(sb.dev_idx), m, &mut r);
    }

    // Extents: their inode, their bounds, their neighbours, their data.
    let have_inodes = trees.contains_key(&btree_id::INODES);
    // (inode, snapshot, end sector) of the last extent-like key seen: an
    // extent that starts before it ends overlaps it (S1's check_extents,
    // "no overlaps"; issue #60).
    let mut prev: Option<(u64, u32, u64)> = None;
    // Every `reflink_p`: where it is, and the reflink btree sectors
    // `idx..idx + size` it shows (#139).
    let mut pointers: Vec<(Bpos, u64, u64)> = Vec::new();
    for k in trees
        .get(&btree_id::EXTENTS)
        .into_iter()
        .flatten()
        .filter(|_| have_inodes)
    {
        if k.key_type == key_type::ERROR {
            r.add(
                "data_lost",
                format!(
                    "extent {}: an `error` extent, its data permanently lost",
                    k.pos
                ),
            );
        }
        if k.key_type == key_type::REFLINK_P {
            match crate::extent::reflink_p_idx(&k.value) {
                Ok(idx) => pointers.push((k.pos, idx, idx + u64::from(k.size))),
                Err(e) => r.add("reflink", format!("extent {}: {e}", k.pos)),
            }
        }
        if matches!(
            k.key_type,
            key_type::EXTENT
                | key_type::INLINE_DATA
                | key_type::RESERVATION
                | key_type::ERROR
                | key_type::REFLINK_P
        ) {
            if let Some((inode, snap, end)) = prev {
                if inode == k.pos.inode && snap == k.pos.snapshot && k.start_offset() < end {
                    r.add(
                        "extent_overlap",
                        format!(
                            "extent {} starts at sector {} before the extent before it ends at {end}",
                            k.pos,
                            k.start_offset()
                        ),
                    );
                }
            }
            prev = Some((k.pos.inode, k.pos.snapshot, k.pos.offset));
        }
        match k.key_type {
            key_type::EXTENT | key_type::INLINE_DATA => {}
            _ => continue,
        }
        r.extents += 1;
        let Some(i) = inode_seen(&chain(k.pos.snapshot), k.pos.inode) else {
            r.add("extent_without_inode", format!("extent {}", k.pos));
            continue;
        };
        // In sectors: in bytes an extent starting 2^55 sectors in overflows
        // (#150), and a wrapped product would judge it inside its file.
        if k.start_offset() >= i.size.div_ceil(512) {
            r.add(
                "extent_past_end_of_inode",
                format!("extent {} of inode {} (size {})", k.pos, i.ino, i.size),
            );
        }
        if k.key_type == key_type::EXTENT {
            let e = match DataExtent::from_key(k) {
                Ok(e) => e,
                Err(e) => {
                    r.add("extent_data", format!("extent {}: {e}", k.pos));
                    continue;
                }
            };
            if have_alloc {
                let gen = bucket_size.and_then(|b| gens.get(&(e.ptr.offset / b)).copied());
                if let Err(err) = e.ptr.check(sb.dev_idx, gen) {
                    r.add("extent_pointer", format!("extent {}: {err}", k.pos));
                    continue;
                }
                if let Some(d) = data_type_wrong(format!("extent {}", k.pos), &e.ptr) {
                    r.add("alloc_key_data_type_wrong", d);
                }
            }
            if let Some(bps) = &backpointers {
                data_backpointer(k, &e, bps, &mut r);
            }
            // A poisoned extent's data is known to be bad, and the
            // filesystem says so: the reference checker passes an image
            // holding one (S8, the `poison` image), and so does this one.
            // An unwritten one has no data to check (#158).
            if matches!(crate::extent::poisoned(&k.value), Ok(true)) || e.ptr.is_unwritten() {
                continue;
            }
            if let Err(e) = verify_data(dev, &e) {
                r.add("extent_data", format!("extent {}: {e}", k.pos));
            }
        }
    }

    // Reflinks (#139): every `reflink_p` lands on indirect extents that
    // cover its whole range, and each indirect extent's refcount is the
    // number of pointers into it (S1 9.1.6: the count of references; the
    // kernel oracle's clone pair, two pointers each, passes the reference
    // checker). Each indirect extent's data is checked as an extent's is.
    let indirect: Vec<&Bkey> = trees
        .get(&btree_id::REFLINK)
        .into_iter()
        .flatten()
        .filter(|k| k.key_type == key_type::REFLINK_V)
        .collect();
    let mut refs = vec![0u64; indirect.len()];
    for (pos, idx, end) in &pointers {
        let mut at = *idx;
        for (n, v) in indirect.iter().enumerate() {
            if v.start_offset() < *end && v.pos.offset > *idx {
                refs[n] += 1;
                if v.start_offset() <= at {
                    at = at.max(v.pos.offset);
                }
            }
        }
        if at < *end {
            r.add(
                "reflink_missing",
                format!(
                    "extent {pos}: the reflink btree has nothing at sector {at}, which it points \
                     into"
                ),
            );
        }
    }
    for (v, n) in indirect.iter().zip(refs) {
        if v.value.len() < 8 {
            r.add("reflink", format!("reflink {}: no refcount", v.pos));
            continue;
        }
        let count = le64(&v.value, 0);
        if count != n {
            r.add(
                "reflink_refcount",
                format!(
                    "reflink {}: refcount {count}; {n} pointers into it found",
                    v.pos
                ),
            );
        }
        let e = match DataExtent::from_key(&Bkey {
            key_type: key_type::EXTENT,
            value: v.value[8..].to_vec(),
            ..(*v).clone()
        }) {
            Ok(e) => e,
            Err(e) => {
                r.add("reflink", format!("reflink {}: {e}", v.pos));
                continue;
            }
        };
        if have_alloc {
            let gen = bucket_size.and_then(|b| gens.get(&(e.ptr.offset / b)).copied());
            if let Err(err) = e.ptr.check(sb.dev_idx, gen) {
                r.add("extent_pointer", format!("reflink {}: {err}", v.pos));
                continue;
            }
            if let Some(d) = data_type_wrong(format!("reflink {}", v.pos), &e.ptr) {
                r.add("alloc_key_data_type_wrong", d);
            }
        }
        if let Err(e) = verify_data(dev, &e) {
            r.add("extent_data", format!("reflink {}: {e}", v.pos));
        }
    }
    Ok(r)
}

/// The freespace btree's runs against the alloc keys of device `dev`
/// (#163). A bucket in two runs is not judged. Every loop is over keys,
/// never over buckets, so a run's size or a member's bucket count, however
/// large, costs nothing.
fn free_space(
    runs: &[Bkey],
    alloc: &BTreeMap<u64, Option<u8>>,
    dev: u64,
    m: &crate::superblock::Member,
    r: &mut Report,
) {
    let first = u64::from(m.first_bucket);
    let mut held: Vec<(u64, u64)> = Vec::new();
    for k in runs.iter().filter(|k| k.key_type == KEY_SET) {
        let end = k.pos.offset;
        let start = end.checked_sub(u64::from(k.size));
        let (Some(start), true) = (start, k.pos.inode == dev && end <= m.nbuckets) else {
            r.add(
                "freespace_entry_for_nonexistent_bucket",
                format!(
                    "freespace run {} of {} buckets: device {dev} has buckets {first}..{}",
                    k.pos, k.size, m.nbuckets
                ),
            );
            continue;
        };
        let used: Vec<(u64, u8)> = alloc
            .range(start..end)
            .filter_map(|(&b, t)| t.filter(|&t| t != 0).map(|t| (b, t)))
            .collect();
        if start < first || !used.is_empty() {
            let (bucket, why) = match used.first() {
                Some((b, t)) if start >= first => {
                    (*b, format!("its alloc key gives it data type {t}"))
                }
                _ => (start, format!("the first usable bucket is {first}")),
            };
            r.add(
                "bucket_incorrectly_set_in_freespace_btree",
                format!(
                    "freespace run {} holds buckets {start}..{end}, {} of them not free: bucket \
                     {bucket}, {why}",
                    k.pos,
                    used.len() + usize::from(start < first)
                ),
            );
        }
        held.push((start.max(first), end));
    }
    held.sort_unstable();
    // The stretches no run holds, from the first usable bucket on.
    let mut gaps = Vec::new();
    let mut at = first;
    for (start, end) in held {
        if start > at {
            gaps.push((at, start));
        }
        at = at.max(end);
    }
    if at < m.nbuckets {
        gaps.push((at, m.nbuckets));
    }
    let hole = |a: u64, b: u64, r: &mut Report| {
        r.add(
            "hole_in_alloc_btree_missing_in_freespace_btree",
            format!("buckets {a}..{b}: no alloc key, and no freespace run holds them"),
        );
    };
    for (from, to) in gaps {
        let mut at = from;
        for (&b, t) in alloc.range(from..to) {
            if b > at {
                hole(at, b, r);
            }
            if *t == Some(0) {
                r.add(
                    "bucket_incorrectly_unset_in_freespace_btree",
                    format!("bucket {b}: free in its alloc key, and no freespace run holds it"),
                );
            }
            at = b + 1;
        }
        if at < to {
            hole(at, to, r);
        }
    }
}

/// The links between the snapshot btrees (#220): each snapshot tree names
/// a subvolume the subvolumes btree has; each snapshot names a tree the
/// snapshot_trees btree has, whose root is the root its parents lead to;
/// and each entry of a snapshot's skiplist is none (0), the snapshot, or
/// one of its ancestors (S1 9.4.4: entries at or above it). A parent cycle, or a parent the snapshots btree does not
/// have, leads to no root: the walk is bounded, and the snapshot is in no
/// tree.
fn snapshot_links(
    trees: &BTreeMap<u8, Vec<Bkey>>,
    parents: &BTreeMap<u32, u32>,
    subvolumes: &BTreeMap<u32, (u64, u32)>,
    r: &mut Report,
) {
    let Some(tree_keys) = trees.get(&SNAPSHOT_TREES) else {
        return;
    };
    // Each tree's master subvolume and root snapshot.
    let snapshot_trees: BTreeMap<u32, (u32, u32)> = tree_keys
        .iter()
        .filter(|k| k.key_type == KEY_SNAPSHOT_TREE && k.value.len() >= 8)
        .map(|k| {
            (
                k.pos.offset as u32,
                (le32(&k.value, 0), le32(&k.value, TREE_ROOT)),
            )
        })
        .collect();
    // The tree each snapshot names.
    let snapshot_tree_of: BTreeMap<u32, u32> = trees
        .get(&SNAPSHOTS)
        .into_iter()
        .flatten()
        .filter(|k| k.key_type == KEY_SNAPSHOT && k.value.len() >= SNAP_TREE + 4)
        .map(|k| (k.pos.offset as u32, le32(&k.value, SNAP_TREE)))
        .collect();
    for (id, (subvol, root)) in &snapshot_trees {
        // A tree's root snapshot is in the snapshots btree and names the
        // tree back; the reference refuses a tree whose root is missing or
        // in another tree as pointing "to missing/incorrect snapshot"
        // (#267).
        match snapshot_tree_of.get(root) {
            Some(t) if t == id => {}
            Some(other) => r.add(
                "snapshot_tree_to_bad_root_snapshot",
                format!("snapshot tree {id}'s root snapshot {root} is in tree {other}"),
            ),
            None => r.add(
                "snapshot_tree_to_bad_root_snapshot",
                format!("snapshot tree {id}'s root snapshot {root} is not in the snapshots btree"),
            ),
        }
        let Some(&(_, snap)) = subvolumes.get(subvol) else {
            r.add(
                "snapshot_tree_to_missing_subvol",
                format!(
                    "snapshot tree {id} names subvolume {subvol}, which the subvolumes btree does not have"
                ),
            );
            continue;
        };
        // The master subvolume exists, but its snapshot belongs to another
        // tree (#265). A snapshot the btree lacks is reported elsewhere.
        if let Some(&other) = snapshot_tree_of.get(&snap).filter(|&&t| t != *id) {
            r.add(
                "snapshot_tree_to_wrong_subvol",
                format!(
                    "snapshot tree {id} names subvolume {subvol}, whose snapshot {snap} is in tree {other}"
                ),
            );
        }
    }
    // A snapshot's parents, nearest first, and the root they end at: none
    // when they repeat, go deeper than any tree, or name a snapshot the
    // btree does not have.
    let ancestors = |snap: u32| -> (Vec<u32>, Option<u32>) {
        let mut out = Vec::new();
        let mut at = snap;
        loop {
            match parents.get(&at) {
                Some(0) => return (out, Some(at)),
                Some(&p) if p != snap && !out.contains(&p) && out.len() < MAX_SNAPSHOT_DEPTH => {
                    out.push(p);
                    at = p;
                }
                _ => return (out, None),
            }
        }
    };
    for k in trees.get(&SNAPSHOTS).into_iter().flatten() {
        if k.key_type != KEY_SNAPSHOT || k.value.len() < SNAP_SKIPLIST + 12 {
            continue;
        }
        let snap = k.pos.offset as u32;
        let (above, root) = ancestors(snap);
        let tree = le32(&k.value, SNAP_TREE);
        match (snapshot_trees.get(&tree), root) {
            (None, _) => r.add(
                "snapshot_to_bad_snapshot_tree",
                format!(
                    "snapshot {snap} names tree {tree}, which the snapshot_trees btree does not have"
                ),
            ),
            (Some(_), None) => r.add(
                "snapshot_to_bad_snapshot_tree",
                format!(
                    "snapshot {snap} names tree {tree}, but its parents lead to no root: a cycle, \
                     or a parent the snapshots btree does not have"
                ),
            ),
            (Some(&(_, want)), Some(root)) if want != root => r.add(
                "snapshot_to_bad_snapshot_tree",
                format!(
                    "snapshot {snap} names tree {tree}, whose root is {want}, but its own root is {root}"
                ),
            ),
            _ => {}
        }
        let bad: Vec<u32> = (0..3)
            .map(|i| le32(&k.value, SNAP_SKIPLIST + 4 * i))
            .filter(|&s| s != 0 && s != snap && !above.contains(&s))
            .collect();
        if !bad.is_empty() {
            r.add(
                "snapshot_bad_skiplist",
                format!("snapshot {snap}: skiplist entries {bad:?} are not among its ancestors {above:?}"),
            );
        }
    }
}

/// The sizes of each inode's extents, by `(inode, snapshot)`, and the
/// snapshots each inode's extents btree keys are at.
#[derive(Default)]
struct ExtentSums {
    sectors: BTreeMap<(u64, u32), u64>,
    snapshots: BTreeMap<u64, std::collections::BTreeSet<u32>>,
}

impl ExtentSums {
    fn of<'a>(keys: impl Iterator<Item = &'a Bkey>) -> Self {
        let mut out = Self::default();
        for k in keys {
            out.snapshots
                .entry(k.pos.inode)
                .or_default()
                .insert(k.pos.snapshot);
            if SECTOR_KEYS.contains(&k.key_type) {
                let sum = out
                    .sectors
                    .entry((k.pos.inode, k.pos.snapshot))
                    .or_default();
                *sum = sum.saturating_add(u64::from(k.size));
            }
        }
        out
    }

    /// What the extents of the inode at `pos` add up to, when every key of
    /// its extents is at the inode's own snapshot. With keys at other
    /// snapshots too, which of them this version of the inode counts
    /// depends on what its snapshot sees, and it is not judged.
    fn of_inode(&self, pos: Bpos) -> Option<u64> {
        let alone = self
            .snapshots
            .get(&pos.offset)
            .is_none_or(|s| s.iter().all(|&x| x == pos.snapshot));
        alone.then(|| {
            self.sectors
                .get(&(pos.offset, pos.snapshot))
                .copied()
                .unwrap_or(0)
        })
    }
}

/// An inode's metadata against the rest (#221): a directory's size is 0;
/// its journal sequence is not past any the journal reaches; and its
/// sector count is what its extents add up to.
fn inode_metadata(
    k: &Bkey,
    raw: &InodeV3Raw,
    i: &Inode,
    extents: &ExtentSums,
    newest_seq: Option<u64>,
    r: &mut Report,
) {
    if i.is_dir() && i.size != 0 {
        r.add(
            "directory_nonzero_i_size",
            format!("directory {}: size {}", k.pos, i.size),
        );
    }
    if let Some(newest) = newest_seq {
        if raw.journal_seq > newest.saturating_add(JOURNAL_SEQ_SLACK) {
            r.add(
                "inode_journal_seq_in_future",
                format!(
                    "inode {}: journal sequence {}, and the newest written is {newest}",
                    k.pos, raw.journal_seq
                ),
            );
        }
    }
    if let Some(want) = extents.of_inode(k.pos) {
        if i.sectors != want {
            r.add(
                "inode_i_sectors_wrong",
                format!(
                    "inode {}: {} sectors, and its extents add up to {want}",
                    k.pos, i.sectors
                ),
            );
        }
    }
}

/// Of the versions of one key, the one a snapshot whose chain is `chain`
/// sees: the one at the snapshot nearest it, a whiteout included (the
/// caller reads a whiteout as nothing there).
fn nearest<'a>(chain: &[u32], keys: &[&'a Bkey]) -> Option<&'a Bkey> {
    keys.iter()
        .filter_map(|k| Some((chain.iter().position(|&s| s == k.pos.snapshot)?, *k)))
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, k)| k)
}

/// The observed backpointer lookup for a written data extent (#163).
fn data_backpointer(k: &Bkey, e: &DataExtent, bps: &BTreeMap<u64, &Bkey>, r: &mut Report) {
    // The aged image's piece at offset 136 of the region at sector 26880
    // has its backpointer at 26880 << 16 | 136. Only uncompressed pieces
    // and whole compressed regions have an observed lookup rule.
    let known = e.crc.is_none_or(|c| {
        c.compression_type == crate::extent::compression::NONE
            || (c.offset == 0 && u64::from(c.uncompressed_size) == e.len)
    });
    let at = e.ptr.offset << 16 | u64::from(e.crc.map_or(0, |c| c.offset));
    if !known || e.ptr.is_unwritten() {
        return;
    }
    let Some(bp) = bps.get(&at) else {
        r.add(
            "missing_backpointer",
            format!(
                "extent {}: no backpointer at device sector {} offset {}",
                k.pos,
                e.ptr.offset,
                at & 0xffff
            ),
        );
        return;
    };
    // S3/S4/S8: three header bytes, five uninterpreted bytes, length u32,
    // then the target bpos. #256's second-word mutation changes both the
    // length and snapshot without moving the backpointer key. The reference
    // accepts the target snapshot changed alone (run 39094186901, #348), so
    // only the target inode and offset are compared.
    let length = e.crc.map_or(e.len, |c| {
        if c.compression_type == crate::extent::compression::NONE {
            e.len
        } else {
            u64::from(c.compressed_size)
        }
    });
    let v = &bp.value;
    let matches = v.len() >= 32
        && v[..3] == [btree_id::EXTENTS, 0, DATA_USER]
        && u64::from(le32(v, 8)) == length
        && Bpos::parse(&v[12..32])
            .is_ok_and(|pos| pos.inode == k.pos.inode && pos.offset == k.pos.offset);
    if !matches {
        r.add(
            "backpointer_to_missing_ptr",
            format!(
                "backpointer {}: value does not match extent {} (expected btree extents, \
                 level 0, data type user, length {length})",
                bp.pos, k.pos
            ),
        );
    }
}

/// Read one extent's stored bytes and verify its checksum.
fn verify_data(dev: &dyn BlockRead, e: &DataExtent) -> Result<()> {
    let Some(crc) = e.crc else { return Ok(()) };
    if !crate::csum::is_known(crc.csum_type) {
        return Err(Error::Unsupported(format!(
            "data checksum type {}",
            crc.csum_type
        )));
    }
    let mut raw = vec![0u8; crc.compressed_size as usize * 512];
    dev.read_at(e.ptr.offset * 512, &mut raw)?;
    crate::csum::verify(crc.csum_type, &raw, crc.csum_lo).map_err(|computed| Error::BadChecksum {
        what: "extent data",
        stored: crc.csum_lo,
        computed,
    })
}

/// Walk btree `id`, recording each node that cannot be read instead of
/// stopping, and the order of the keys found. `None` when not even its root
/// can be read.
fn walk_checked(
    dev: &dyn BlockRead,
    sb: &Superblock,
    id: u8,
    replay: Option<&Replay>,
    r: &mut Report,
) -> Option<Vec<Bkey>> {
    let name = journal::btree_name(id);
    match btree::walk_replayed(dev, sb, id, replay) {
        Ok(keys) => {
            let mut last: Option<Bpos> = None;
            for k in &keys {
                if last.is_some_and(|l| k.pos <= l) {
                    r.add(
                        "key_order",
                        format!("btree {name}: {} after {}", k.pos, last.unwrap()),
                    );
                }
                last = Some(k.pos);
                // check_key_fields covers the bucket_gens size refusal from
                // differential run 38814891184 ("size != 0") with the same
                // code and message, so it is reported once. It also reports
                // an xattr key in dirents (differential runs 38753768243 and
                // 38814891184) as bkey_invalid_type_for_btree, the one code
                // for that offender (#219).
                check_key_fields(id, k, r);
            }
            Some(keys)
        }
        Err(e) => {
            r.add("btree_node", format!("btree {name}: {e}"));
            None
        }
    }
}

/// What the reference refuses in a key's own fields (#219): a size on a
/// key of a btree whose keys have none ("size != 0", `bkey_size_nonzero`),
/// a snapshot on a key of a btree that is not snapshotted ("nonzero
/// snapshot", `bkey_snapshot_nonzero`), an inode, subvolume, snapshot,
/// snapshot_tree or inode allocation cursor at a position it cannot have,
/// an accounting key holding more than its kind and btree, and an xattr
/// whose lengths run past its value. Each pair of btree and key type below
/// is one differential run 38579070302, nightly-seed run 39018152705 or
/// nightly run 39756419740 saw the reference refuse; a pair nobody
/// observed is not judged (docs/clean-room.md).
fn check_key_fields(id: u8, k: &Bkey, r: &mut Report) {
    const SNAPSHOT_TREES: u8 = 15;
    const LOGGED_OPS: u8 = 17;
    const KEY_SNAPSHOT_TREE: u8 = 31;
    const KEY_INODE_ALLOC_CURSOR: u8 = 35;
    /// An accounting key's kind, the top byte of its inode; the lister
    /// prints 6 as `btree`, with the btree's id in the byte below.
    const ACCT_BTREE: u64 = 6;
    const SIZE_ZERO: &[(u8, u8)] = &[
        (btree_id::INODES, key_type::INODE_V3),
        (btree_id::DIRENTS, key_type::DIRENT),
        (btree_id::XATTRS, key_type::XATTR),
        (SUBVOLUMES, KEY_SUBVOLUME),
        (LRU, KEY_SET),
        (BACKPOINTERS, KEY_BACKPOINTER),
        (BUCKET_GENS, KEY_BUCKET_GENS),
        (SNAPSHOT_TREES, KEY_SNAPSHOT_TREE),
        (LOGGED_OPS, KEY_INODE_ALLOC_CURSOR),
        (ACCOUNTING, KEY_ACCOUNTING),
    ];
    const SNAPSHOT_ZERO: &[(u8, u8)] = &[
        (SUBVOLUMES, KEY_SUBVOLUME),
        (SNAPSHOTS, KEY_SNAPSHOT),
        (LRU, KEY_SET),
        (NEED_DISCARD, KEY_SET),
        (FREESPACE, KEY_SET),
        (LOGGED_OPS, KEY_INODE_ALLOC_CURSOR),
    ];
    let name = journal::btree_name(id);
    if k.size != 0 && SIZE_ZERO.contains(&(id, k.key_type)) {
        r.add(
            "bkey_size_nonzero",
            format!("btree {name}: key {} has size {}", k.pos, k.size),
        );
    }
    if k.pos.snapshot != 0 && SNAPSHOT_ZERO.contains(&(id, k.key_type)) {
        r.add(
            "bkey_snapshot_nonzero",
            format!(
                "btree {name}: key {} is at snapshot {}",
                k.pos, k.pos.snapshot
            ),
        );
    }
    let v = &k.value;
    match (id, k.key_type) {
        // Run 39018152705 rejects inode_v3 at inode U64_MAX as
        // "nonzero k.p.inode"; its inode number belongs in the offset (#255).
        (btree_id::INODES, key_type::INODE_V3) if k.pos.inode != 0 => r.add(
            "inode_pos_inode_nonzero",
            format!("btree {name}: inode_v3 at {}, not at inode 0", k.pos),
        ),
        // A subvolume is at `0:id` ("Snapshots and subvolumes"): one at
        // offset 0, or at another inode, is "invalid pos".
        (SUBVOLUMES, KEY_SUBVOLUME) if k.pos.offset == 0 || k.pos.inode != 0 => r.add(
            "subvol_pos_bad",
            format!("btree {name}: subvolume at {}, not at 0:id", k.pos),
        ),
        // Nightly-seed run 39018152705: the root snapshot moved to
        // inode U64_MAX is "bad pos"; run 38878947701 also saw inode 1.
        (SNAPSHOTS, KEY_SNAPSHOT) if k.pos.inode != 0 => r.add(
            "snapshot_pos_bad",
            format!("btree {name}: snapshot at {}, not at inode 0", k.pos),
        ),
        // The inode allocation cursor is at 1:1:0 (S8). Nightly run
        // 39756419740 rejects it moved to inode U64_MAX and to inode 0 as
        // "k.p.inode bad" (#376).
        (LOGGED_OPS, KEY_INODE_ALLOC_CURSOR) if k.pos.inode != 1 => r.add(
            "inode_alloc_cursor_inode_bad",
            format!(
                "btree {name}: inode_alloc_cursor at {}, not at inode 1",
                k.pos
            ),
        ),
        // Run 39018152705 also rejects tree 1 moved to inode 1 as
        // "bad pos"; the tree id remains in the offset (#254). Nightly run
        // 38878947701 rejects one moved to offset U64_MAX the same way: a
        // tree id is 32 bits, so an offset above U32_MAX names no tree (#219).
        (SNAPSHOT_TREES, KEY_SNAPSHOT_TREE)
            if k.pos.offset == 0 || k.pos.offset > u64::from(u32::MAX) || k.pos.inode != 0 =>
        {
            r.add(
                "snapshot_tree_pos_bad",
                format!("btree {name}: snapshot_tree at {}, not at 0:tree", k.pos),
            )
        }
        // A `btree` accounting key is its kind and the btree's id in the
        // top two bytes of its inode; a bit below them, or a snapshot, is
        // "junk at end of accounting key".
        (ACCOUNTING, KEY_ACCOUNTING)
            if k.pos.inode >> 56 == ACCT_BTREE
                && (k.pos.inode & ((1 << 48) - 1) != 0 || k.pos.snapshot != 0) =>
        {
            r.add(
                "accounting_key_junk_at_end",
                format!(
                    "btree {name}: accounting key {} holds more than its btree",
                    k.pos
                ),
            )
        }
        // An xattr's header gives its name's and its value's lengths
        // (src/xattr.rs); together they must fit the key's value.
        (btree_id::XATTRS, key_type::XATTR)
            if v.len() >= 4
                && 4 + usize::from(v[1]) + usize::from(u16::from_le_bytes([v[2], v[3]]))
                    > v.len() =>
        {
            r.add(
                "xattr_val_size_too_small",
                format!(
                    "btree {name}: xattr {} has a {}-byte name and a {}-byte value in {} bytes",
                    k.pos,
                    v[1],
                    u16::from_le_bytes([v[2], v[3]]),
                    v.len()
                ),
            )
        }
        // An xattr belongs in the xattrs btree; differential run
        // 38753768243 saw the reference delete one found in dirents as
        // "invalid key type for btree dirents (xattr)".
        (btree_id::DIRENTS, key_type::XATTR) => r.add(
            "bkey_invalid_type_for_btree",
            format!("btree {name}: key {} is an xattr", k.pos),
        ),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bkey::Bpos;

    #[test]
    fn judged_key_types_match_the_recorded_reference_verdicts() {
        // Non-crashing, one-key images judged in run 39037096192's
        // guest suite. These are type verdicts, not value validation.
        for (id, accepted) in [
            (17, vec![0, 1, 2, 3, 25, 32, 33, 35, 37, 39, 40]),
            (20, vec![0, 1, 2, 3, 25, 34, 40]),
            (24, vec![0, 1, 2, 3, 25, 40]),
        ] {
            let allowed = KEY_TYPES_ALLOWED
                .iter()
                .find(|(tree, _)| *tree == id)
                .unwrap()
                .1;
            for t in 0..=40 {
                assert_eq!(
                    KEY_TYPES_ANYWHERE.contains(&t) || allowed.contains(&t),
                    accepted.contains(&t),
                    "btree {id}, key type {t}: recorded reference verdict differs"
                );
            }
        }
        let xattrs = KEY_TYPES_ALLOWED
            .iter()
            .find(|(tree, _)| *tree == btree_id::XATTRS)
            .unwrap()
            .1;
        assert_eq!(xattrs, &[key_type::XATTR]);
        let freespace = KEY_TYPES_ALLOWED
            .iter()
            .find(|(tree, _)| *tree == FREESPACE)
            .unwrap()
            .1;
        assert_eq!(freespace, &[KEY_SET]);
    }

    #[test]
    fn the_reconcile_tree_name_matches_the_reference_diagnostic() {
        // Run 39037096192 prints this name when refusing the exact dirent.
        assert_eq!(journal::btree_name(24), "reconcile_work_phys");
        assert_eq!(journal::btree_name(255), "btree255");
    }

    /// A key at `0:offset`, as the snapshot btrees file theirs.
    fn key(key_type: u8, offset: u64, value: Vec<u8>) -> Bkey {
        Bkey {
            key_type,
            size: 0,
            version_hi: 0,
            version_lo: 0,
            pos: Bpos {
                inode: 0,
                offset,
                snapshot: 0,
            },
            value,
        }
    }

    /// Backpointer fields observed by the reference tools, checked against
    /// the extent they are supposed to describe.
    #[test]
    fn a_backpointer_value_must_agree_with_its_extent() {
        // #256, reference run 39018152705, block4k seed 1659443.
        let mut extent = key(key_type::EXTENT, 8, Vec::new());
        extent.pos.inode = 2147483766;
        extent.pos.snapshot = u32::MAX;
        extent.size = 8;
        let e = DataExtent {
            file_start: 0,
            len: 8,
            ptr: crate::extent::Ptr {
                dev: 0,
                offset: 17040,
                gen: 0,
                flags: 0,
            },
            crc: None,
            skip: 0,
        };
        let mut value = vec![0, 0, DATA_USER, 0, 0, 0, 0, 0];
        value.extend_from_slice(&8u32.to_le_bytes());
        value.extend_from_slice(&u32::MAX.to_le_bytes());
        value.extend_from_slice(&8u64.to_le_bytes());
        value.extend_from_slice(&2147483766u64.to_le_bytes());
        let bp = key(KEY_BACKPOINTER, 1116733440, value);
        let findings = |bp: &Bkey, e: &DataExtent| {
            let mut r = Report::default();
            data_backpointer(&extent, e, &BTreeMap::from([(bp.pos.offset, bp)]), &mut r);
            r.problems
        };
        assert!(findings(&bp, &e).is_empty());
        // The exact mutation changes length and snapshot in one word.
        let mut bad = bp.clone();
        bad.value[8..16].copy_from_slice(&1u64.to_le_bytes());
        let problems = findings(&bad, &e);
        assert!(problems
            .iter()
            .any(|p| p.kind == "backpointer_to_missing_ptr"
                && p.detail.contains(&extent.pos.to_string())));
        // The target snapshot alone is not compared: the reference judged
        // that change clean (run 39094186901, #348).
        let mut snapshot = bp.clone();
        snapshot.value[12..16].copy_from_slice(&0u32.to_le_bytes());
        assert!(findings(&snapshot, &e).is_empty());
        // Each documented field, and every short value, is checked safely.
        for at in [0, 1, 2, 8, 16, 24] {
            let mut bad = bp.clone();
            bad.value[at] ^= 1;
            assert_eq!(
                findings(&bad, &e)[0].kind,
                "backpointer_to_missing_ptr",
                "byte {at}"
            );
        }
        for len in 0..32 {
            let mut bad = bp.clone();
            bad.value.truncate(len);
            assert_eq!(
                findings(&bad, &e)[0].kind,
                "backpointer_to_missing_ptr",
                "length {len}"
            );
        }
        let mut missing = Report::default();
        data_backpointer(&extent, &e, &BTreeMap::new(), &mut missing);
        assert_eq!(missing.problems[0].kind, "missing_backpointer");

        // Whole compressed regions use their physical length; an
        // uncompressed piece uses its live length and offset in the region.
        let mut e = e;
        e.crc = Some(crate::extent::Crc {
            compressed_size: 2,
            uncompressed_size: 8,
            offset: 0,
            nonce: 0,
            csum_type: 5,
            compression_type: crate::extent::compression::LZ4,
            csum_hi: 0,
            csum_lo: 0,
        });
        let mut compressed = bp.clone();
        compressed.value[8..12].copy_from_slice(&2u32.to_le_bytes());
        assert!(findings(&compressed, &e).is_empty());
        let c = e.crc.as_mut().unwrap();
        c.compression_type = crate::extent::compression::NONE;
        c.offset = 136;
        c.uncompressed_size = 256;
        let mut piece = bp.clone();
        piece.pos.offset |= 136;
        assert!(findings(&piece, &e).is_empty());
        // The existing exclusions stay excluded.
        e.crc.as_mut().unwrap().compression_type = crate::extent::compression::LZ4;
        assert!(findings(&bad, &e).is_empty());
        e.crc = None;
        e.ptr.flags = crate::extent::PTR_UNWRITTEN;
        assert!(findings(&bad, &e).is_empty());
    }

    /// A snapshot tree's value: master subvolume, then root snapshot.
    fn tree(subvol: u32, root: u32) -> Vec<u8> {
        [subvol.to_le_bytes(), root.to_le_bytes()].concat()
    }

    /// A snapshot's value naming `tree`, with `skiplist`.
    fn snapshot(tree: u32, skiplist: [u32; 3]) -> Vec<u8> {
        let mut v = vec![0u8; SNAP_SKIPLIST + 12];
        v[SNAP_TREE..SNAP_TREE + 4].copy_from_slice(&tree.to_le_bytes());
        for (i, s) in skiplist.iter().enumerate() {
            let at = SNAP_SKIPLIST + 4 * i;
            v[at..at + 4].copy_from_slice(&s.to_le_bytes());
        }
        v
    }

    /// The kinds `snapshot_links` reports for these btrees.
    fn kinds(
        trees: &[(u32, Vec<u8>)],
        snapshots: &[(u32, Vec<u8>)],
        parents: &[(u32, u32)],
        subvolumes: &[u32],
    ) -> Vec<&'static str> {
        let mut btrees = BTreeMap::new();
        btrees.insert(
            SNAPSHOT_TREES,
            trees
                .iter()
                .map(|(id, v)| key(KEY_SNAPSHOT_TREE, u64::from(*id), v.clone()))
                .collect(),
        );
        btrees.insert(
            SNAPSHOTS,
            snapshots
                .iter()
                .map(|(id, v)| key(KEY_SNAPSHOT, u64::from(*id), v.clone()))
                .collect(),
        );
        let parents = parents.iter().copied().collect();
        let subvolumes = subvolumes.iter().map(|&s| (s, (0, 0))).collect();
        let mut r = Report::default();
        snapshot_links(&btrees, &parents, &subvolumes, &mut r);
        r.problems.iter().map(|p| p.kind).collect()
    }

    /// Tree 1 of subvolume 1, rooted at 100, with 99 under it; 99's
    /// skiplist names its parent.
    #[test]
    fn consistent_snapshot_links_are_clean() {
        let clean = kinds(
            &[(1, tree(1, 100))],
            &[(100, snapshot(1, [0; 3])), (99, snapshot(1, [100, 0, 0]))],
            &[(100, 0), (99, 100)],
            &[1],
        );
        assert!(clean.is_empty(), "{clean:?}");
        // Without a snapshot_trees btree there is nothing to link.
        let mut r = Report::default();
        snapshot_links(&BTreeMap::new(), &BTreeMap::new(), &BTreeMap::new(), &mut r);
        assert!(r.problems.is_empty());
    }

    #[test]
    fn a_snapshot_key_with_a_nonzero_inode_is_named() {
        // Reference run 39018152705 rejects the root snapshot at
        // U64_MAX:U32_MAX:0 as snapshot_pos_bad (#253).
        for inode in [0, 1, u64::MAX] {
            let mut k = key(KEY_SNAPSHOT, u64::from(u32::MAX), snapshot(1, [0; 3]));
            k.pos.inode = inode;
            let mut report = Report::default();
            check_key_fields(SNAPSHOTS, &k, &mut report);
            if inode == 0 {
                assert!(report.clean(), "{report:?}");
            } else {
                assert_eq!(report.problems.len(), 1, "{report:?}");
                assert_eq!(report.problems[0].kind, "snapshot_pos_bad");
                assert!(report.problems[0].detail.contains(&k.pos.to_string()));
            }

            let mut unrelated = Report::default();
            check_key_fields(SNAPSHOT_TREES, &k, &mut unrelated);
            assert!(unrelated.clean(), "{unrelated:?}");
        }
    }

    /// The logged_ops inode allocation cursor, at 1:1:0 (S8).
    const KEY_INODE_ALLOC_CURSOR: u8 = 35;

    fn cursor(inode: u64, snapshot: u32) -> Bkey {
        let mut k = key(KEY_INODE_ALLOC_CURSOR, 1, vec![0; 16]);
        k.pos.inode = inode;
        k.pos.snapshot = snapshot;
        k
    }

    #[test]
    fn an_inode_alloc_cursor_at_a_snapshot_is_named() {
        // Nightly differential run 39756419740 (seed 1659898): the reference
        // refuses the cursor at 1:1:4294967294 as "nonzero snapshot" (#376).
        let mut clean = Report::default();
        check_key_fields(LOGGED_OPS, &cursor(1, 0), &mut clean);
        assert!(clean.clean(), "{clean:?}");

        let k = cursor(1, u32::MAX - 1);
        let mut report = Report::default();
        check_key_fields(LOGGED_OPS, &k, &mut report);
        assert_eq!(report.problems.len(), 1, "{report:?}");
        assert_eq!(report.problems[0].kind, "bkey_snapshot_nonzero");
        assert!(report.problems[0].detail.contains(&k.pos.to_string()));
    }

    #[test]
    fn an_inode_alloc_cursor_off_inode_1_is_named() {
        // The same run refuses the cursor moved to inode U64_MAX (seed
        // 1659851) and to inode 0 (seed 1659879) as "k.p.inode bad" (#376).
        for inode in [0, u64::MAX] {
            let k = cursor(inode, 0);
            let mut report = Report::default();
            check_key_fields(LOGGED_OPS, &k, &mut report);
            assert_eq!(report.problems.len(), 1, "{report:?}");
            assert_eq!(report.problems[0].kind, "inode_alloc_cursor_inode_bad");
            assert!(report.problems[0].detail.contains(&k.pos.to_string()));

            let mut unrelated = Report::default();
            check_key_fields(SNAPSHOT_TREES, &k, &mut unrelated);
            assert!(unrelated.clean(), "{unrelated:?}");
        }
    }

    #[test]
    fn a_snapshot_tree_key_with_a_nonzero_inode_is_named() {
        // Reference run 39018152705 rejects tree 1 at 1:1:0 as
        // snapshot_tree_pos_bad (#254); offset 0 was already refused.
        for inode in [0, 1, u64::MAX] {
            for offset in [0, 1] {
                let mut k = key(KEY_SNAPSHOT_TREE, offset, tree(1, u32::MAX));
                k.pos.inode = inode;
                let mut report = Report::default();
                check_key_fields(SNAPSHOT_TREES, &k, &mut report);
                if inode == 0 && offset != 0 {
                    assert!(report.clean(), "{report:?}");
                } else {
                    assert_eq!(report.problems.len(), 1, "{report:?}");
                    assert_eq!(report.problems[0].kind, "snapshot_tree_pos_bad");
                    assert!(report.problems[0].detail.contains(&k.pos.to_string()));
                }

                let mut unrelated = Report::default();
                check_key_fields(SNAPSHOTS, &k, &mut unrelated);
                assert!(unrelated.clean(), "{unrelated:?}");
                k.key_type = KEY_SUBVOLUME;
                check_key_fields(SNAPSHOT_TREES, &k, &mut unrelated);
                assert!(unrelated.clean(), "{unrelated:?}");
            }
        }
    }

    #[test]
    fn a_snapshot_tree_key_with_an_offset_past_u32_max_is_named() {
        // Nightly run 38878947701, kernel seed 1659362 (#219): the reference
        // rejects a snapshot tree at 0:U64_MAX:0 as snapshot_tree_pos_bad.
        let past = u64::from(u32::MAX) + 1;
        for offset in [1, u64::from(u32::MAX), past, u64::MAX] {
            let k = key(KEY_SNAPSHOT_TREE, offset, tree(1, u32::MAX));
            let mut report = Report::default();
            check_key_fields(SNAPSHOT_TREES, &k, &mut report);
            if offset <= u64::from(u32::MAX) {
                assert!(report.clean(), "{report:?}");
            } else {
                assert_eq!(report.problems.len(), 1, "{report:?}");
                assert_eq!(report.problems[0].kind, "snapshot_tree_pos_bad");
                assert!(report.problems[0].detail.contains(&k.pos.to_string()));
            }

            let mut unrelated = Report::default();
            check_key_fields(SNAPSHOTS, &k, &mut unrelated);
            assert!(unrelated.clean(), "{unrelated:?}");
        }
    }

    #[test]
    fn an_xattr_key_with_a_nonzero_size_is_named() {
        // Run 39018152705, aged seed 1659486 (#258): size 1 on this
        // xattr is refused by the reference as bkey_size_nonzero.
        for size in [0, 1, u32::MAX] {
            let mut k = key(key_type::XATTR, 2679498547732338290, vec![0; 8]);
            k.pos.inode = 2147487679;
            k.pos.snapshot = u32::MAX;
            k.size = size;
            let mut report = Report::default();
            check_key_fields(btree_id::XATTRS, &k, &mut report);
            if size == 0 {
                assert!(report.clean(), "{report:?}");
            } else {
                assert_eq!(report.problems.len(), 1, "{report:?}");
                assert_eq!(report.problems[0].kind, "bkey_size_nonzero");
                assert!(report.problems[0].detail.contains(&k.pos.to_string()));
                assert!(report.problems[0].detail.contains(&format!("size {size}")));
            }

            let mut unrelated = Report::default();
            check_key_fields(btree_id::EXTENTS, &k, &mut unrelated);
            assert!(unrelated.clean(), "{unrelated:?}");
            k.key_type = key_type::EXTENT;
            check_key_fields(btree_id::XATTRS, &k, &mut unrelated);
            assert!(unrelated.clean(), "{unrelated:?}");
        }
    }

    #[test]
    fn an_inode_key_with_a_nonzero_inode_position_is_named() {
        // Run 39018152705, seed 1659490 (#255): the reference rejects
        // inode_v3 at U64_MAX:2147483849:U32_MAX, leaving its value intact.
        for inode in [0, 1, u64::MAX] {
            let mut k = key(key_type::INODE_V3, 2147483849, Vec::new());
            k.pos.inode = inode;
            k.pos.snapshot = u32::MAX;
            let mut report = Report::default();
            check_key_fields(btree_id::INODES, &k, &mut report);
            if inode == 0 {
                assert!(report.clean(), "{report:?}");
            } else {
                assert_eq!(report.problems.len(), 1, "{report:?}");
                assert_eq!(report.problems[0].kind, "inode_pos_inode_nonzero");
                assert!(report.problems[0].detail.contains(&k.pos.to_string()));
            }

            let mut unrelated = Report::default();
            check_key_fields(btree_id::DIRENTS, &k, &mut unrelated);
            assert!(unrelated.clean(), "{unrelated:?}");
            k.key_type = key_type::DIRENT;
            check_key_fields(btree_id::INODES, &k, &mut unrelated);
            assert!(unrelated.clean(), "{unrelated:?}");
        }
    }

    #[test]
    fn each_broken_snapshot_link_is_named() {
        let parents = [(100, 0), (99, 100)];
        let snaps = |skip: u32| [(100, snapshot(1, [0; 3])), (99, snapshot(1, [skip, 0, 0]))];
        // A tree whose master subvolume the subvolumes btree lacks.
        assert_eq!(
            kinds(&[(1, tree(7, 100))], &snaps(100), &parents, &[1]),
            ["snapshot_tree_to_missing_subvol"]
        );
        // A skiplist entry that is not an ancestor.
        assert_eq!(
            kinds(&[(1, tree(1, 100))], &snaps(42), &parents, &[1]),
            ["snapshot_bad_skiplist"]
        );
        // A snapshot naming a tree the btree does not have: tree 1's root
        // is then in another tree as well (#267).
        assert_eq!(
            kinds(
                &[(1, tree(1, 100))],
                &[(100, snapshot(2, [0; 3]))],
                &[(100, 0)],
                &[1]
            ),
            [
                "snapshot_tree_to_bad_root_snapshot",
                "snapshot_to_bad_snapshot_tree"
            ]
        );
        // A tree whose root is not the root the snapshot's parents reach:
        // the snapshots btree lacks the tree's root, 100 (#267).
        assert_eq!(
            kinds(
                &[(1, tree(1, 100))],
                &[(50, snapshot(1, [0; 3]))],
                &[(50, 0)],
                &[1]
            ),
            [
                "snapshot_tree_to_bad_root_snapshot",
                "snapshot_to_bad_snapshot_tree"
            ]
        );
        // A parent cycle reaches no root, and the tree's root, 100, is not
        // in the snapshots btree (#267).
        assert_eq!(
            kinds(
                &[(1, tree(1, 100))],
                &[(98, snapshot(1, [0; 3])), (99, snapshot(1, [0; 3]))],
                &[(98, 99), (99, 98)],
                &[1]
            ),
            [
                "snapshot_tree_to_bad_root_snapshot",
                "snapshot_to_bad_snapshot_tree",
                "snapshot_to_bad_snapshot_tree"
            ]
        );
    }

    /// Trees 1 and 2, rooted at 100 and 200, of subvolumes 1 and 2 (#265).
    /// Subvolume 2 is the master of tree 2 only while its snapshot is in it.
    #[test]
    fn a_master_subvolume_in_another_tree_is_named() {
        let mut btrees = BTreeMap::new();
        btrees.insert(
            SNAPSHOT_TREES,
            vec![
                key(KEY_SNAPSHOT_TREE, 1, tree(1, 100)),
                key(KEY_SNAPSHOT_TREE, 2, tree(2, 200)),
            ],
        );
        btrees.insert(
            SNAPSHOTS,
            vec![
                key(KEY_SNAPSHOT, 100, snapshot(1, [0; 3])),
                key(KEY_SNAPSHOT, 200, snapshot(2, [0; 3])),
            ],
        );
        let parents = [(100, 0), (200, 0)].into_iter().collect();
        let report = |subvol2_snap: u32| {
            let subvolumes = [(1, (0, 100)), (2, (0, subvol2_snap))]
                .into_iter()
                .collect();
            let mut r = Report::default();
            snapshot_links(&btrees, &parents, &subvolumes, &mut r);
            r.problems
        };
        assert!(report(200).is_empty());
        let wrong = report(100);
        assert_eq!(wrong.len(), 1, "{wrong:?}");
        assert_eq!(wrong[0].kind, "snapshot_tree_to_wrong_subvol");
        assert!(
            wrong[0].detail.contains("snapshot tree 2"),
            "{}",
            wrong[0].detail
        );
        assert!(wrong[0].detail.contains("tree 1"), "{}", wrong[0].detail);
    }
}
