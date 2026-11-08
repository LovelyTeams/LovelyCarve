//! Btree nodes: the header, the bsets inside them, and walking a btree
//! from its root to every leaf key.
//!
//! Provenance (docs/clean-room.md): the node structure (a header with
//! checksum, magic, sequence number, flags carrying the btree id and level,
//! a min/max key interval, a key format and the first bset; then
//! `btree_node_entry` records each wrapping one more bset) is documented
//! (S1, 9.8.5). The byte layout below was found by hexdump and checked
//! against the reference lister's `nodes-ondisk` mode (S3, S4):
//!
//! ```text
//! header:  csum[16] magic u64 @16 flags u64 @24 min_key @32 max_key @52
//!          8 unread bytes @72, bkey_format @80 (56 bytes), bset @136
//! bset:    seq u64, journal_seq u64, flags u32, version u16, u64s u16,
//!          then u64s*8 bytes of keys
//! entry:   csum[16], bset @16; each entry starts on a block boundary
//! ```
//!
//! A bset's checksum covers from just after the checksum to the end of its
//! keys, with the checksum type in the low 4 bits of the bset's flags.

use crate::bkey::{self, key_type, Bkey, BkeyFormat, Bpos};
use crate::error::{Error, Result};
use crate::extent::{self, ExtentEntry};
use crate::superblock::Superblock;
use crate::util::{le16, le32, le64};
use fs_core::BlockRead;

/// A btree node's magic is this constant XOR the first eight bytes of the
/// filesystem's internal UUID read as a little-endian u64. INFERRED: the
/// magic differs per filesystem, and this XOR is the same constant for
/// every fixture.
pub const BSET_MAGIC_BASE: u64 = 0x9013_5c78_b99e_07f5;

/// The node magic for a filesystem with this internal UUID.
pub fn node_magic(uuid: &[u8; 16]) -> u64 {
    BSET_MAGIC_BASE ^ le64(uuid, 0)
}
const HEADER_BSET: usize = 136;
const BSET_HEADER: usize = 24;
/// Deeper than any real btree; a cycle or a corrupt level stops here.
const MAX_DEPTH: u32 = 16;

/// Btree ids, in the order the Principles of Operation lists them (S1,
/// 11.3); extents 0, inodes 1 and dirents 2 confirmed by the reference
/// lister reading the roots this reader found.
pub mod btree_id {
    pub const EXTENTS: u8 = 0;
    pub const INODES: u8 = 1;
    pub const DIRENTS: u8 = 2;
    pub const XATTRS: u8 = 3;
    pub const ALLOC: u8 = 4;
    /// Data shared by reflinked files (S1 9.1.6), positioned at inode 0
    /// and the end of its range, as an extent is.
    pub const REFLINK: u8 = 7;
}

/// A pointer to a btree node: a `btree_ptr_v2` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodePtr {
    pub seq: u64,
    pub sectors_written: u16,
    pub min_key: Bpos,
    pub ptrs: Vec<extent::Ptr>,
}

impl NodePtr {
    /// Decode a `btree_ptr_v2` value: mem_ptr u64 (unread), seq u64,
    /// sectors_written u16, flags u16, min_key, then extent entries.
    pub fn parse(v: &[u8]) -> Result<Self> {
        if v.len() < 40 {
            return Err(Error::Corrupt(
                "btree_ptr_v2 value shorter than 40 bytes".into(),
            ));
        }
        let ptrs = extent::parse_entries(&v[40..])?
            .into_iter()
            .filter_map(|e| match e {
                ExtentEntry::Ptr(p) => Some(p),
                _ => None,
            })
            .collect::<Vec<_>>();
        if ptrs.is_empty() {
            return Err(Error::Corrupt(
                "btree pointer with no device pointer".into(),
            ));
        }
        Ok(NodePtr {
            seq: le64(v, 8),
            sectors_written: le16(v, 16),
            min_key: Bpos::parse(&v[20..40])?,
            ptrs,
        })
    }

    pub fn from_key(k: &Bkey) -> Result<Self> {
        if k.key_type != key_type::BTREE_PTR_V2 {
            return Err(Error::Unsupported(format!(
                "btree pointer key type {}",
                k.key_type
            )));
        }
        Self::parse(&k.value)
    }
}

/// The btree id and level a node header's flags carry: the id's low four
/// bits in bits 0..4, the level in bits 4..8, the id's higher bits from
/// bit 9 (S8: every node of every btree of every clean fixture and
/// write-study image, ids 16 and above included; tests/oracle_node_flags.rs).
pub fn flags_id_and_level(flags: u64) -> (u8, u8) {
    (
        ((flags & 0xf) | ((flags >> 9) & 0xf) << 4) as u8,
        ((flags >> 4) & 0xf) as u8,
    )
}

/// `flags` with its btree id and level replaced (see
/// [`flags_id_and_level`]); every other bit is kept.
pub fn flags_with_id_and_level(flags: u64, id: u8, level: u8) -> u64 {
    let id = u64::from(id);
    (flags & !(0xff | 0xf << 9)) | (id & 0xf) | u64::from(level & 0xf) << 4 | (id >> 4) << 9
}

/// One parsed node: its header facts and its keys, merged across bsets.
#[derive(Debug, Clone)]
pub struct Node {
    pub seq: u64,
    pub flags: u64,
    pub min_key: Bpos,
    pub max_key: Bpos,
    pub format: BkeyFormat,
    pub keys: Vec<Bkey>,
}

impl Node {
    /// Parse the written part of a node. `block_bytes` is the filesystem
    /// block size: every `btree_node_entry` starts on a block boundary.
    /// `expect_seq` is the sequence number the parent's pointer recorded.
    pub fn parse(
        b: &[u8],
        magic: u64,
        block_bytes: usize,
        expect_seq: Option<u64>,
    ) -> Result<Self> {
        Self::parse_upto(b, magic, block_bytes, expect_seq, u64::MAX)
    }

