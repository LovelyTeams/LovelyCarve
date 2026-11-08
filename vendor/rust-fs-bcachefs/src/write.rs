//! Writing, behind the `write` feature: new keys go into the btrees by
//! appending a bset to the leaf that covers them, and each node's new length
//! is carried up to the root, whose key lives in the superblock.
//!
//! Provenance (docs/clean-room.md, "The write path"). Everything here
//! repeats what the reference implementation was observed to do in the
//! write study's before/after pairs (S8): creating a file appended one bset
//! to the dirents leaf, with the node's seq, a newer journal sequence, flags
//! carrying the checksum type in the low 4 bits and the bset's start in
//! sectors from bit 16, and raised the `sectors_written` of the root key in
//! the superblock's clean field. The reference checker judges every image
//! this writes (tests in the guest).
//!
//! NOT CRASH-SAFE: nodes are written, then their parents, then the
//! superblock. An interruption leaves bsets nothing points past; the
//! reference checker would find them. Committing through the journal is
//! issue #25.

use crate::bkey::{self, key_type, Bkey, BkeyFormat, Bpos};
use crate::btree::{self, Node, NodePtr};
use crate::error::{Error, Result};
use crate::superblock::{Superblock, FIELD_CLEAN};
use crate::util::{le16, le32, le64};
use fs_core::BlockDevice;

#[path = "write_alloc.rs"]
mod alloc;
#[path = "write_journal.rs"]
mod journal_commit;
#[path = "write_nodes.rs"]
mod nodes;

pub(crate) const SB_HEADER_BYTES: usize = 0x2f0;
const HEADER_BSET: usize = 136;
const BSET_HEADER: usize = 24;

/// The bytes of an unpacked key: u64s, format 1, type, pad, version_lo u64,
/// version_hi u32, size u32, bpos (snapshot u32, offset u64, inode u64),
/// then the value, which must be a whole number of u64s.
pub fn encode_key(k: &Bkey) -> Result<Vec<u8>> {
    if !k.value.len().is_multiple_of(8) {
        return Err(Error::Corrupt("a key's value must be whole u64s".into()));
    }
    let u64s = 5 + k.value.len() / 8;
    let u64s_byte =
        u8::try_from(u64s).map_err(|_| Error::Unsupported(format!("a key of {u64s} u64s")))?;
    let mut b = Vec::with_capacity(u64s * 8);
    b.extend_from_slice(&[u64s_byte, 1, k.key_type, 0]);
    b.extend_from_slice(&k.version_lo.to_le_bytes());
    b.extend_from_slice(&k.version_hi.to_le_bytes());
    b.extend_from_slice(&k.size.to_le_bytes());
    b.extend_from_slice(&k.pos.snapshot.to_le_bytes());
    b.extend_from_slice(&k.pos.offset.to_le_bytes());
    b.extend_from_slice(&k.pos.inode.to_le_bytes());
    b.extend_from_slice(&k.value);
    Ok(b)
}

const UNPACKED: BkeyFormat = BkeyFormat {
    key_u64s: 5,
    nr_fields: 6,
    bits: [0; 6],
    field_offset: [0; 6],
};

/// A btree_ptr_v2 key with its `sectors_written` replaced (value bytes
/// 16..18, see `NodePtr::parse`).
fn with_sectors_written(k: &Bkey, sectors: u16) -> Result<Bkey> {
    if k.key_type != key_type::BTREE_PTR_V2 || k.value.len() < 40 {
        return Err(Error::Corrupt("not a btree_ptr_v2 key".into()));
    }
    let mut k = k.clone();
    k.value[16..18].copy_from_slice(&sectors.to_le_bytes());
    Ok(k)
}

/// A writer over one clean, single-device, unencrypted filesystem.
pub struct Writer<D: BlockDevice> {
    dev: D,
    sb: Superblock,
    /// The selected superblock copy's bytes, header and fields.
    sb_raw: Vec<u8>,
    /// The journal sequence number new bsets carry: the one the clean
    /// field records as the last.
    journal_seq: u64,
    /// Set by [`Writer::journal_commits`]: commits go to the journal.
    session: Option<journal_commit::Session>,
    /// Buckets taken during this writer's life, which the freespace btree
    /// may not show as taken yet.
    reserved: std::collections::BTreeSet<u64>,
    /// The nodes the cursor has read: roots and interior nodes are read
    /// once per writer rather than once per lookup (#108).
    cache: btree::NodeCache,
}

impl<D: BlockDevice> Writer<D> {
    pub fn open(dev: D) -> Result<Self> {
        let sb = Superblock::read(&dev)?;
        if !sb.is_clean() {
            return Err(Error::Unsupported(
                "the filesystem was not cleanly unmounted: writing in place needs a clean one \
                 (Writer::open_journalled continues its journal)"
                    .into(),
            ));
        }
        Self::open_any(dev, sb)
    }

    /// A writer whatever the clean bit says; the caller decides what an
    /// unclean filesystem allows.
    fn open_any(dev: D, sb: Superblock) -> Result<Self> {
        if sb.field(2).is_some() || sb.nr_devices != 1 {
            return Err(Error::Unsupported(
                "writing needs a single-device, unencrypted filesystem".into(),
            ));
        }
        // Inode times are stored in units of the superblock's time
        // precision; this writer stamps nanoseconds, which is right only
        // when the precision is 1 (S8: every fixture; open question).
        if sb.time_precision != 1 {
            return Err(Error::Unsupported(format!(
                "time precision {} is not nanoseconds: the times this writer stamps would be wrong",
                sb.time_precision
            )));
        }
        let len = SB_HEADER_BYTES + sb.u64s as usize * 8;
        let mut sb_raw = vec![0u8; len];
        let at = sb
            .offset
            .checked_mul(512)
            .ok_or_else(|| Error::Corrupt(format!("superblock sector {} overflows", sb.offset)))?;
        dev.read_at(at, &mut sb_raw)?;
        let clean = sb
            .field(FIELD_CLEAN)
            .ok_or_else(|| Error::Corrupt("a clean filesystem with no clean field".into()))?;
        if clean.body.len() < 16 {
            return Err(Error::Corrupt("clean field shorter than its header".into()));
        }
        let journal_seq = le64(&clean.body, 8);
        Ok(Writer {
            dev,
            sb,
            sb_raw,
            journal_seq,
            session: None,
            reserved: Default::default(),
            cache: Default::default(),
        })
    }

    /// Every key this writer makes is at snapshot u32::MAX, the root
    /// subvolume's until it is snapshotted. After that the root subvolume
    /// moves to a child snapshot and the keys at u32::MAX are shared with
    /// its snapshot, so a write there would change the snapshot too (#138):
    /// refused, before anything is committed: not at open, where a
    /// journalled session's replay is not laid over the nodes yet.
    fn refuse_a_snapshotted_root(&self) -> Result<()> {
        let root = self
            .keys_from(
                ids::SUBVOLUMES,
                Bpos {
                    inode: 0,
                    offset: ids::ROOT_SUBVOLUME,
                    snapshot: 0,
                },
            )?
            .into_iter()
            .find(|k| {
                k.pos.offset == ids::ROOT_SUBVOLUME
                    && k.key_type == ids::SUBVOLUME
                    && k.value.len() >= 16
            });
        match root.map(|k| (le64(&k.value, 0) >> 32) as u32) {
            // A filesystem with no subvolumes btree is not snapshotted.
            None | Some(u32::MAX) => Ok(()),
            Some(snap) => Err(Error::Unsupported(format!(
                "the root subvolume is at snapshot {snap}, not {}: it has been snapshotted, and \
                 this writer writes only at {}, which its snapshot shares",
                u32::MAX,
                u32::MAX
            ))),
        }
    }

    pub fn superblock(&self) -> &Superblock {
        &self.sb
    }

    /// The device this writer writes to.
    pub fn device(&self) -> &D {
        &self.dev
    }

    /// Insert `keys` into btree `id`: each replaces the key at its position
    /// (a key of type `deleted` removes it). Keys going to the same leaf are
    /// appended as one bset; the new lengths are carried up to the root. A
    /// node that is full is rewritten -- compacted, or split in two -- in
    /// fresh buckets.
    pub fn insert(&mut self, id: u8, keys: Vec<Bkey>) -> Result<()> {
        let mut t = Txn::default();
        t.keys.insert(id, keys);
        self.commit_in_place(t)
    }

    /// Rewrite btree `id`'s leaf root as one compact node holding only its
    /// live keys: removed keys and their whiteouts, which the reference
    /// validates bset by bset, are dropped. The old node's bucket is freed
    /// and the accounting the rewrite adds is committed.
    pub fn compact_root(&mut self, id: u8) -> Result<()> {
        let (level, root_key) = self.root(id)?;
        if level != 0 {
            return Err(Error::Corrupt(format!(
                "btree {id}'s root is at level {level}, not a leaf"
            )));
        }
        let mut t = Txn::default();
        let new = self.rewrite_node(id, level, &root_key, &[], &mut t)?;
        let [new] = new.as_slice() else {
            return Err(Error::Corrupt(format!(
                "btree {id}'s root split while compacting"
            )));
        };
        self.set_root(id, level, new)?;
        self.write_superblock()?;
        self.commit_in_place(t)
    }