    /// [`Node::parse`], ignoring the keys of every bset whose journal
    /// sequence number is above `max_journal_seq`: after an unclean
    /// shutdown those bsets belong to journal entries that were never
    /// committed (S1 9.7.5, "sequence blacklisting"). Their checksums are
    /// still verified.
    pub fn parse_upto(
        b: &[u8],
        magic: u64,
        block_bytes: usize,
        expect_seq: Option<u64>,
        max_journal_seq: u64,
    ) -> Result<Self> {
        Self::parse_filtered(b, magic, block_bytes, expect_seq, max_journal_seq, &[])
    }

    /// [`Node::parse_upto`], also ignoring the keys of every bset whose
    /// journal sequence falls in a blacklisted range (`start <= seq < end`,
    /// the superblock's `journal_seq_blacklist`): the journal entry that
    /// carried those keys was never committed, and the reference ignores
    /// them "until the btree node is next rewritten" (S1 1.3). A clean
    /// image can hold such bsets. Their checksums are still verified.
    pub fn parse_filtered(
        b: &[u8],
        magic: u64,
        block_bytes: usize,
        expect_seq: Option<u64>,
        max_journal_seq: u64,
        blacklist: &[(u64, u64)],
    ) -> Result<Self> {
        Self::parse_keyed(
            b,
            magic,
            block_bytes,
            expect_seq,
            max_journal_seq,
            blacklist,
            None,
        )
    }

    /// [`Node::parse_filtered`] for a node of an encrypted filesystem,
    /// whose master key is `key`: every record's MAC is verified and the
    /// record decrypted before it is read ([`decrypt_node`]). A node whose
    /// records carry MACs is refused without a key; with one, a node whose
    /// records do not is refused too, because an encrypted filesystem
    /// authenticates all of its metadata (S1 2.1.2).
    pub fn parse_keyed(
        b: &[u8],
        magic: u64,
        block_bytes: usize,
        expect_seq: Option<u64>,
        max_journal_seq: u64,
        blacklist: &[(u64, u64)],
        key: Option<&crate::crypt::MasterKey>,
    ) -> Result<Self> {
        if b.len() < HEADER_BSET + BSET_HEADER {
            return Err(Error::Corrupt("btree node shorter than its header".into()));
        }
        if le64(b, 16) != magic {
            return Err(Error::BadMagic { what: "btree node" });
        }
        let block = block_bytes.max(512);
        let encrypted =
            (le32(b, HEADER_BSET + 16) & 0xf) as u8 == crate::crypt::CSUM_CHACHA20_POLY1305_128;
        let decrypted;
        let b = match (encrypted, key) {
            (false, None) => b,
            (false, Some(_)) => {
                return Err(Error::Corrupt(
                    "a btree node with no MAC in an encrypted filesystem".into(),
                ))
            }
            (true, None) => {
                return Err(Error::Unsupported(
                    "an encrypted btree node, and no master key to decrypt it".into(),
                ))
            }
            (true, Some(key)) => {
                let mut copy = b.to_vec();
                decrypt_node(&mut copy, block, key)?;
                decrypted = copy;
                &decrypted[..]
            }
        };
        let format = BkeyFormat::parse(&b[80..80 + BkeyFormat::BYTES])?;
        let seq = le64(b, HEADER_BSET);
        if let Some(e) = expect_seq {
            if e != seq {
                return Err(Error::Corrupt(format!(
                    "btree node seq {seq:#x}, pointer expected {e:#x}"
                )));
            }
        }
        let mut node = Node {
            seq,
            flags: le64(b, 24),
            min_key: Bpos::parse(&b[32..52])?,
            max_key: Bpos::parse(&b[52..72])?,
            format,
            keys: Vec::new(),
        };
        // (bset index, key) so later bsets win a tie at the same position.
        let mut all: Vec<(usize, Bkey)> = Vec::new();
        let mut start = 0usize; // where this record's checksum starts
        let mut bset_at = HEADER_BSET;
        let mut index = 0usize;
        loop {
            if bset_at + BSET_HEADER > b.len() {
                break;
            }
            let bseq = le64(b, bset_at);
            if bseq != seq {
                break; // stale or unwritten space: not part of this node
            }
            let bflags = le32(b, bset_at + 16);
            let u64s = le16(b, bset_at + 22) as usize;
            let keys_at = bset_at + BSET_HEADER;
            let end = keys_at + u64s * 8;
            if end > b.len() {
                return Err(Error::Corrupt(
                    "bset runs past the written part of the node".into(),
                ));
            }
            let csum_type = (bflags & 0xf) as u8;
            if encrypted {
                // decrypt_node verified this record's MAC, and stops at
                // the first record that carries none.
                if csum_type != crate::crypt::CSUM_CHACHA20_POLY1305_128 {
                    return Err(Error::Corrupt(format!(
                        "a bset with checksum type {csum_type} in an encrypted btree node"
                    )));
                }
            } else {
                if !crate::csum::is_known(csum_type) {
                    return Err(Error::Unsupported(format!(
                        "btree node checksum type {csum_type}"
                    )));
                }
                let stored = le64(b, start) & csum_mask(csum_type);
                crate::csum::verify(csum_type, &b[start + 16..end], stored).map_err(
                    |computed| Error::BadChecksum {
                        what: "btree node",
                        stored,
                        computed,
                    },
                )?;
            }
            let journal_seq = le64(b, bset_at + 8);
            let committed = journal_seq <= max_journal_seq
                && !blacklist
                    .iter()
                    .any(|&(start, end)| start <= journal_seq && journal_seq < end);
            let mut p = keys_at;
            let mut prev: Option<Bpos> = None;
            while committed && p < end {
                let n = b[p] as usize;
                if n == 0 {
                    return Err(Error::Corrupt("zero-length key in a bset".into()));
                }
                let kend = p + n * 8;
                if kend > end {
                    return Err(Error::Corrupt("key runs past its bset".into()));
                }
                let k = bkey::decode(&b[p..kend], &node.format)?;
                // A bset is sorted; one that is not would be re-sorted
                // below without a word, and the reference refuses it ("keys
                // out of order", #162).
                if let Some(prev) = prev.filter(|&prev| k.pos < prev) {
                    return Err(Error::Corrupt(format!(
                        "keys out of order in a bset: {} after {prev}",
                        k.pos
                    )));
                }
                prev = Some(k.pos);
                all.push((index, k));
                p = kend;
            }
            index += 1;
            start = end.div_ceil(block) * block;
            bset_at = start + 16;
        }
        if index == 0 {
            return Err(Error::Corrupt(
                "btree node holds no bset of its own sequence".into(),
            ));
        }
        // Every sector the parent's pointer counts as written belongs to a
        // bset of this node. A later bset that is missing, cut short or of
        // another node's sequence leaves a valid prefix that is not the
        // whole node, and the reference refuses it (#222).
        if start < b.len() {
            return Err(Error::Corrupt(format!(
                "btree node data missing: expected {} sectors, found {}",
                b.len().div_ceil(512),
                start / 512
            )));
        }
        // Stable sort by position keeps bset order within a position; the
        // last one (newest bset) wins.
        all.sort_by(|a, b| a.1.pos.cmp(&b.1.pos).then(a.0.cmp(&b.0)));
        let mut merged: Vec<Bkey> = Vec::with_capacity(all.len());
        for (_, k) in all {
            if let Some(last) = merged.last_mut() {
                if last.pos == k.pos {
                    *last = k;
                    continue;
                }
            }
            merged.push(k);
        }
        merged.retain(|k| k.key_type != key_type::DELETED);
        node.keys = merged;
        Ok(node)
    }
}

/// The bytes of a node's header that are encrypted: the flags, the min and
/// max keys, the 8 unread bytes and the key format. OBSERVED: the magic
/// before them reads in the clear on every node of `encrypted.img`, and
/// these bytes decrypt to the bounds each node's parent pointer names.
const HEADER_ENCRYPTED: std::ops::Range<usize> = 24..HEADER_BSET;

/// Verify the MAC of, and decrypt in place, every record of an encrypted
/// node, walking the records as [`Node::parse_keyed`] does: the first after
/// the header, each later one in a `btree_node_entry` at the next block
/// boundary, while its bset carries the node's seq and a MAC. OBSERVED on
/// every record of `encrypted.img` (S4, docs/clean-room.md "Encryption:
/// btree nodes"):
///
/// - a record's MAC is its first 16 bytes, over the record from byte 16 to
///   the end of its keys as stored: the ciphertext, and the magic or the
///   bset header in the clear;
/// - the first record's encrypted header and its keys share one keystream,
///   the keys starting at the 64-byte block after the header's;
/// - a later record encrypts only its keys, from the start of its nonce's
///   keystream;
/// - every bset header is in the clear.
///
/// A record is decrypted only after its MAC verifies.
fn decrypt_node(b: &mut [u8], block: usize, key: &crate::crypt::MasterKey) -> Result<()> {
    let seq = le64(b, HEADER_BSET);
    let mut start = 0usize;
    let mut bset_at = HEADER_BSET;
    while bset_at + BSET_HEADER <= b.len() && le64(b, bset_at) == seq {
        let csum_type = (le32(b, bset_at + 16) & 0xf) as u8;
        let keys_at = bset_at + BSET_HEADER;
        let end = keys_at + le16(b, bset_at + 22) as usize * 8;
        if csum_type != crate::crypt::CSUM_CHACHA20_POLY1305_128 || end > b.len() {
            break; // the parse names what is wrong
        }
        let nonce = crate::crypt::btree_node_nonce(start, seq, le64(b, bset_at + 8))?;
        let mac = key.mac(nonce, &b[start + 16..end])?;
        if b[start..start + 16] != mac {
            return Err(Error::BadChecksum {
                what: "btree node MAC",
                stored: le64(b, start),
                computed: u64::from_le_bytes(mac[..8].try_into().expect("8 bytes")),
            });
        }
        let mut keys_nonce = nonce;
        if start == 0 {
            key.apply(nonce, &mut b[HEADER_ENCRYPTED])?;
            keys_nonce[0] += HEADER_ENCRYPTED.len().div_ceil(64) as u32;
        }
        key.apply(keys_nonce, &mut b[keys_at..end])?;
        start = end.div_ceil(block) * block;
        bset_at = start + 16;
    }
    Ok(())
}

/// How many bytes of the 16-byte checksum field a type uses.
fn csum_mask(t: u8) -> u64 {
    match t {
        1 | 5 => 0xffff_ffff,
        _ => u64::MAX,
    }
}

/// Every leaf key of one btree, in order, on a cleanly unmounted
/// filesystem. Reads the whole tree: fine for the images this spike reads,
/// and the first thing to replace with a cursor.
///
/// An image that was not shut down cleanly is refused: its superblock's
/// roots are stale. [`walk_replayed`] reads it through its journal.
pub fn walk(dev: &dyn BlockRead, sb: &Superblock, id: u8) -> Result<Vec<Bkey>> {
    walk_replayed(dev, sb, id, None)
}