    /// Insert into one btree, collecting what rewriting nodes adds to other
    /// btrees and to accounting in `t`.
    fn insert_one(&mut self, id: u8, mut keys: Vec<Bkey>, t: &mut Txn) -> Result<()> {
        if keys.is_empty() {
            // An empty bset is one the reference checker rejects ("empty
            // bset"): nothing to add is nothing to write.
            return Ok(());
        }
        keys.sort_by_key(|k| k.pos);
        // A later key at the same position replaces an earlier one.
        let mut deduped: Vec<Bkey> = Vec::with_capacity(keys.len());
        for k in keys {
            match deduped.last_mut() {
                Some(last) if last.pos == k.pos => *last = k,
                _ => deduped.push(k),
            }
        }
        let (root_level, root_key) = self.root(id)?;
        let new = self.insert_at(id, &root_key, root_level, &deduped, t)?;
        if new.len() == 1 {
            self.set_root(id, root_level, &new[0])?;
        } else {
            // The root split: a new root one level up holds both halves.
            let root = self.new_node(
                id,
                root_level + 1,
                Bpos::default(),
                nodes::SPOS_MAX,
                &new,
                t,
            )?;
            self.set_root(id, root_level + 1, &root)?;
        }
        self.write_superblock()
    }

    /// Insert into the subtree under `ptr_key` (at `level`); returns the
    /// pointer keys that now stand for it: one, with its new
    /// `sectors_written` or its new place; or two, when it split.
    fn insert_at(
        &mut self,
        id: u8,
        ptr_key: &Bkey,
        level: u8,
        keys: &[Bkey],
        t: &mut Txn,
    ) -> Result<Vec<Bkey>> {
        let ptr = NodePtr::from_key(ptr_key)?;
        let own = if level == 0 {
            keys.to_vec()
        } else {
            let node = btree::read_node(&self.dev, &self.sb, &ptr)?;
            // Each key goes to the first child whose pointer position (its
            // max key) is at or after it.
            let mut updated = Vec::new();
            let mut rest = keys;
            for child in node
                .keys
                .iter()
                .filter(|k| k.key_type == key_type::BTREE_PTR_V2)
            {
                let n = rest.iter().take_while(|k| k.pos <= child.pos).count();
                if n > 0 {
                    updated.extend(self.insert_at(id, child, level - 1, &rest[..n], t)?);
                    rest = &rest[n..];
                }
            }
            if !rest.is_empty() {
                return Err(Error::Corrupt(format!(
                    "key {} is past the last child of an interior node",
                    rest[0].pos
                )));
            }
            updated
        };
        match self.append_bset(&ptr, &own)? {
            Some(written) => Ok(vec![with_sectors_written(ptr_key, written)?]),
            None => self.rewrite_node(id, level, ptr_key, &own, t),
        }
    }

    /// Append one bset holding `keys` to the node; returns the node's new
    /// `sectors_written`, or `None` when it does not fit.
    fn append_bset(&self, ptr: &NodePtr, keys: &[Bkey]) -> Result<Option<u16>> {
        let block = (self.sb.block_size as usize * 512).max(512);
        let node_bytes = self.sb.btree_node_size() as usize * 512;
        let written = ptr.sectors_written as usize * 512;
        let mut node = vec![0u8; written];
        let at = ptr.ptrs[0].offset * 512;
        self.dev.read_at(at, &mut node)?;
        // The node must parse as it stands before anything is added to it.
        Node::parse(
            &node,
            btree::node_magic(&self.sb.uuid),
            block,
            Some(ptr.seq),
        )?;
        let first_flags = le32(&node, HEADER_BSET + 16);
        let version = le16(&node, HEADER_BSET + 20);
        let csum_type = (first_flags & 0xf) as u8;

        let start = written.div_ceil(block) * block;
        let mut body = Vec::new();
        for k in keys {
            body.extend(encode_key(k)?);
        }
        let Ok(u64s) = u16::try_from(body.len() / 8) else {
            return Ok(None);
        };
        let sector = u32::try_from(start / 512)
            .map_err(|_| Error::Corrupt("node offset overflows".into()))?;
        let mut rec = vec![0u8; 16 + BSET_HEADER];
        rec[16..24].copy_from_slice(&ptr.seq.to_le_bytes());
        rec[24..32].copy_from_slice(&self.journal_seq.to_le_bytes());
        rec[32..36].copy_from_slice(&(u32::from(csum_type) | sector << 16).to_le_bytes());
        rec[36..38].copy_from_slice(&version.to_le_bytes());
        rec[38..40].copy_from_slice(&u64s.to_le_bytes());
        rec.extend_from_slice(&body);
        let csum = crate::csum::compute(csum_type, &rec[16..])?;
        rec[0..8].copy_from_slice(&csum.to_le_bytes());
        let padded = rec.len().div_ceil(block) * block;
        if start + padded > node_bytes {
            return Ok(None);
        }
        rec.resize(padded, 0);
        self.dev.write_at(at + start as u64, &rec)?;
        u16::try_from((start + padded) / 512)
            .map(Some)
            .map_err(|_| Error::Corrupt("sectors_written overflows".into()))
    }

    /// The root of btree `id` from the clean field: its level and key.
    fn root(&self, id: u8) -> Result<(u8, Bkey)> {
        if let Some(s) = &self.session {
            return s
                .roots
                .get(&id)
                .cloned()
                .ok_or_else(|| Error::NotFound(format!("btree {id} has no root")));
        }
        let r = self
            .sb
            .btree_roots()?
            .into_iter()
            .find(|r| r.btree_id == id)
            .ok_or_else(|| Error::NotFound(format!("btree {id} has no root")))?;
        Ok((r.level, bkey::decode(&r.key, &UNPACKED)?))
    }

    /// Replace btree `id`'s root key in the clean field (same length).
    fn set_root(&mut self, id: u8, level: u8, key: &Bkey) -> Result<()> {
        let bytes = encode_key(key)?;
        let (off, len) = self.clean_root_span(id)?;
        if len != bytes.len() {
            return Err(Error::Corrupt("the new root key's length differs".into()));
        }
        self.sb_raw[off..off + len].copy_from_slice(&bytes);
        // The entry's level byte: u16 u64s, btree id, level.
        self.sb_raw[off - 8 + 3] = level;
        self.sb = Superblock::parse_unchecked(&self.sb_raw)?;
        Ok(())
    }

    /// Where btree `id`'s root key sits in `sb_raw`.
    fn clean_root_span(&self, id: u8) -> Result<(usize, usize)> {
        let mut p = SB_HEADER_BYTES;
        while p + 8 <= self.sb_raw.len() {
            let u64s = le32(&self.sb_raw, p) as usize;
            let ty = le32(&self.sb_raw, p + 4);
            let end = p + u64s * 8;
            if u64s == 0 || end > self.sb_raw.len() {
                break;
            }
            if ty == FIELD_CLEAN {
                let mut e = p + 8 + 16;
                while e + 8 <= end {
                    let n = le16(&self.sb_raw, e) as usize;
                    let (btree_id, entry_type) = (self.sb_raw[e + 2], self.sb_raw[e + 4]);
                    if entry_type == 1 && btree_id == id && n > 0 {
                        return Ok((e + 8, n * 8));
                    }
                    e += 8 + n * 8;
                }
            }
            p = end;
        }
        Err(Error::NotFound(format!(
            "btree {id}'s root in the clean field"
        )))
    }

    /// Checksum the superblock and write it to every location its layout
    /// names.
    fn write_superblock(&mut self) -> Result<()> {
        let csum = crate::csum::compute(self.sb.csum_type(), &self.sb_raw[16..])?;
        self.sb_raw[0..16].fill(0);
        self.sb_raw[0..8].copy_from_slice(&csum.to_le_bytes());
        self.sb = Superblock::parse(&self.sb_raw)?;
        for &sector in &self.sb.layout.sb_offsets {
            let mut copy = self.sb_raw.clone();
            // Each copy records its own offset.
            copy[0x68..0x70].copy_from_slice(&sector.to_le_bytes());
            let csum = crate::csum::compute(self.sb.csum_type(), &copy[16..])?;
            copy[0..16].fill(0);
            copy[0..8].copy_from_slice(&csum.to_le_bytes());
            self.dev.write_at(sector * 512, &copy)?;
        }
        self.dev.flush()?;
        Ok(())
    }

    /// Give the device back.
    pub fn into_inner(self) -> D {
        self.dev
    }
}

/// Where a key lands: the position of a dirent, an inode, an extent.
pub fn pos(inode: u64, offset: u64) -> Bpos {
    Bpos {
        inode,
        offset,
        snapshot: u32::MAX,
    }
}

use crate::inode::INODE_HAS_OPTIONS;
/// The options a new inode takes from its directory (#142).
use crate::inode::OPTION_FIELDS as INHERITED_OPTIONS;

/// The data checksum and compression a file's data is written with, as
/// superblock option numbers (crc32c 1, ...; lz4 1, gzip 2, zstd 3): the
/// inode's own, stored plus one, or the filesystem's where it has none
/// (#142). More than one replica, which one device cannot hold, is refused.
pub(crate) fn data_options(sb: &Superblock, raw: &crate::inode::InodeV3Raw) -> Result<(u64, u64)> {
    let f = |name: &str| raw.field(name).unwrap_or(0);
    if f("data_replicas") > 2 {
        return Err(Error::Unsupported(format!(
            "the inode asks for {} replicas of its data, on one device",
            f("data_replicas") - 1
        )));
    }
    let pick = |name: &str, fs: u64| match f(name) {
        0 => fs,
        v => v - 1,
    };
    Ok((
        pick("data_checksum", u64::from(sb.data_checksum_opt())),
        pick("compression", u64::from(sb.compression_opt())),
    ))
}