/// Every leaf key of one btree, in order, with a journal replay applied
/// over it when one is given: the root is the one the replay's newest
/// flush entry recorded, bsets from uncommitted journal entries are
/// ignored, and the replayed keys replace the keys at their positions (a
/// deleted key or a whiteout removes it). Nothing is written.
pub fn walk_replayed(
    dev: &dyn BlockRead,
    sb: &Superblock,
    id: u8,
    replay: Option<&crate::journal::Replay>,
) -> Result<Vec<Bkey>> {
    let Some((root_level, root_key)) = root_of(sb, id, replay)? else {
        return Ok(Vec::new());
    };
    if u32::from(root_level) >= MAX_DEPTH {
        return Err(Error::Corrupt(format!(
            "btree {id} root at level {root_level}"
        )));
    }
    let walk = Walk {
        dev,
        sb,
        id,
        replay,
        max_seq: replay.map(|r| r.seq).unwrap_or(u64::MAX),
        blacklist: sb.journal_seq_blacklist()?,
    };
    let mut out = Vec::new();
    walk.descend(
        &NodePtr::from_key(&root_key)?,
        root_key.pos,
        root_level,
        &mut out,
    )?;
    Ok(out)
}

/// The position just after `p` in key order.
pub fn successor(p: Bpos) -> Bpos {
    if p.snapshot < u32::MAX {
        Bpos {
            snapshot: p.snapshot + 1,
            ..p
        }
    } else if p.offset < u64::MAX {
        Bpos {
            inode: p.inode,
            offset: p.offset + 1,
            snapshot: 0,
        }
    } else {
        Bpos {
            inode: p.inode.saturating_add(1),
            offset: 0,
            snapshot: 0,
        }
    }
}

/// A cursor over one btree: [`Cursor::seek`] to a position, then
/// [`Cursor::next`] key by key. It reads only the nodes on its path -- the
/// root and one node per level down to the leaf holding the position --
/// and moves to the next leaf only when the current one is used up,
/// applying a journal replay to each node it reads as [`walk_replayed`]
/// does.
pub struct Cursor<'a> {
    walk: Walk<'a>,
    cache: Option<&'a NodeCache>,
    root_level: u8,
    /// `None` for a btree with no root recorded: it holds nothing.
    root: Option<NodePtr>,
    /// The largest position the whole btree covers: the root key's
    /// position, which is the root node's maximum key. SPOS_MAX on every
    /// fixture (docs/clean-room.md); the cursor does not depend on that.
    root_max: Bpos,
    /// The leaf being read, from where the cursor stands.
    leaf: std::vec::IntoIter<Bkey>,
    /// The largest position the leaf covers; `root_max` for the last.
    leaf_max: Bpos,
    started: bool,
}

impl<'a> Cursor<'a> {
    /// A cursor over btree `id`; on an uncleanly unmounted filesystem pass
    /// its replay, as for [`walk_replayed`].
    pub fn new(
        dev: &'a dyn BlockRead,
        sb: &'a Superblock,
        id: u8,
        replay: Option<&'a crate::journal::Replay>,
    ) -> Result<Self> {
        let (root_level, root, root_max) = match root_of(sb, id, replay)? {
            Some((level, key)) => (level, Some(NodePtr::from_key(&key)?), key.pos),
            None => (0, None, Bpos::MAX),
        };
        Ok(Cursor {
            walk: Walk {
                dev,
                sb,
                id,
                replay,
                max_seq: replay.map(|r| r.seq).unwrap_or(u64::MAX),
                blacklist: sb.journal_seq_blacklist()?,
            },
            root_level,
            root,
            root_max,
            cache: None,
            leaf: Vec::new().into_iter(),
            leaf_max: root_max,
            started: false,
        })
    }