/// The btrees and key types the file operations touch.
mod ids {
    pub const EXTENTS: u8 = 0;
    pub const INODES: u8 = 1;
    pub const DIRENTS: u8 = 2;
    /// The subvolumes btree (S1 11.3), its key type (S1 11.5), and the root
    /// subvolume's id (the kernel oracle's lister: `subvolume 1`).
    pub const SUBVOLUMES: u8 = 8;
    pub const SUBVOLUME: u8 = 21;
    pub const ROOT_SUBVOLUME: u64 = 1;
    pub const LOGGED_OPS: u8 = 17;
    pub const ACCOUNTING: u8 = 20;
    pub const INODE_ALLOC_CURSOR: u8 = 35;
    pub const ACCOUNTING_KEY: u8 = 34;
}

/// The longest symlink target this writer stores: the longest the aged
/// fixture showed (S8). Past what was seen is not guessed.
pub const SYMLINK_MAX: usize = 248;

/// The inline bound on 512-byte blocks, the default; kept so the public
/// API does not lose it. Other block sizes have their own bound.
#[deprecated(note = "use inline_max(block size in bytes)")]
pub const INLINE_MAX: usize = 256;

/// How much data the reference keeps inline on blocks of `block_bytes`: a
/// whole file of up to this many bytes, and, after a longer file's full
/// blocks, a final partial block of up to this many. S1 (9.1.7) gives
/// min(block size / 2, 1024); the write study's `inline-*` images show both
/// bounds are inclusive (S8: 256 on 512-byte blocks, 1024 on 4096).
pub fn inline_max(block_bytes: usize) -> usize {
    (block_bytes / 2).min(1024)
}

/// Where `len` bytes of file data go on blocks of `block_bytes`, as the
/// reference puts them (S8): `(bytes in extents, bytes inline after them)`.
/// A file of up to [`inline_max`] bytes is inline; a longer one has its
/// full blocks in extents and keeps a final partial block of up to that
/// size inline, or is all extents when the partial block is larger.
pub fn data_layout(len: usize, block_bytes: usize) -> (usize, usize) {
    let max = inline_max(block_bytes);
    if len <= max {
        return (0, len);
    }
    let tail = len % block_bytes;
    let extents = if tail != 0 && tail <= max {
        len - tail
    } else {
        len
    };
    (extents, len - extents)
}

/// Varint positions in an `inode_v3`, as the lister orders the fields
/// (the four times take two varints each).
mod field {
    pub const ATIME: usize = 0;
    pub const CTIME: usize = 2;
    pub const MTIME: usize = 4;
    pub const OTIME: usize = 6;
    pub const UID: usize = 8;
    pub const GID: usize = 9;
    pub const NLINK: usize = 10;
    pub const DEPTH: usize = 28;
    /// Fields stored by a directory the reference created.
    pub const DIR_FIELDS: u64 = 25;
    pub const DIR: usize = 23;
    pub const DIR_OFFSET: usize = 24;
    /// Fields stored by a regular file the reference created.
    pub const FILE_FIELDS: u64 = 21;
}

/// The accounting key counting the keys and bytes a btree holds in the
/// all-ones snapshot: kind 5 in the top byte, the snapshot id, the btree id
/// at bits 16..24 (S8: `snapshot id=4294967295 btree=NAME` at
/// 0x05ffffffff000000 + id << 16; its value is keys, bytes, 0).
fn btree_counter_pos(id: u8) -> Bpos {
    Bpos {
        inode: 0x05ff_ffff_ff00_0000 | (u64::from(id) << 16),
        offset: 0,
        snapshot: 0,
    }
}

fn unpacked_bytes(k: &Bkey) -> i64 {
    (40 + k.value.len()) as i64
}

/// What one operation changes: keys per btree (a key of type `deleted`
/// removes the one at its position), and the accounting deltas they imply.
#[derive(Default)]
struct Txn {
    keys: std::collections::BTreeMap<u8, Vec<Bkey>>,
    /// Accounting: per accounting key position, a delta per counter.
    acct: std::collections::BTreeMap<Bpos, Vec<i64>>,
    /// The keys being inserted right now, out of `keys`: a read-modify-write
    /// made while they are (freeing a node of the same btree) must see them.
    inflight: Option<(u8, Vec<Bkey>)>,
}

impl Txn {
    /// Add `delta` to counter `i` of the accounting key at `pos`, which
    /// holds `n` counters.
    fn count(&mut self, pos: Bpos, n: usize, i: usize, delta: i64) {
        let v = self.acct.entry(pos).or_insert_with(|| vec![0; n]);
        if v.len() < n {
            v.resize(n, 0);
        }
        v[i] += delta;
    }

    /// The per-btree counter: keys, bytes, data sectors.
    fn count_key(&mut self, id: u8, key: &Bkey, sign: i64) {
        self.count(btree_counter_pos(id), 3, 0, sign);
        self.count(btree_counter_pos(id), 3, 1, sign * unpacked_bytes(key));
    }

    /// Add `new` where `old` was (either may be absent).
    fn put(&mut self, id: u8, old: Option<&Bkey>, new: Bkey) {
        if let Some(o) = old {
            self.count_key(id, o, -1);
        }
        self.count_key(id, &new, 1);
        self.keys.entry(id).or_default().push(new);
    }

    /// A key in a btree the per-btree counters do not cover (alloc,
    /// freespace, backpointers, lru, logged_ops).
    fn put_uncounted(&mut self, id: u8, new: Bkey) {
        self.keys.entry(id).or_default().push(new);
    }

    /// Remove `old`.
    fn delete(&mut self, id: u8, old: &Bkey) {
        self.count_key(id, old, -1);
        self.delete_uncounted(id, old.pos);
    }

    fn delete_uncounted(&mut self, id: u8, at: Bpos) {
        self.keys.entry(id).or_default().push(Bkey {
            key_type: key_type::DELETED,
            size: 0,
            version_hi: 0,
            version_lo: 0,
            pos: at,
            value: Vec::new(),
        });
    }

    fn inodes(&mut self, delta: i64) {
        self.count(Bpos::default(), 1, 0, delta);
    }
}

/// An inode as the writer holds it: its key and its decoded value.
struct InodeRef {
    key: Bkey,
    raw: crate::inode::InodeV3Raw,
}

impl InodeRef {
    fn is_dir(&self) -> bool {
        self.raw.mode() & 0o170000 == 0o040000
    }

    fn rekey(&self) -> Bkey {
        Bkey {
            value: self.raw.encode(),
            ..self.key.clone()
        }
    }
}

fn valid_name(name: &[u8]) -> Result<()> {
    if name.is_empty()
        || name.len() > 255
        || name.contains(&b'/')
        || name.contains(&0)
        || name == b"."
        || name == b".."
    {
        return Err(Error::Corrupt(format!(
            "not a valid name: {:?}",
            String::from_utf8_lossy(name)
        )));
    }
    Ok(())
}

impl<D: BlockDevice> Writer<D> {
    /// Every leaf key of btree `id`, as it stands.
    fn keys(&self, id: u8) -> Result<Vec<Bkey>> {
        match &self.session {
            Some(s) => btree::walk_replayed(&self.dev, &self.sb, id, Some(&s.replay)),
            None => btree::walk(&self.dev, &self.sb, id),
        }
    }