    /// Read nodes through `cache`, so the root and interior nodes every
    /// lookup passes through are read once.
    pub fn with_cache(mut self, cache: &'a NodeCache) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Stand before the first key at or after `pos`; past the btree's last
    /// position, before nothing.
    pub fn seek(&mut self, pos: Bpos) -> Result<()> {
        let root = match &self.root {
            Some(root) if pos <= self.root_max => root.clone(),
            _ => {
                self.leaf = Vec::new().into_iter();
                self.leaf_max = self.root_max;
                self.started = true;
                return Ok(());
            }
        };
        let mut ptr = root;
        let mut level = self.root_level;
        let mut max = self.root_max;
        loop {
            let node = match self.cache {
                Some(c) => c.get(
                    self.walk.dev,
                    self.walk.sb,
                    &ptr,
                    self.walk.max_seq,
                    &self.walk.blacklist,
                )?,
                None => read_node_upto(
                    self.walk.dev,
                    self.walk.sb,
                    &ptr,
                    self.walk.max_seq,
                    &self.walk.blacklist,
                )?,
            };
            check_header(&node, self.walk.id, level, &ptr, max)?;
            let keys = self
                .walk
                .apply(node.keys, level, node.min_key, node.max_key);
            if level == 0 {
                let rest: Vec<Bkey> = keys.into_iter().filter(|k| k.pos >= pos).collect();
                self.leaf = rest.into_iter();
                self.leaf_max = max;
                self.started = true;
                return Ok(());
            }
            let child = keys
                .into_iter()
                .filter(|k| k.key_type == key_type::BTREE_PTR_V2)
                .find(|k| k.pos >= pos)
                .ok_or_else(|| {
                    Error::Corrupt(format!("no child of an interior node covers {pos}"))
                })?;
            max = child.pos;
            ptr = NodePtr::from_key(&child)?;
            level -= 1;
        }
    }

    /// The next key, or `None` past the last.
    pub fn next_key(&mut self) -> Result<Option<Bkey>> {
        if !self.started {
            self.seek(Bpos::default())?;
        }
        loop {
            if let Some(k) = self.leaf.next() {
                return Ok(Some(k));
            }
            if self.leaf_max >= self.root_max {
                return Ok(None);
            }
            let from = successor(self.leaf_max);
            self.seek(from)?;
        }
    }
}

/// Parsed nodes, by where they are, the generation of their bucket and how
/// much of them is written: the root and the interior nodes are what every
/// lookup reads, and they do not change under a reader. The generation is
/// part of the key because a writer reuses freed buckets (#132): a new node
/// in one can sit where an old one was, with the same sequence number, and
/// only the bucket's new generation tells them apart. Bounded: it starts
/// over past a few hundred nodes.
#[derive(Default)]
pub struct NodeCache {
    nodes: std::sync::Mutex<std::collections::HashMap<NodeKey, Node>>,
}

/// A cached node's identity: device offset, bucket generation, sequence
/// number, sectors written, and the newest journal sequence read.
type NodeKey = (u64, u8, u64, u16, u64);

/// How many nodes a cache holds before it starts over.
const NODE_CACHE_CAP: usize = 512;

impl NodeCache {
    fn get(
        &self,
        dev: &dyn BlockRead,
        sb: &Superblock,
        ptr: &NodePtr,
        max_seq: u64,
        blacklist: &[(u64, u64)],
    ) -> Result<Node> {
        // The blacklist is the superblock's, the same for every node a
        // cache sees, so it is not part of the key.
        let key = (
            ptr.ptrs[0].offset,
            ptr.ptrs[0].gen,
            ptr.seq,
            ptr.sectors_written,
            max_seq,
        );
        if let Some(n) = self.nodes.lock().ok().and_then(|m| m.get(&key).cloned()) {
            return Ok(n);
        }
        let node = read_node_upto(dev, sb, ptr, max_seq, blacklist)?;
        if let Ok(mut m) = self.nodes.lock() {
            let cached_bytes: usize = m.keys().map(|key| usize::from(key.3) * 512).sum();
            if m.len() >= NODE_CACHE_CAP
                || cached_bytes + usize::from(ptr.sectors_written) * 512 > 64 * 1024 * 1024
            {
                m.clear();
            }
            m.insert(key, node.clone());
        }
        Ok(node)
    }
}

/// The journal a reader reads a filesystem through: its replay when the
/// filesystem was not shut down cleanly, and also when it was but a root
/// the superblock's clean section records will not read (#166). `None` when
/// the clean section's roots all read.
///
/// Each journal write names every btree's root again, in its `btree_root`
/// entries (S1, journal entry types), and on every clean
/// fixture its newest flush entry records the same roots as the clean
/// section (tests/oracle_journal_roots.rs). The reference opened, and
/// checked clean, two images whose superblock root had been damaged (the
/// differential run, #164), so it does not depend on the superblock's copy
/// alone; which copy it prefers when both read and disagree is not observed,
/// and this reader keeps the clean section's then.
pub fn journal_for(dev: &dyn BlockRead, sb: &Superblock) -> Result<Option<crate::journal::Replay>> {
    if !sb.is_clean() {
        return crate::journal::replay(dev, sb).map(Some);
    }
    let Err(bad) = clean_roots_read(dev, sb) else {
        return Ok(None);
    };
    crate::journal::replay(dev, sb).map(Some).map_err(|e| {
        Error::Corrupt(format!(
            "{bad}; and the journal, which records every root again, does not replay: {e}"
        ))
    })
}

/// Whether every root the clean section records reads: its key decodes,
/// its node reads, and the node is of that btree and level.
fn clean_roots_read(dev: &dyn BlockRead, sb: &Superblock) -> Result<()> {
    let blacklist = sb.journal_seq_blacklist()?;
    for root in sb.btree_roots()? {
        let id = root.btree_id;
        let checked = (|| {
            let Some((level, key)) = root_of(sb, id, None)? else {
                return Ok(());
            };
            let ptr = NodePtr::from_key(&key)?;
            let node = read_node_upto(dev, sb, &ptr, u64::MAX, &blacklist)?;
            check_header(&node, id, level, &ptr, key.pos)
        })();
        checked.map_err(|e| {
            Error::Corrupt(format!(
                "the superblock's root for btree {} does not read: {e}",
                crate::journal::btree_name(id)
            ))
        })?;
    }
    Ok(())
}

/// A btree's root: from the replay's newest flush entry, or the clean
/// field. `None` when none is recorded, which is a btree holding nothing:
/// the reference formatter records no xattrs root on a new filesystem, and
/// the reference lister lists such a btree as empty, without an error (S3:
/// every formatter-made fixture, and `write-study/base.xattrs.txt`). The
/// journal's roots are taken the same way, by analogy (INFERRED).
fn root_of(
    sb: &Superblock,
    id: u8,
    replay: Option<&crate::journal::Replay>,
) -> Result<Option<(u8, Bkey)>> {
    match replay {
        Some(r) => Ok(r
            .roots
            .iter()
            .find(|(b, _, _)| *b == id)
            .map(|(_, level, k)| (*level, k.clone()))),
        None => {
            if !sb.is_clean() {
                return Err(Error::Unsupported(
                    "the filesystem was not cleanly unmounted; its journal must be replayed first"
                        .into(),
                ));
            }
            let roots = sb.btree_roots()?;
            let Some(root) = roots.iter().find(|r| r.btree_id == id) else {
                return Ok(None);
            };
            let key = bkey::decode(
                &root.key,
                &BkeyFormat {
                    key_u64s: 5,
                    nr_fields: 6,
                    bits: [0; 6],
                    field_offset: [0; 6],
                },
            )?;
            Ok(Some((root.level, key)))
        }
    }
}

/// One walk of one btree, with the replay (if any) it applies.
struct Walk<'a> {
    dev: &'a dyn BlockRead,
    sb: &'a Superblock,
    id: u8,
    replay: Option<&'a crate::journal::Replay>,
    max_seq: u64,
    /// The superblock's blacklisted journal sequence ranges.
    blacklist: Vec<(u64, u64)>,
}