    /// A cursor over btree `id` (#108): it reads only the nodes on a key's
    /// path, and in a journalled session sees the replay's keys over the
    /// nodes', as `walk_replayed` does.
    fn cursor(&self, id: u8) -> Result<btree::Cursor<'_>> {
        Ok(btree::Cursor::new(
            &self.dev,
            &self.sb,
            id,
            self.session.as_ref().map(|s| &s.replay),
        )?
        // Nodes are cached by where they are, their bucket's generation and
        // how much of them is written (btree::NodeCache), so a node this
        // writer appends to, rewrites, or writes into a reused bucket is a
        // new entry: what is cached is never stale.
        .with_cache(&self.cache))
    }

    /// Every key of btree `id` from `from` on while its position's inode
    /// field is `from.inode`, through a cursor.
    fn keys_from(&self, id: u8, from: Bpos) -> Result<Vec<Bkey>> {
        let mut c = match self.cursor(id) {
            // A btree with no root yet holds nothing.
            Err(Error::NotFound(_)) if self.root(id).is_err() => return Ok(Vec::new()),
            r => r?,
        };
        c.seek(from)?;
        let mut out = Vec::new();
        while let Some(k) = c.next_key()? {
            if k.pos.inode != from.inode {
                break;
            }
            out.push(k);
        }
        Ok(out)
    }

    /// Whether directory `dir` holds a live entry: its dirents are read
    /// through a cursor up to the first, not all of them (#193); whiteouts
    /// before it are passed over.
    fn has_entries(&self, dir: u64) -> Result<bool> {
        let mut c = match self.cursor(ids::DIRENTS) {
            // A btree with no root yet holds nothing.
            Err(Error::NotFound(_)) if self.root(ids::DIRENTS).is_err() => return Ok(false),
            r => r?,
        };
        c.seek(Bpos {
            inode: dir,
            offset: 0,
            snapshot: 0,
        })?;
        while let Some(k) = c.next_key()? {
            if k.pos.inode != dir {
                break;
            }
            if k.key_type == key_type::DIRENT {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The keys of btree `id` in the run of consecutive offsets that starts
    /// at `from`, through a cursor: a hash table's probe. It stops at the
    /// first key that skips an offset, so a lookup reads the nodes its run
    /// is in, not every node after its slot (#191).
    fn run_from(&self, id: u8, from: Bpos) -> Result<Vec<Bkey>> {
        let mut c = match self.cursor(id) {
            // A btree with no root yet holds nothing.
            Err(Error::NotFound(_)) if self.root(id).is_err() => return Ok(Vec::new()),
            r => r?,
        };
        c.seek(from)?;
        let mut out = Vec::new();
        let mut next = Some(from.offset);
        while let Some(k) = c.next_key()? {
            if k.pos.inode != from.inode || next.is_none_or(|n| k.pos.offset > n) {
                break;
            }
            next = k.pos.offset.checked_add(1);
            out.push(k);
        }
        Ok(out)
    }

    /// Delete every key of inode `ino`'s xattr hash table, for an inode
    /// that is going (#161): left behind, the reference checker reports each
    /// as `key_in_missing_inode`. Whiteouts go too, and none is written,
    /// since no entry of the table is left for a lookup to probe past.
    fn drop_xattrs(&self, ino: u64, t: &mut Txn) -> Result<()> {
        for k in self.keys_from(
            XATTRS,
            Bpos {
                inode: ino,
                offset: 0,
                snapshot: 0,
            },
        )? {
            if k.key_type == key_type::XATTR || k.key_type == key_type::HASH_WHITEOUT {
                t.delete(XATTRS, &k);
            }
        }
        Ok(())
    }

    fn key_at(&self, id: u8, pos: Bpos) -> Result<Option<Bkey>> {
        let mut c = match self.cursor(id) {
            // A btree with no root yet holds nothing.
            Err(Error::NotFound(_)) if self.root(id).is_err() => return Ok(None),
            r => r?,
        };
        c.seek(pos)?;
        Ok(c.next_key()?.filter(|k| k.pos == pos))
    }

    fn inode(&self, ino: u64) -> Result<InodeRef> {
        let key = self
            .key_at(ids::INODES, pos(0, ino))?
            .filter(|k| k.key_type == key_type::INODE_V3)
            .ok_or_else(|| Error::NotFound(format!("inode {ino}")))?;
        let raw = crate::inode::InodeV3Raw::parse(&key.value)?;
        Ok(InodeRef { key, raw })
    }

    fn dir(&self, ino: u64) -> Result<InodeRef> {
        let i = self.inode(ino)?;
        if !i.is_dir() {
            return Err(Error::Corrupt(format!("inode {ino} is not a directory")));
        }
        Ok(i)
    }

    /// Where `name` is in `dir`: `(slot, the dirent, the key a new dirent
    /// replaces)`. From the name's hash slot up, every slot holding another
    /// name or a `hash_whiteout` is passed, until the name or an empty
    /// slot. A new name takes the first whiteout passed, else the empty
    /// slot (S8: colliding names take consecutive offsets in creation
    /// order, and a name created again takes the whiteout its removal
    /// left).
    fn dirent(&self, dir: &InodeRef, name: &[u8]) -> Result<(Bpos, Option<Bkey>, Option<Bkey>)> {
        let ino = dir.key.pos.offset;
        // A casefolded directory's entries carry a folded name too, placed
        // by it (inode::INODE_HAS_CASE_INSENSITIVE): not made here.
        if dir.raw.flags & u64::from(crate::inode::INODE_HAS_CASE_INSENSITIVE) != 0 {
            return Err(Error::Unsupported(format!(
                "directory {ino} is casefolded: its entries hold a folded name, which this writer \
                 does not make"
            )));
        }
        let start = crate::inode::name_hash(dir.raw.hash_type(), dir.raw.hash_seed, name)
            .ok_or_else(|| Error::Unsupported(format!("directory {ino}: unknown string hash")))?;
        let slots: std::collections::BTreeMap<u64, Bkey> = self
            .run_from(
                ids::DIRENTS,
                Bpos {
                    inode: ino,
                    offset: start,
                    snapshot: 0,
                },
            )?
            .into_iter()
            .map(|k| (k.pos.offset, k))
            .collect();
        let mut whiteout = None;
        let mut at = start;
        loop {
            match slots.get(&at) {
                Some(k) if k.key_type == key_type::DIRENT => {
                    if crate::inode::Dirent::from_key(k)?.name == name {
                        return Ok((pos(ino, at), Some(k.clone()), None));
                    }
                }
                Some(k) if k.key_type == key_type::HASH_WHITEOUT => {
                    whiteout.get_or_insert_with(|| k.clone());
                }
                _ => break,
            }
            at = at
                .checked_add(1)
                .ok_or_else(|| Error::Unsupported("a hash run reaches the last offset".into()))?;
        }
        Ok(match whiteout {
            Some(w) => (w.pos, None, Some(w)),
            None => (pos(ino, at), None, None),
        })
    }

    /// Remove `dirent` in `t`: a `hash_whiteout` takes its slot when the
    /// next slot is in use, so a name further along its run is still
    /// reached; otherwise it is deleted (S8: a removal inside a run of
    /// colliding names left a whiteout, the write study's unlink none).
    fn remove_dirent(&self, t: &mut Txn, dirent: &Bkey) -> Result<()> {
        let next = pos(dirent.pos.inode, dirent.pos.offset.wrapping_add(1));
        let in_use = self.key_at(ids::DIRENTS, next)?.is_some_and(|k| {
            k.key_type == key_type::DIRENT || k.key_type == key_type::HASH_WHITEOUT
        });
        if in_use {
            let whiteout = Bkey {
                key_type: key_type::HASH_WHITEOUT,
                size: 0,
                version_hi: 0,
                version_lo: 0,
                pos: dirent.pos,
                value: Vec::new(),
            };
            t.put(ids::DIRENTS, Some(dirent), whiteout);
        } else {
            t.delete(ids::DIRENTS, dirent);
        }
        Ok(())
    }

    /// The extents of an inode, all of which must be inline: freeing
    /// allocated space is not implemented.
    fn extents_of(&self, ino: u64) -> Result<Vec<Bkey>> {
        Ok(self
            .keys_from(
                ids::EXTENTS,
                Bpos {
                    inode: ino,
                    offset: 0,
                    snapshot: 0,
                },
            )?
            .into_iter()
            .filter(|k| k.key_type != key_type::DELETED)
            .collect())
    }

    /// Nanoseconds since the filesystem's time base, the unit inode times
    /// are stored in (S8: precision 1, base in nanoseconds since the epoch).
    fn now(&self) -> u64 {
        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        ns.saturating_sub(self.sb.time_base_lo)
    }

    /// The next inode number from the allocation cursor, and the cursor
    /// advanced past it (S8: the reference advanced it by one per create).
    fn next_ino(&self) -> Result<(u64, Bkey)> {
        let cursor = self
            .keys(ids::LOGGED_OPS)?
            .into_iter()
            .find(|k| k.key_type == ids::INODE_ALLOC_CURSOR)
            .ok_or_else(|| Error::Unsupported("no inode allocation cursor".into()))?;
        if cursor.value.len() < 16 {
            return Err(Error::Corrupt("inode allocation cursor too short".into()));
        }
        // The cursor's next, or the first free number above it (#107): the
        // cursors are per-CPU (S1 11.5), so one can lag numbers another
        // handed out. How the reference itself skips them has not been
        // observed; a number with a live key at it is taken.
        // Through the cursor from the cursor's next on (#108): only the
        // run of taken numbers is read, not the whole btree.
        let mut ino = le64(&cursor.value, 8);
        if self.root(ids::INODES).is_ok() {
            let mut c = self.cursor(ids::INODES)?;
            c.seek(Bpos {
                inode: 0,
                offset: ino,
                snapshot: 0,
            })?;
            while let Some(k) = c.next_key()? {
                if k.pos.inode != 0 || k.pos.offset > ino {
                    break;
                }
                if k.pos.offset == ino
                    && k.key_type != key_type::DELETED
                    && k.key_type != key_type::WHITEOUT
                {
                    ino = ino
                        .checked_add(1)
                        .ok_or_else(|| Error::Unsupported("no free inode number".into()))?;
                }
            }
        }
        let mut next = cursor;
        next.value[8..16].copy_from_slice(&(ino + 1).to_le_bytes());
        Ok((ino, next))
    }

    fn touch(&self, i: &mut InodeRef, now: u64) {
        i.raw.varints[field::CTIME] = now;
        i.raw.varints[field::MTIME] = now;
        i.raw.journal_seq = self.journal_seq;
    }

    /// A new inode's value, in the shape the reference gave a new file or
    /// directory: the parent's hash type and the four flags bits not yet
    /// understood, every time `now`, `dir`/`dir_offset` naming its dirent.
    #[allow(clippy::too_many_arguments)]
    fn new_inode(
        &self,
        ino: u64,
        parent: &InodeRef,
        offset: u64,
        mode: u32,
        kind: u32,
        size: u64,
        now: u64,
        name: &[u8],
    ) -> Bkey {
        let is_dir = kind == S_IFDIR;
        let fields = if is_dir {
            field::DIR_FIELDS
        } else {
            field::FILE_FIELDS
        };
        let mut varints = vec![0u64; fields as usize + 4];
        for t in [field::ATIME, field::CTIME, field::MTIME, field::OTIME] {
            varints[t] = now;
        }
        varints[field::DIR] = parent.key.pos.offset;
        varints[field::DIR_OFFSET] = offset;
        if is_dir {
            varints[field::DEPTH] = parent.raw.varints.get(field::DEPTH).copied().unwrap_or(0) + 1;
        }
        let mut inherited = parent.raw.flags & (0xf << 20 | 0xf << 32);
        // A new inode takes its directory's options, its own and the ones
        // it inherited, as values only: none is marked set on it, and it
        // carries the flag an inode with options has (S3 + S4: the kernel
        // image's opts/dir/inherited, under a directory with compression
        // set: the same stored value, fields_set 0, flag bit 12; #142).
        for name in INHERITED_OPTIONS {
            let at = crate::inode::FIELD_NAMES
                .iter()
                .position(|n| n == &name)
                .expect("an inode field")
                + 8;
            if let (Some(slot), Some(&v)) = (varints.get_mut(at), parent.raw.varints.get(at)) {
                *slot = v;
            }
        }
        inherited |= parent.raw.flags & INODE_HAS_OPTIONS;
        let raw = crate::inode::InodeV3Raw {
            journal_seq: self.journal_seq,
            hash_seed: crate::siphash::siphash24(now, ino, name),
            flags: inherited | fields << 24 | u64::from(kind | (mode & 0o7777)) << 36,
            sectors: size.div_ceil(512),
            size,
            version: 0,
            varints,
        };
        Bkey {
            key_type: key_type::INODE_V3,
            size: 0,
            version_hi: 0,
            version_lo: 0,
            pos: pos(0, ino),
            value: raw.encode(),
        }
    }

    /// Inline data covering the one block after `start` sectors: the
    /// value is the bytes zero-padded to a whole u64 (S8).
    fn inline_key(ino: u64, start: u64, data: &[u8], block_sectors: u64) -> Bkey {
        let mut value = data.to_vec();
        value.resize(value.len().div_ceil(8) * 8, 0);
        Bkey {
            key_type: key_type::INLINE_DATA,
            size: block_sectors as u32,
            version_hi: 0,
            version_lo: 0,
            pos: pos(ino, start + block_sectors),
            value,
        }
    }

    /// [`data_layout`] on this filesystem's blocks.
    fn layout(&self, len: usize) -> (usize, usize) {
        data_layout(len, (self.sb.block_size as usize * 512).max(512))
    }

    /// Put `data` for inode `ino` into `t` as [`Self::layout`] says.
    /// Returns the sectors it covers, the inode's `bi_sectors`.
    fn put_data(
        &mut self,
        ino: u64,
        raw: &crate::inode::InodeV3Raw,
        data: &[u8],
        t: &mut Txn,
    ) -> Result<u64> {
        let (extents, inline) = self.layout(data.len());
        let mut sectors = 0;
        if extents > 0 {
            let opts = data_options(&self.sb, raw)?;
            sectors = self.allocate_data(ino, opts, 0, &data[..extents], t)?;
        }
        if inline > 0 {
            let block_sectors = u64::from(self.sb.block_size).max(1);
            t.put(
                ids::EXTENTS,
                None,
                Self::inline_key(ino, sectors, &data[extents..], block_sectors),
            );
            sectors += block_sectors;
        }
        Ok(sectors)
    }

    fn dirent_key(at: Bpos, name: &[u8], ino: u64, d_type: u8) -> Bkey {
        Bkey {
            key_type: key_type::DIRENT,
            size: 0,
            version_hi: 0,
            version_lo: 0,
            pos: at,
            value: crate::inode::Dirent {
                dir: at.inode,
                name: name.to_vec(),
                inum: ino,
                d_type,
            }
            .encode_value(),
        }
    }

    /// Write a transaction: the accounting it implies first computed from
    /// the keys as they stand, then each btree's keys, then accounting.
    fn commit(&mut self, mut t: Txn) -> Result<()> {
        self.refuse_a_snapshotted_root()?;
        if self.session.is_some() {
            return self.commit_journal(t);
        }
        self.commit_in_place(std::mem::take(&mut t))
    }

    /// Write a transaction in place: btree by btree, lowest id first, then
    /// accounting; rewriting a node adds keys to other btrees and to
    /// accounting, which are written in turn until nothing is left.
    fn commit_in_place(&mut self, mut t: Txn) -> Result<()> {
        loop {
            if let Some(id) = t.keys.keys().next().copied() {
                let keys = t.keys.remove(&id).unwrap_or_default();
                t.inflight = Some((id, keys.clone()));
                self.insert_one(id, keys, &mut t)?;
                t.inflight = None;
                continue;
            }
            if t.acct.values().any(|d| d.iter().any(|&x| x != 0)) {
                let acct = std::mem::take(&mut t.acct);
                let keys = self.accounting_deltas(&acct)?;
                self.insert_one(ids::ACCOUNTING, keys, &mut t)?;
                continue;
            }
            return Ok(());
        }
    }

    fn create(
        &mut self,
        parent: u64,
        name: &[u8],
        data: &[u8],
        mode: u32,
        kind: u32,
    ) -> Result<u64> {
        let is_dir = kind == S_IFDIR;
        valid_name(name)?;
        if self.layout(data.len()).0 > 0 {
            self.discard_freed()?;
        }
        let mut p = self.dir(parent)?;
        let (at, existing, taken) = self.dirent(&p, name)?;
        if existing.is_some() {
            return Err(Error::Corrupt(format!(
                "{:?} already exists",
                String::from_utf8_lossy(name)
            )));
        }
        let (ino, cursor) = self.next_ino()?;
        let now = self.now();
        let mut t = Txn::default();
        t.inodes(1);
        let mut new = self.new_inode(ino, &p, at.offset, mode, kind, data.len() as u64, now, name);
        if !data.is_empty() {
            let mut raw = crate::inode::InodeV3Raw::parse(&new.value)?;
            let sectors = self.put_data(ino, &raw, data, &mut t)?;
            raw.sectors = sectors;
            new.value = raw.encode();
        }
        t.put(ids::INODES, None, new);
        let old_parent = p.key.clone();
        self.touch(&mut p, now);
        if is_dir {
            // The parent stores its number of subdirectories (S8: mkdir
            // raised it by one).
            p.raw.varints[field::NLINK] += 1;
        }
        t.put(ids::INODES, Some(&old_parent), p.rekey());
        t.put(
            ids::DIRENTS,
            taken.as_ref(),
            Self::dirent_key(at, name, ino, d_type(kind)),
        );
        // The cursor replaces itself; logged_ops has no counter.
        t.put_uncounted(ids::LOGGED_OPS, cursor);
        self.commit(t)?;
        Ok(ino)
    }

    /// Create a regular file named `name` in directory `parent` holding
    /// `data`, laid out as the reference would (see [`inline_max`]). Returns its
    /// inode number.
    pub fn create_file(&mut self, parent: u64, name: &[u8], data: &[u8], mode: u32) -> Result<u64> {
        self.create(parent, name, data, mode, S_IFREG)
    }

    /// Create a directory named `name` in `parent`. Returns its inode
    /// number.
    pub fn mkdir(&mut self, parent: u64, name: &[u8], mode: u32) -> Result<u64> {
        self.create(parent, name, &[], mode, S_IFDIR)
    }

    fn remove(&mut self, parent: u64, name: &[u8], want_dir: bool) -> Result<()> {
        valid_name(name)?;
        let mut p = self.dir(parent)?;
        let (_, dirent, _) = self.dirent(&p, name)?;
        let dirent = dirent
            .ok_or_else(|| Error::NotFound(format!("{:?}", String::from_utf8_lossy(name))))?;
        let target = crate::inode::Dirent::from_key(&dirent)?.inum;
        let mut i = self.inode(target)?;
        if i.is_dir() != want_dir {
            return Err(Error::Corrupt(format!(
                "{:?} is {}a directory",
                String::from_utf8_lossy(name),
                if want_dir { "not " } else { "" }
            )));
        }
        let now = self.now();
        let mut t = Txn::default();
        if want_dir && self.has_entries(target)? {
            return Err(Error::Corrupt("the directory is not empty".into()));
        }
        self.remove_dirent(&mut t, &dirent)?;
        if !want_dir && i.raw.varints[field::NLINK] > 0 {
            // Another name still links it: one link fewer.
            let old = i.key.clone();
            i.raw.varints[field::NLINK] -= 1;
            i.raw.varints[field::CTIME] = now;
            i.raw.journal_seq = self.journal_seq;
            t.put(ids::INODES, Some(&old), i.rekey());
        } else {
            let extents = self.extents_of(target)?;
            self.free_extents(target, &extents, &mut t)?;
            self.drop_xattrs(target, &mut t)?;
            t.delete(ids::INODES, &i.key);
            t.inodes(-1);
        }
        let old_parent = p.key.clone();
        self.touch(&mut p, now);
        if want_dir {
            p.raw.varints[field::NLINK] = p.raw.varints[field::NLINK].saturating_sub(1);
        }
        t.put(ids::INODES, Some(&old_parent), p.rekey());
        self.commit(t)
    }

    /// Remove the file or symlink `name` from `parent`.
    pub fn unlink(&mut self, parent: u64, name: &[u8]) -> Result<()> {
        self.remove(parent, name, false)
    }

    /// Remove the empty directory `name` from `parent`.
    pub fn rmdir(&mut self, parent: u64, name: &[u8]) -> Result<()> {
        self.remove(parent, name, true)
    }

    /// Move `name` in `from` to `to_name` in `to`. A file already at
    /// `to_name` is replaced, as rename(2) replaces it; a directory may move
    /// to another directory.
    pub fn rename(&mut self, from: u64, name: &[u8], to: u64, to_name: &[u8]) -> Result<()> {
        valid_name(name)?;
        valid_name(to_name)?;
        let src = self.dir(from)?;
        let (_, dirent, _) = self.dirent(&src, name)?;
        let dirent = dirent
            .ok_or_else(|| Error::NotFound(format!("{:?}", String::from_utf8_lossy(name))))?;
        let d = crate::inode::Dirent::from_key(&dirent)?;
        let dst = self.dir(to)?;
        let (at, existing, taken) = self.dirent(&dst, to_name)?;
        let mut i = self.inode(d.inum)?;
        if i.is_dir() && from != to {
            // Not into itself or below it, as rename(2) refuses: walk up
            // from the new parent by each directory's bi_dir.
            let mut up = to;
            while up != 0 {
                if up == d.inum {
                    return Err(Error::Corrupt(
                        "a directory cannot move into itself or below it".into(),
                    ));
                }
                up = self.inode(up)?.raw.varints[field::DIR];
            }
        }
        let now = self.now();
        let mut t = Txn::default();
        // The name it replaces (S8, the write study's rename-over): the
        // dirent keeps its slot and names the moved inode, and the inode it
        // named, with no other link, is deleted with its extents.
        let replaced = match &existing {
            Some(k) => {
                let r = crate::inode::Dirent::from_key(k)?.inum;
                if r == d.inum {
                    return Ok(());
                }
                let ri = self.inode(r)?;
                if ri.is_dir() || i.is_dir() {
                    return Err(Error::Unsupported(
                        "renaming over a directory, or a directory over a file, is not implemented"
                            .into(),
                    ));
                }
                Some(ri)
            }
            None => None,
        };
        self.remove_dirent(&mut t, &dirent)?;
        t.put(
            ids::DIRENTS,
            existing.as_ref().or(taken.as_ref()),
            Self::dirent_key(at, to_name, d.inum, d.d_type),
        );
        if let Some(mut ri) = replaced {
            if ri.raw.varints[field::NLINK] > 0 {
                let old = ri.key.clone();
                ri.raw.varints[field::NLINK] -= 1;
                ri.raw.varints[field::CTIME] = now;
                ri.raw.journal_seq = self.journal_seq;
                t.put(ids::INODES, Some(&old), ri.rekey());
            } else {
                let extents = self.extents_of(ri.key.pos.offset)?;
                self.free_extents(ri.key.pos.offset, &extents, &mut t)?;
                self.drop_xattrs(ri.key.pos.offset, &mut t)?;
                t.delete(ids::INODES, &ri.key);
                t.inodes(-1);
            }
        }
        // The inode names its dirent (S8: rename changed bi_dir_offset and
        // the inode's ctime; move-dir changed bi_dir too).
        let moves_dir = i.is_dir() && from != to;
        let old = i.key.clone();
        i.raw.varints[field::DIR] = to;
        i.raw.varints[field::DIR_OFFSET] = at.offset;
        i.raw.varints[field::CTIME] = now;
        i.raw.journal_seq = self.journal_seq;
        t.put(ids::INODES, Some(&old), i.rekey());
        for dir in if from == to {
            vec![from]
        } else {
            vec![from, to]
        } {
            let mut p = self.dir(dir)?;
            let old = p.key.clone();
            self.touch(&mut p, now);
            // A directory's bi_nlink counts its subdirectories (S8,
            // move-dir: the old parent's fell by one, the new one's rose).
            if moves_dir && dir == from {
                p.raw.varints[field::NLINK] = p.raw.varints[field::NLINK].saturating_sub(1);
            } else if moves_dir {
                p.raw.varints[field::NLINK] += 1;
            }
            t.put(ids::INODES, Some(&old), p.rekey());
        }
        self.commit(t)
    }

    /// A file's whole contents as this writer sees them, a session's
    /// journal included: inline data, and data extents read, checked and
    /// decompressed as the reader does (`fs::extent_bytes`). Holes,
    /// reservations and unwritten extents read as zeros: an unwritten
    /// extent's sectors are not the file's data (#158, #207). Reflinked data
    /// is read from the indirect extents a `reflink_p` covers
    /// ([`Writer::reflinked_into`]), so the rewrite copies it into extents
    /// of the file's own.
    fn contents(&self, ino: u64) -> Result<Vec<u8>> {
        let claimed = self.inode(ino)?.raw.size;
        // The whole file is held to be rewritten; one larger than the device
        // is refused, as `Filesystem::read_at` refuses it (#149).
        if claimed > self.dev.size_bytes() {
            return Err(Error::Unsupported(format!(
                "inode {ino}: {claimed} bytes is more than the device holds"
            )));
        }
        let size = usize::try_from(claimed)
            .map_err(|_| Error::Unsupported("file larger than memory".into()))?;
        let mut out = crate::util::zeroed(size, format_args!("inode {ino}"))?;
        for k in self.extents_of(ino)? {
            let start = k.start_offset().saturating_mul(512);
            match k.key_type {
                key_type::EXTENT => {
                    let e = crate::extent::DataExtent::from_key(&k)?;
                    if e.ptr.is_unwritten() {
                        continue;
                    }
                    let data =
                        crate::fs::extent_bytes(&e, None, self.dev.size_bytes(), |at, buf| {
                            Ok(self.dev.read_at(at, buf)?)
                        })?;
                    crate::fs::copy_window(&mut out, 0, size as u64, e.file_start * 512, &data);
                }
                key_type::INLINE_DATA => {
                    let n = k.value.len().min(k.size as usize * 512);
                    crate::fs::copy_window(&mut out, 0, size as u64, start, &k.value[..n]);
                }
                key_type::REFLINK_P => self.reflinked_into(ino, &k, &mut out)?,
                key_type::RESERVATION | key_type::WHITEOUT | key_type::EXTENT_WHITEOUT => {}
                other => {
                    return Err(Error::Unsupported(format!(
                        "inode {ino}: extent key type {other} cannot be rewritten"
                    )))
                }
            }
        }
        Ok(out)
    }

    /// Copy the data `reflink_p` key `p` of file `ino` shows into `out`,
    /// the file's bytes (#169): each indirect extent the pointer covers,
    /// read as the reader reads it (`Filesystem::read_reflinked`), at the
    /// file sector it shows up at. The reflink btree is read through a
    /// cursor from the pointer's start to the end of its run, as
    /// `unreference` reads it (#193).
    fn reflinked_into(&self, ino: u64, p: &Bkey, out: &mut [u8]) -> Result<()> {
        const REFLINK: u8 = crate::btree::btree_id::REFLINK;
        let idx = crate::extent::reflink_p_idx(&p.value)?;
        let end = idx + u64::from(p.size);
        let missing = |at: u64| {
            Error::Corrupt(format!(
                "inode {ino}: the reflink btree has nothing at sector {at}, which the \
                 `reflink_p` at {} points into",
                p.pos
            ))
        };
        let mut c = match self.cursor(REFLINK) {
            Err(Error::NotFound(_)) if self.root(REFLINK).is_err() => return Err(missing(idx)),
            r => r?,
        };
        c.seek(Bpos {
            inode: 0,
            offset: idx + 1,
            snapshot: 0,
        })?;
        let mut at = idx;
        while let Some(v) = c.next_key()? {
            let from = v.start_offset();
            if v.pos.inode != 0 || at >= end || from > at {
                break;
            }
            if v.key_type != key_type::REFLINK_V || v.value.len() < 8 {
                return Err(Error::Unsupported(format!(
                    "inode {ino}: reflinked data in a key of type {} at {}",
                    v.key_type, v.pos
                )));
            }
            let e = crate::extent::DataExtent::from_key(&Bkey {
                key_type: key_type::EXTENT,
                value: v.value[8..].to_vec(),
                ..v.clone()
            })?;
            let to = v.pos.offset.min(end);
            // An unwritten extent's sectors are not data: the range stays
            // zeros, as the reader reads it (#158).
            if !e.ptr.is_unwritten() {
                let data = crate::fs::extent_bytes(&e, None, self.dev.size_bytes(), |at, buf| {
                    Ok(self.dev.read_at(at, buf)?)
                })?;
                let cut = data
                    .get(((at - from) * 512) as usize..((to - from) * 512) as usize)
                    .ok_or_else(|| {
                        Error::Corrupt(format!(
                            "the indirect extent at {} holds less than its size",
                            v.pos
                        ))
                    })?;
                let size = out.len() as u64;
                crate::fs::copy_window(out, 0, size, (p.start_offset() + at - idx) * 512, cut);
            }
            at = to;
        }
        if at < end {
            return Err(missing(at));
        }
        Ok(())
    }

    /// Write `data` into a file at byte `offset` (#102), growing it when
    /// the write ends past its size; a gap past the old end reads as zeros.
    /// The file is rewritten whole, laid out as [`Writer::write_file`] lays
    /// it out, so what the reference would store for those bytes is what
    /// is stored. Its holes stay holes (#156): only the blocks that held
    /// something and the blocks the write covers are written, so a gap of a
    /// block or more past the old end is a hole too.
    pub fn write_at(&mut self, ino: u64, offset: u64, data: &[u8]) -> Result<()> {
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or_else(|| Error::Unsupported("a write past the largest offset".into()))?;
        let mut c = self.contents(ino)?;
        let block = (u64::from(self.sb.block_size) * 512).max(512);
        let mut ranges = self.stored_ranges(ino, block)?;
        let kept = (c.len() as u64).next_multiple_of(block);
        let start = offset - offset % block;
        if !data.is_empty() && start > kept {
            let device = self.dev.size_bytes();
            if end > device {
                return Err(Error::Unsupported(format!(
                    "inode {ino}: a size of {end} bytes is more than the device holds \
                     ({device} bytes)"
                )));
            }
            self.grow(ino, &mut c, kept)?;
            let mut tail = Vec::new();
            tail.try_reserve_exact((end - start) as usize)
                .map_err(|_| {
                    Error::Unsupported(format!(
                        "inode {ino}: {} bytes do not fit in memory",
                        end - start
                    ))
                })?;
            tail.resize((offset - start) as usize, 0);
            tail.extend_from_slice(data);
            let mut pieces = pieces_of(&c, &ranges);
            pieces.push((start, &tail[..]));
            return self.write_pieces(ino, end, &pieces);
        }
        if end > c.len() as u64 {
            self.grow(ino, &mut c, end)?;
        }
        let (from, to) = (offset as usize, end as usize);
        c[from..to].copy_from_slice(data);
        if !data.is_empty() {
            ranges.push((start, end.next_multiple_of(block)));
        }
        let pieces = pieces_of(&c, &merge_ranges(ranges));
        self.write_pieces(ino, c.len() as u64, &pieces)
    }

    /// Append `data` to a file (#102).
    pub fn append(&mut self, ino: u64, data: &[u8]) -> Result<()> {
        let size = self.inode(ino)?.raw.size;
        self.write_at(ino, size, data)
    }

    /// Set a file's size (#102): cut short, or grown with zeros. The
    /// file's holes stay holes (#156), and growing leaves the new range one:
    /// only the blocks that held something are rewritten, the last
    /// zero-filled past the old end, and the size set, so the range has no
    /// extent and reads as zeros. A size larger than the device is still
    /// refused, as [`Writer::write_at`]'s is.
    pub fn truncate(&mut self, ino: u64, size: u64) -> Result<()> {
        let mut c = self.contents(ino)?;
        let block = (u64::from(self.sb.block_size) * 512).max(512);
        let ranges = self.stored_ranges(ino, block)?;
        if size <= c.len() as u64 {
            c.truncate(size as usize);
            return self.write_pieces(ino, size, &pieces_of(&c, &ranges));
        }
        let device = self.dev.size_bytes();
        if size > device {
            return Err(Error::Unsupported(format!(
                "inode {ino}: a size of {size} bytes is more than the device holds \
                 ({device} bytes)"
            )));
        }
        let kept = (c.len() as u64).next_multiple_of(block).min(size);
        self.grow(ino, &mut c, kept)?;
        self.write_pieces(ino, size, &pieces_of(&c, &ranges))
    }

    /// The byte ranges of file `ino` its extents btree holds something in,
    /// widened to whole blocks of `block` bytes and merged, in order:
    /// everything else is a hole (#156). A reservation or an unwritten
    /// extent holds space, so it is kept as written zeros, as before.
    fn stored_ranges(&self, ino: u64, block: u64) -> Result<Vec<(u64, u64)>> {
        let mut out = Vec::new();
        for k in self.extents_of(ino)? {
            if k.size == 0 || matches!(k.key_type, key_type::WHITEOUT | key_type::EXTENT_WHITEOUT) {
                continue;
            }
            let from = k.start_offset().saturating_mul(512);
            let to = k.pos.offset.saturating_mul(512);
            out.push((
                from - from % block,
                to.div_ceil(block).saturating_mul(block),
            ));
        }
        Ok(merge_ranges(out))
    }

    /// Grow the contents `c` of file `ino` to `size` bytes with zeros, for
    /// a write or a truncate that ends past them. The file is held in
    /// memory whole, its holes as zeros, so a size larger than the device
    /// is refused before anything is allocated, where holding it once
    /// aborted the process (#156). The rest is reserved fallibly
    /// ([`crate::util::zeroed`]).
    fn grow(&self, ino: u64, c: &mut Vec<u8>, size: u64) -> Result<()> {
        let device = self.dev.size_bytes();
        if size > device {
            return Err(Error::Unsupported(format!(
                "inode {ino}: a size of {size} bytes is more than the device holds \
                 ({device} bytes), and this writer writes a file whole, holes included"
            )));
        }
        let more = usize::try_from(size)
            .map_err(|_| Error::Unsupported("size larger than memory".into()))?
            - c.len();
        c.try_reserve_exact(more).map_err(|_| {
            Error::Unsupported(format!("inode {ino}: {size} bytes do not fit in memory"))
        })?;
        c.resize(c.len() + more, 0);
        Ok(())
    }

    /// Replace a file's whole contents with `data`, laid out as the
    /// reference would (see [`inline_max`]); empty truncates it.
    /// The old contents' space is freed.
    pub fn write_file(&mut self, ino: u64, data: &[u8]) -> Result<()> {
        self.write_file_sized(ino, data, data.len() as u64)
    }

    /// Replace a file's contents with `pieces`, each the bytes at a byte
    /// offset that is a whole block, in order and apart, with nothing
    /// between them: a hole, which reads as zeros (#156). The file is
    /// `size` bytes, and only a piece that ends there may end inside a
    /// block. The pieces are laid out as the file would be were it all
    /// data (see [`data_layout`]): full blocks in extents, and a short last
    /// block inline when the reference would keep it inline.
    fn write_pieces(&mut self, ino: u64, size: u64, pieces: &[(u64, &[u8])]) -> Result<()> {
        let mut i = self.inode(ino)?;
        if i.raw.mode() & 0o170000 != 0o100000 {
            return Err(Error::Corrupt(format!("inode {ino} is not a regular file")));
        }
        let whole = usize::try_from(size)
            .map_err(|_| Error::Unsupported("size larger than memory".into()))?;
        let in_extents = self.layout(whole).0 as u64;
        // Each piece's bytes before `in_extents` go in extents, the rest of
        // it, the file's short last block, inline.
        let split =
            |at: u64, bytes: &[u8]| (in_extents.clamp(at, at + bytes.len() as u64) - at) as usize;
        // Buckets freed by earlier commits come back first; the ones this
        // rewrite frees wait for the next (#132).
        if pieces.iter().any(|&(at, bytes)| split(at, bytes) > 0) {
            self.discard_freed()?;
        }
        let now = self.now();
        let mut t = Txn::default();
        let old_extents = self.extents_of(ino)?;
        self.free_extents(ino, &old_extents, &mut t)?;
        let opts = data_options(&self.sb, &i.raw)?;
        let block_sectors = u64::from(self.sb.block_size).max(1);
        let mut sectors = 0;
        for &(at, bytes) in pieces {
            let n = split(at, bytes);
            if n > 0 {
                sectors += self.allocate_data(ino, opts, at / 512, &bytes[..n], &mut t)?;
            }
            if n < bytes.len() {
                t.put(
                    ids::EXTENTS,
                    None,
                    Self::inline_key(ino, (at + n as u64) / 512, &bytes[n..], block_sectors),
                );
                sectors += block_sectors;
            }
        }
        let old = i.key.clone();
        i.raw.size = size;
        i.raw.sectors = sectors;
        self.touch(&mut i, now);
        t.put(ids::INODES, Some(&old), i.rekey());
        self.commit(t)
    }

    /// [`Writer::write_file`], with the file's size set to `size`, at least
    /// `data`'s length: the bytes past `data` are a hole and read as zeros.
    fn write_file_sized(&mut self, ino: u64, data: &[u8], size: u64) -> Result<()> {
        let mut i = self.inode(ino)?;
        if i.raw.mode() & 0o170000 != 0o100000 {
            return Err(Error::Corrupt(format!("inode {ino} is not a regular file")));
        }
        // Buckets freed by earlier commits come back first; the ones this
        // rewrite frees wait for the next (#132).
        if self.layout(data.len()).0 > 0 {
            self.discard_freed()?;
        }
        let now = self.now();
        let mut t = Txn::default();
        let old_extents = self.extents_of(ino)?;
        self.free_extents(ino, &old_extents, &mut t)?;
        let sectors = self.put_data(ino, &i.raw, data, &mut t)?;
        let old = i.key.clone();
        i.raw.size = size.max(data.len() as u64);
        i.raw.sectors = sectors;
        self.touch(&mut i, now);
        t.put(ids::INODES, Some(&old), i.rekey());
        self.commit(t)
    }

    /// The accounting keys with `deltas` applied. A key that does not exist
    /// yet is created with zero counters (S8: a first data write created
    /// the user replicas and inode keys).
    fn accounting_deltas(
        &self,
        deltas: &std::collections::BTreeMap<Bpos, Vec<i64>>,
    ) -> Result<Vec<Bkey>> {
        let all = self.keys(ids::ACCOUNTING)?;
        let mut out = Vec::new();
        for (&at, d) in deltas {
            if d.iter().all(|&x| x == 0) {
                continue;
            }
            let mut k = match all
                .iter()
                .find(|k| k.pos == at && k.key_type == ids::ACCOUNTING_KEY)
            {
                Some(k) => k.clone(),
                None => Bkey {
                    key_type: ids::ACCOUNTING_KEY,
                    size: 0,
                    // The journal sequence above 32 bits of zero.
                    version_hi: (self.journal_seq >> 32) as u32,
                    version_lo: self.journal_seq << 32,
                    pos: at,
                    value: vec![0; d.len() * 8],
                },
            };
            if k.value.len() < d.len() * 8 {
                return Err(Error::Corrupt(format!("accounting key {at} is too short")));
            }
            for (i, &delta) in d.iter().enumerate() {
                let v = le64(&k.value, i * 8)
                    .checked_add_signed(delta)
                    .ok_or_else(|| {
                        Error::Corrupt(format!("accounting counter {i} of {at} would go negative"))
                    })?;
                k.value[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
            }
            out.push(k);
        }
        Ok(out)
    }
}

/// `ranges`, sorted and with each run of overlapping or touching ones made
/// one.
fn merge_ranges(mut ranges: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    ranges.sort_unstable();
    let mut out: Vec<(u64, u64)> = Vec::with_capacity(ranges.len());
    for (from, to) in ranges {
        match out.last_mut() {
            Some(last) if from <= last.1 => last.1 = last.1.max(to),
            _ => out.push((from, to)),
        }
    }
    out
}

/// The bytes of `contents` each of `ranges` covers, cut at its end, and
/// where they start: the pieces [`Writer::write_pieces`] writes. A range
/// past the end is dropped.
fn pieces_of<'a>(contents: &'a [u8], ranges: &[(u64, u64)]) -> Vec<(u64, &'a [u8])> {
    let len = contents.len() as u64;
    ranges
        .iter()
        .filter(|&&(from, _)| from < len)
        .map(|&(from, to)| (from, &contents[from as usize..to.min(len) as usize]))
        .collect()
}

const S_IFREG: u32 = 0o100000;
const S_IFDIR: u32 = 0o040000;
const S_IFLNK: u32 = 0o120000;

/// The DT_* type a dirent records for an inode type.
fn d_type(kind: u32) -> u8 {
    match kind {
        S_IFDIR => 4,
        S_IFLNK => 10,
        _ => 8,
    }
}

/// The xattrs btree and key type (S1's orders).
const XATTRS: u8 = 3;

/// An xattr's namespace number and the name without its prefix (S4, the
/// aged fixture; others are refused, as the reader refuses them).
fn xattr_namespace(name: &[u8]) -> Result<(u8, &[u8])> {
    for (ns, prefix) in [(0u8, &b"user."[..]), (3, &b"trusted."[..])] {
        if let Some(rest) = name.strip_prefix(prefix) {
            if rest.is_empty() || rest.len() > 255 {
                break;
            }
            return Ok((ns, rest));
        }
    }
    Err(Error::Unsupported(format!(
        "xattr {:?}: only the user and trusted namespaces are written",
        String::from_utf8_lossy(name)
    )))
}

impl<D: BlockDevice> Writer<D> {
    /// Create a symlink named `name` in `parent` pointing at `target`: an
    /// inode of mode 120777 whose inline data is the target (S8: the aged
    /// fixture's symlinks).
    pub fn symlink(&mut self, parent: u64, name: &[u8], target: &[u8]) -> Result<u64> {
        if target.is_empty() || target.len() > SYMLINK_MAX || target.contains(&0) {
            return Err(Error::Unsupported(format!(
                "symlink targets of 1 to {SYMLINK_MAX} bytes without NUL are written"
            )));
        }
        self.create(parent, name, target, 0o777, S_IFLNK)
    }

    /// Give inode `ino` one more name: `name` in `dir`. The inode's stored
    /// link count rises and its back-reference moves to the new name, as
    /// the aged fixture's hard links show (S8).
    pub fn link(&mut self, ino: u64, dir: u64, name: &[u8]) -> Result<()> {
        valid_name(name)?;
        let mut i = self.inode(ino)?;
        if i.is_dir() {
            return Err(Error::Corrupt("directories cannot be hard-linked".into()));
        }
        let mut p = self.dir(dir)?;
        let (at, existing, taken) = self.dirent(&p, name)?;
        if existing.is_some() {
            return Err(Error::Corrupt(format!(
                "{:?} already exists",
                String::from_utf8_lossy(name)
            )));
        }
        let now = self.now();
        let mut t = Txn::default();
        t.put(
            ids::DIRENTS,
            taken.as_ref(),
            Self::dirent_key(at, name, ino, d_type(i.raw.mode() & 0o170000)),
        );
        let old = i.key.clone();
        i.raw.varints[field::NLINK] += 1;
        i.raw.varints[field::DIR] = dir;
        i.raw.varints[field::DIR_OFFSET] = at.offset;
        i.raw.varints[field::CTIME] = now;
        i.raw.journal_seq = self.journal_seq;
        t.put(ids::INODES, Some(&old), i.rekey());
        let old_parent = p.key.clone();
        self.touch(&mut p, now);
        t.put(ids::INODES, Some(&old_parent), p.rekey());
        self.commit(t)
    }

    /// Change an inode's permissions and owner; `None` leaves one as it is.
    pub fn set_attributes(
        &mut self,
        ino: u64,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
    ) -> Result<()> {
        let mut i = self.inode(ino)?;
        let old = i.key.clone();
        if let Some(m) = mode {
            let mode_bits = (i.raw.mode() & 0o170000) | (m & 0o7777);
            i.raw.flags = (i.raw.flags & !(0xffff << 36)) | u64::from(mode_bits) << 36;
        }
        if let Some(u) = uid {
            i.raw.varints[field::UID] = u64::from(u);
        }
        if let Some(g) = gid {
            i.raw.varints[field::GID] = u64::from(g);
        }
        i.raw.varints[field::CTIME] = self.now();
        i.raw.journal_seq = self.journal_seq;
        let mut t = Txn::default();
        t.put(ids::INODES, Some(&old), i.rekey());
        self.commit(t)
    }

    /// Where xattr `name` of inode `i` is, or would go (#106): from its
    /// slot ([`crate::xattr::slot`]) along the run of taken slots, as a
    /// dirent's is found (S8, the write study's collide and xcollide):
    /// `(position, the xattr there, a whiteout to reuse)`.
    fn xattr_find(
        &self,
        i: &InodeRef,
        ns: u8,
        short: &[u8],
        name: &[u8],
    ) -> Result<(Bpos, Option<Bkey>, Option<Bkey>)> {
        let ino = i.key.pos.offset;
        let start =
            crate::xattr::slot(i.raw.hash_type(), i.raw.hash_seed, ns, short).ok_or_else(|| {
                Error::Unsupported(format!(
                    "inode {ino} uses string hash type {}: no xattr slot is known for it",
                    i.raw.hash_type()
                ))
            })?;
        // The run of taken slots from the name's own, read up to the first
        // offset it skips, not every xattr of the inode after it (#193).
        let slots: std::collections::BTreeMap<u64, Bkey> = self
            .run_from(
                XATTRS,
                Bpos {
                    inode: ino,
                    offset: start,
                    snapshot: 0,
                },
            )?
            .into_iter()
            .map(|k| (k.pos.offset, k))
            .collect();
        let mut whiteout = None;
        let mut at = start;
        loop {
            match slots.get(&at) {
                Some(k) if k.key_type == key_type::XATTR => {
                    if crate::xattr::Xattr::from_key(k)?.name == name {
                        return Ok((pos(ino, at), Some(k.clone()), None));
                    }
                }
                Some(k) if k.key_type == key_type::HASH_WHITEOUT => {
                    whiteout.get_or_insert_with(|| k.clone());
                }
                _ => break,
            }
            at = at
                .checked_add(1)
                .ok_or_else(|| Error::Unsupported("a hash run reaches the last offset".into()))?;
        }
        Ok(match whiteout {
            Some(w) => (w.pos, None, Some(w)),
            None => (pos(ino, at), None, None),
        })
    }

    /// Set the extended attribute `name` (with its namespace, `user.x`).
    pub fn set_xattr(&mut self, ino: u64, name: &[u8], value: &[u8]) -> Result<()> {
        let (ns, short) = xattr_namespace(name)?;
        if value.len() > 0xffff {
            return Err(Error::Unsupported("xattr values over 65535 bytes".into()));
        }
        let mut i = self.inode(ino)?;
        let (at, old_x, whiteout) = self.xattr_find(&i, ns, short, name)?;
        let mut v = vec![ns, short.len() as u8];
        v.extend_from_slice(&(value.len() as u16).to_le_bytes());
        v.extend_from_slice(short);
        v.extend_from_slice(value);
        v.resize(v.len().div_ceil(8) * 8, 0);
        if 5 + v.len() / 8 > 255 {
            return Err(Error::Unsupported("an xattr too large for one key".into()));
        }
        let mut t = Txn::default();
        if self.root(XATTRS).is_err() {
            // The first xattr of a filesystem that never had one: its
            // btree's root is made, as for lru.
            self.create_root(XATTRS, &mut t)?;
        }
        t.put(
            XATTRS,
            old_x.as_ref().or(whiteout.as_ref()),
            Bkey {
                key_type: key_type::XATTR,
                size: 0,
                version_hi: 0,
                version_lo: 0,
                pos: at,
                value: v,
            },
        );
        let old = i.key.clone();
        i.raw.varints[field::CTIME] = self.now();
        i.raw.journal_seq = self.journal_seq;
        t.put(ids::INODES, Some(&old), i.rekey());
        self.commit(t)
    }

    /// Remove the extended attribute `name`: a `hash_whiteout` keeps its
    /// slot when the next one is taken, so xattrs further along the run are
    /// still found, as for dirents.
    pub fn remove_xattr(&mut self, ino: u64, name: &[u8]) -> Result<()> {
        let (ns, short) = xattr_namespace(name)?;
        let mut i = self.inode(ino)?;
        let (_, old_x, _) = self.xattr_find(&i, ns, short, name)?;
        let old_x = old_x
            .ok_or_else(|| Error::NotFound(format!("xattr {:?}", String::from_utf8_lossy(name))))?;
        let mut t = Txn::default();
        let next = pos(old_x.pos.inode, old_x.pos.offset.wrapping_add(1));
        let in_use = self.key_at(XATTRS, next)?.is_some_and(|k| {
            k.key_type == key_type::XATTR || k.key_type == key_type::HASH_WHITEOUT
        });
        if in_use {
            let whiteout = Bkey {
                key_type: key_type::HASH_WHITEOUT,
                size: 0,
                version_hi: 0,
                version_lo: 0,
                pos: old_x.pos,
                value: Vec::new(),
            };
            t.put(XATTRS, Some(&old_x), whiteout);
        } else {
            t.delete(XATTRS, &old_x);
        }
        let old = i.key.clone();
        i.raw.varints[field::CTIME] = self.now();
        i.raw.journal_seq = self.journal_seq;
        t.put(ids::INODES, Some(&old), i.rekey());
        self.commit(t)
    }
}