impl Walk<'_> {
    /// Read the node at `level` (0 = leaf), which its pointer places at
    /// `max`, and everything below it.
    fn descend(&self, ptr: &NodePtr, max: Bpos, level: u8, out: &mut Vec<Bkey>) -> Result<()> {
        let node = read_node_upto(self.dev, self.sb, ptr, self.max_seq, &self.blacklist)?;
        check_header(&node, self.id, level, ptr, max)?;
        let keys = self.apply(node.keys, level, node.min_key, node.max_key);
        if level == 0 {
            out.extend(keys);
            return Ok(());
        }
        for k in keys {
            if k.key_type != key_type::BTREE_PTR_V2 {
                return Err(Error::Corrupt(format!(
                    "key type {} in an interior node",
                    k.key_type
                )));
            }
            self.descend(&NodePtr::from_key(&k)?, k.pos, level - 1, out)?;
        }
        Ok(())
    }

    /// A node's keys with the replayed keys of its btree and level that
    /// fall within it applied: each replaces the key at its position, and a
    /// deleted key or a whiteout removes it.
    fn apply(&self, keys: Vec<Bkey>, level: u8, min: Bpos, max: Bpos) -> Vec<Bkey> {
        let Some(overlay) = self
            .replay
            .and_then(|r| r.keys.get(&(self.id, level)))
            .filter(|o| o.iter().any(|k| k.pos >= min && k.pos <= max))
        else {
            return keys;
        };
        let mut merged: std::collections::BTreeMap<Bpos, Bkey> =
            keys.into_iter().map(|k| (k.pos, k)).collect();
        for k in overlay.iter().filter(|k| k.pos >= min && k.pos <= max) {
            if self.id == ACCOUNTING && k.key_type == ACCOUNTING_KEY {
                // The journal carries accounting as deltas, applied once:
                // only one newer than the key it adds to (by version, journal
                // sequence first) counts (S8: signed counters in the
                // journal's accounting keys, totals in the btree's).
                match merged.get_mut(&k.pos) {
                    Some(old)
                        if (k.version_hi, k.version_lo) <= (old.version_hi, old.version_lo) => {}
                    Some(old) => {
                        let n = old.value.len().min(k.value.len()) / 8;
                        for i in 0..n {
                            let v = le64(&old.value, i * 8).wrapping_add(le64(&k.value, i * 8));
                            old.value[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
                        }
                        old.version_lo = k.version_lo;
                        old.version_hi = k.version_hi;
                    }
                    None => {
                        merged.insert(k.pos, k.clone());
                    }
                }
                continue;
            }
            if k.key_type == key_type::DELETED || k.key_type == key_type::WHITEOUT {
                merged.remove(&k.pos);
            } else {
                merged.insert(k.pos, k.clone());
            }
        }
        merged.into_values().collect()
    }
}

/// The accounting btree and its key type (S1's orders; S8).
const ACCOUNTING: u8 = 20;
const ACCOUNTING_KEY: u8 = 34;

/// A node read through the pointer `ptr`, in btree `id` at `level`, where
/// the pointer's key places its last position at `max`: its header must
/// say the same of it. The reference checks a node against its parent and
/// refuses one that disagrees ("incorrect btree id", "incorrect min_key",
/// "incorrect max key"); a node that lies about where it belongs would feed
/// its keys into the wrong place (#162).
fn check_header(node: &Node, id: u8, level: u8, ptr: &NodePtr, max: Bpos) -> Result<()> {
    let (node_id, node_level) = flags_id_and_level(node.flags);
    let at = ptr.ptrs[0].offset;
    if node_id != id {
        return Err(Error::Corrupt(format!(
            "btree node at sector {at} is of btree {node_id}, and its pointer is in btree {id}"
        )));
    }
    if node_level != level {
        return Err(Error::Corrupt(format!(
            "btree node at sector {at} is at level {node_level}, and its pointer leads to \
             level {level}"
        )));
    }
    if node.min_key != ptr.min_key {
        return Err(Error::Corrupt(format!(
            "btree node at sector {at} starts at {}, and its pointer at {}",
            node.min_key, ptr.min_key
        )));
    }
    if node.max_key != max {
        return Err(Error::Corrupt(format!(
            "btree node at sector {at} ends at {}, and its pointer at {max}",
            node.max_key
        )));
    }
    Ok(())
}

pub fn read_node(dev: &dyn BlockRead, sb: &Superblock, ptr: &NodePtr) -> Result<Node> {
    read_node_upto(dev, sb, ptr, u64::MAX, &sb.journal_seq_blacklist()?)
}

fn read_node_upto(
    dev: &dyn BlockRead,
    sb: &Superblock,
    ptr: &NodePtr,
    max_seq: u64,
    blacklist: &[(u64, u64)],
) -> Result<Node> {
    let p = &ptr.ptrs[0];
    let len = ptr.sectors_written as usize * 512;
    if len == 0 || len > (sb.btree_node_size() as usize * 512).max(512) {
        return Err(Error::Corrupt(format!(
            "btree pointer claims {len} bytes written"
        )));
    }
    let mut b = vec![0u8; len];
    dev.read_at(p.offset * 512, &mut b)?;
    let key = sb.crypt()?.and_then(|c| c.master_key());
    Node::parse_keyed(
        &b,
        node_magic(&sb.uuid),
        sb.block_size as usize * 512,
        Some(ptr.seq),
        max_seq,
        blacklist,
        key.as_ref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAGIC: u64 = 0x1234_5678_9abc_def0;
    const SEQ: u64 = 7;

    /// An unpacked dirent key at `100:offset:U32_MAX`, no value.
    fn key(offset: u64) -> Vec<u8> {
        let mut k = vec![5u8, 1, key_type::DIRENT, 0];
        k.extend_from_slice(&[0u8; 12]);
        k.extend_from_slice(&0u32.to_le_bytes());
        k.extend_from_slice(&u32::MAX.to_le_bytes());
        k.extend_from_slice(&offset.to_le_bytes());
        k.extend_from_slice(&100u64.to_le_bytes());
        k
    }

    /// A bset record: `csum[16]` then seq, journal_seq, flags (crc32c),
    /// version, u64s, keys; the checksum over everything after it.
    fn record(journal_seq: u64, keys: &[u8]) -> Vec<u8> {
        let mut r = vec![0u8; 16];
        r.extend_from_slice(&SEQ.to_le_bytes());
        r.extend_from_slice(&journal_seq.to_le_bytes());
        r.extend_from_slice(&1u32.to_le_bytes());
        r.extend_from_slice(&0u16.to_le_bytes());
        r.extend_from_slice(&((keys.len() / 8) as u16).to_le_bytes());
        r.extend_from_slice(keys);
        let c = crate::csum::crc32c_nonzero(&r[16..]);
        r[0..4].copy_from_slice(&c.to_le_bytes());
        r
    }

    /// A node of two bsets in two 512-byte blocks: the header's with one
    /// key at journal sequence 5, then an entry with one key at
    /// `second_journal_seq`.
    fn node(second_journal_seq: u64) -> Vec<u8> {
        // Header up to the first bset: magic, flags, min/max key, format.
        let mut head = vec![0u8; HEADER_BSET - 16];
        head[0..8].copy_from_slice(&MAGIC.to_le_bytes());
        head[64] = 5; // bkey_format: key_u64s 5, nr_fields 6, no bits
        head[65] = 6;
        let first = record(5, &key(1));
        // The first record's checksum covers the header too.
        let mut b = vec![0u8; 16];
        b.extend_from_slice(&head);
        b.extend_from_slice(&first[16..]);
        let c = crate::csum::crc32c_nonzero(&b[16..]);
        b[0..4].copy_from_slice(&c.to_le_bytes());
        b.resize(512, 0);
        b.extend_from_slice(&record(second_journal_seq, &key(2)));
        b.resize(1024, 0);
        b
    }

    #[test]
    fn a_bset_in_a_blacklisted_range_is_ignored_and_one_at_the_range_end_is_not() {
        let keys = |js: u64, blacklist: &[(u64, u64)]| {
            Node::parse_filtered(&node(js), MAGIC, 512, Some(SEQ), u64::MAX, blacklist)
                .unwrap()
                .keys
                .len()
        };
        assert_eq!(keys(15, &[]), 2, "no blacklist: both bsets");
        assert_eq!(keys(15, &[(10, 20)]), 1, "inside the range: ignored");
        assert_eq!(keys(10, &[(10, 20)]), 1, "at the start: ignored");
        assert_eq!(keys(20, &[(10, 20)]), 2, "at the end: live (exclusive end)");
        assert_eq!(keys(5, &[(10, 20)]), 2, "below the range: live");
        assert_eq!(keys(15, &[(0, 3), (10, 20)]), 1, "any of several ranges");
        // The replay bound still applies on top.
        assert_eq!(
            Node::parse_filtered(&node(30), MAGIC, 512, Some(SEQ), 29, &[])
                .unwrap()
                .keys
                .len(),
            1
        );
    }

    #[test]
    fn a_torn_later_bset_cannot_satisfy_the_parents_written_sectors() {
        for seq in [0, SEQ + 1] {
            let mut b = node(15);
            b[528..536].copy_from_slice(&seq.to_le_bytes());
            assert!(
                matches!(
                    Node::parse(&b, MAGIC, 512, Some(SEQ)),
                    Err(Error::Corrupt(_))
                ),
                "a valid first sector must not stand in for two written sectors"
            );
        }
    }

    #[test]
    fn missing_later_bset_headers_cannot_satisfy_the_parents_written_sectors() {
        for extra in [1, 16, 39, 512] {
            let mut b = node(15);
            b.truncate(512);
            b.resize(512 + extra, 0);
            assert!(
                matches!(
                    Node::parse(&b, MAGIC, 512, Some(SEQ)),
                    Err(Error::Corrupt(_))
                ),
                "{extra} bytes beyond the first record have no complete matching bset"
            );
        }
    }

    #[test]
    fn complete_multiple_bsets_and_their_block_padding_are_accepted() {
        assert_eq!(
            Node::parse(&node(15), MAGIC, 512, Some(SEQ))
                .unwrap()
                .keys
                .len(),
            2
        );
        let mut b = node(15);
        b.truncate(512);
        b[200..].fill(0xff);
        assert_eq!(
            Node::parse(&b, MAGIC, 512, Some(SEQ)).unwrap().keys.len(),
            1
        );
    }

    #[test]
    fn the_reference_case_requiring_61_sectors_but_finding_37_is_refused() {
        // Reference-made default.img from CI run 38554478803, its extent
        // node at byte 3014656, limited to the parent's 61 written sectors.
        // Differential run 38579070302, seed 42, tore the bset at sector 37
        // ("node offset 37/61: btree node data missing: expected 61
        // sectors, found 37"; the reference exited 9). Four bytes of that
        // bset's sequence stand in for the tear here.
        let mut b = include_bytes!("../tests/data/default-extents-38554478803.bin").to_vec();
        let magic = le64(&b, 16);
        let seq = le64(&b, HEADER_BSET);
        assert!(!Node::parse(&b, magic, 512, Some(seq))
            .unwrap()
            .keys
            .is_empty());
        b[18960..18964].copy_from_slice(&0xadd0_4627u32.to_le_bytes());
        assert!(
            matches!(
                Node::parse(&b, magic, 512, Some(seq)),
                Err(Error::Corrupt(_))
            ),
            "the original non-crashing reference case must not be accepted as a valid prefix"
        );
    }

    #[test]
    fn an_ignored_bset_is_still_checksummed() {
        let mut b = node(15);
        b[560] ^= 1; // inside the second bset's key (its record spans 512..592)
        assert!(matches!(
            Node::parse_filtered(&b, MAGIC, 512, Some(SEQ), u64::MAX, &[(10, 20)]),
            Err(Error::BadChecksum { .. })
        ));
    }

    /// The backpointers btree's root of `encrypted.img`, from the fixtures
    /// of CI run 39021762764, as the reference formatter wrote it: an
    /// interior node of two records, the header's (bytes 0..304, journal
    /// sequence 0) and an entry at byte 512 (512..696, journal sequence
    /// 0x2b), each holding two `btree_ptr_v2` keys, ChaCha20-encrypted and
    /// MAC'd under the master key below, which the image's crypt field
    /// stores unsealed. The clean field gives the root's level (1) and its
    /// position (SPOS_MAX).
    const ENCRYPTED_NODE: &[u8] =
        include_bytes!("../tests/data/encrypted-backpointers-root-39021762764.bin");
    const ENCRYPTED_MAGIC: u64 = 0x1d5a_a64f_da25_5056;
    const ENCRYPTED_SEQ: u64 = 0x20fc_092d_3613_f1cd;

    fn encrypted_key() -> crate::crypt::MasterKey {
        crate::crypt::MasterKey::new([
            0x4b, 0xb0, 0x49, 0x86, 0xf2, 0x3a, 0x34, 0x30, 0x7c, 0x1f, 0xf3, 0x81, 0xd3, 0x34,
            0x4a, 0x8a, 0x6a, 0xe1, 0xcf, 0x6c, 0xc2, 0x89, 0xfc, 0x8e, 0xf8, 0x4b, 0xb4, 0x10,
            0x70, 0x88, 0x9c, 0xc3,
        ])
    }

    fn parse_encrypted(b: &[u8], key: Option<&crate::crypt::MasterKey>) -> Result<Node> {
        Node::parse_keyed(
            b,
            ENCRYPTED_MAGIC,
            512,
            Some(ENCRYPTED_SEQ),
            u64::MAX,
            &[],
            key,
        )
    }

    #[test]
    fn an_encrypted_node_is_verified_and_decrypted() {
        let node = parse_encrypted(ENCRYPTED_NODE, Some(&encrypted_key())).unwrap();
        // The decrypted header names the btree and level, and the bounds,
        // that the superblock's root pointer gives.
        assert_eq!(flags_id_and_level(node.flags), (13, 1));
        assert_eq!((node.min_key, node.max_key), (Bpos::default(), Bpos::MAX));
        let keys: Vec<(u8, Bpos)> = node.keys.iter().map(|k| (k.key_type, k.pos)).collect();
        assert_eq!(
            keys,
            [
                (
                    key_type::BTREE_PTR_V2,
                    Bpos {
                        inode: 0,
                        offset: 954_662_912,
                        snapshot: 0
                    }
                ),
                (key_type::BTREE_PTR_V2, Bpos::MAX),
            ]
        );
    }

    /// One byte changed in either record -- its MAC, the first record's
    /// encrypted header, a bset header stored in the clear, the keys -- is
    /// refused by name before anything is decrypted.
    #[test]
    fn an_encrypted_node_whose_mac_does_not_verify_is_refused() {
        let key = encrypted_key();
        for at in [0, 90, 144, 200, 512, 536, 600] {
            let mut b = ENCRYPTED_NODE.to_vec();
            b[at] ^= 1;
            match parse_encrypted(&b, Some(&key)) {
                Err(Error::BadChecksum { what, .. }) => {
                    assert_eq!(what, "btree node MAC", "byte {at}")
                }
                other => panic!("byte {at}: {other:?}"),
            }
        }
    }

    #[test]
    fn an_encrypted_node_is_refused_without_its_key_or_with_another() {
        assert!(matches!(
            parse_encrypted(ENCRYPTED_NODE, None),
            Err(Error::Unsupported(_))
        ));
        let other = crate::crypt::MasterKey::new([0x5a; 32]);
        assert!(matches!(
            parse_encrypted(ENCRYPTED_NODE, Some(&other)),
            Err(Error::BadChecksum { .. })
        ));
    }

    /// An encrypted filesystem authenticates all of its metadata (S1
    /// 2.1.2): a node, or a later bset, that carries a checksum instead of
    /// a MAC is refused, not read unauthenticated.
    #[test]
    fn a_bset_without_a_mac_is_refused_in_an_encrypted_filesystem() {
        let key = encrypted_key();
        assert!(matches!(
            Node::parse_keyed(&node(6), MAGIC, 512, Some(SEQ), u64::MAX, &[], Some(&key)),
            Err(Error::Corrupt(_))
        ));
        // The second record's flags (bset at 528, flags at 544): checksum
        // type 4, the MAC, becomes crc32c.
        let mut b = ENCRYPTED_NODE.to_vec();
        assert_eq!(b[544] & 0xf, crate::crypt::CSUM_CHACHA20_POLY1305_128);
        b[544] = (b[544] & !0xf) | 1;
        assert!(matches!(
            parse_encrypted(&b, Some(&key)),
            Err(Error::Corrupt(_))
        ));
    }
}
