//! Making and retiring btree nodes: a node that is full is rewritten into a
//! fresh bucket with its live keys in one bset -- or split in two when they
//! would fill most of a node -- and the bucket it came from is freed.
//!
//! Every part of this is the write path's own pieces put together: a node
//! is written as `create_root` writes the empty roots the reference was
//! seen to make (S8), its bucket gets the alloc key, backpointer, bitmap
//! bit and accounting of a btree bucket (S8: create-large's lru node), and
//! the old bucket is freed as an emptied data bucket is (S8: unlink-large).
//! The reference checker judges the result (tests/write_oracle.rs).

use super::{alloc, encode_key, Txn, Writer};
use crate::bkey::{key_type, Bkey, Bpos};
use crate::btree::{self, NodePtr};
use crate::error::{Error, Result};
use crate::util::le64;
use fs_core::BlockDevice;

pub(super) const SPOS_MAX: Bpos = Bpos {
    inode: u64::MAX,
    offset: u64::MAX,
    snapshot: u32::MAX,
};

/// The position just after `p` in key order (inode, offset, snapshot).
fn successor(p: Bpos) -> Bpos {
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

/// A rewritten node is filled to at most this fraction of a node, so that
/// it takes appends again; beyond it the keys are split between two.
const FILL_NUMERATOR: usize = 2;
const FILL_DENOMINATOR: usize = 3;

impl<D: BlockDevice> Writer<D> {
    /// Write a new node of btree `id` at `level` covering `min..=max` and
    /// holding `keys` (sorted, unpacked) in one bset, in a bucket of its
    /// own. Returns the pointer key for it, at `max`.
    pub(super) fn new_node(
        &mut self,
        id: u8,
        level: u8,
        min: Bpos,
        max: Bpos,
        keys: &[Bkey],
        t: &mut Txn,
    ) -> Result<Bkey> {
        if level >= 16 {
            return Err(Error::Unsupported(format!("a node at level {level}")));
        }
        let bucket_sectors = self.bucket_sectors()?;
        let node_sectors = u64::from(self.sb.btree_node_size());
        // One node per bucket, at its start (#109). A node smaller than its
        // bucket leaves the rest of it unused, which the bucket's alloc key
        // and the accounting record (count_btree_bucket: the device's btree
        // buckets less what their nodes use). The formatter requires the
        // bucket to be at least the node size.
        if node_sectors > bucket_sectors {
            return Err(Error::Corrupt("btree nodes larger than a bucket".into()));
        }
        let block = (self.sb.block_size as usize * 512).max(512);
        // A template: the extents btree's root node, for the header flags
        // above the id and level, the checksum type and the version.
        let (_, ext_root) = self.root(super::ids::EXTENTS)?;
        let ext_ptr = NodePtr::from_key(&ext_root)?;
        let mut tmpl = vec![0u8; 160];
        self.dev.read_at(ext_ptr.ptrs[0].offset * 512, &mut tmpl)?;
        let tmpl_flags = le64(&tmpl, 24);
        let csum_type = (crate::util::le32(&tmpl, 136 + 16) & 0xf) as u8;
        let version = crate::util::le16(&tmpl, 136 + 20);

        // A node may use the buckets data leaves free for it (NODE_RESERVE).
        let bucket = self.take_buckets(1, 0, t)?[0];
        let gen = u64::from(self.bucket_gen(bucket)?);
        let seq =
            crate::siphash::siphash24(self.journal_seq, bucket, &min.offset.to_le_bytes()) | 1;
        let mut body = Vec::new();
        for k in keys {
            body.extend(encode_key(k)?);
        }
        let u64s = u16::try_from(body.len() / 8)
            .map_err(|_| Error::Unsupported("a node of more than 65535 u64s".into()))?;
        let mut node = vec![0u8; 160];
        node[16..24].copy_from_slice(&btree::node_magic(&self.sb.uuid).to_le_bytes());
        // The id and level where the reference puts them
        // (`btree::flags_with_id_and_level`); the rest as the template has them.
        let flags = btree::flags_with_id_and_level(tmpl_flags, id, level);
        node[24..32].copy_from_slice(&flags.to_le_bytes());
        node[32..36].copy_from_slice(&min.snapshot.to_le_bytes());
        node[36..44].copy_from_slice(&min.offset.to_le_bytes());
        node[44..52].copy_from_slice(&min.inode.to_le_bytes());
        node[52..56].copy_from_slice(&max.snapshot.to_le_bytes());
        node[56..64].copy_from_slice(&max.offset.to_le_bytes());
        node[64..72].copy_from_slice(&max.inode.to_le_bytes());
        node[80..88].copy_from_slice(&[3, 6, 64, 64, 32, 0, 0, 0]);
        node[136..144].copy_from_slice(&seq.to_le_bytes());
        node[144..152].copy_from_slice(&self.journal_seq.to_le_bytes());
        node[152..156].copy_from_slice(&u32::from(csum_type).to_le_bytes());
        node[156..158].copy_from_slice(&version.to_le_bytes());
        node[158..160].copy_from_slice(&u64s.to_le_bytes());
        node.extend_from_slice(&body);
        let csum = crate::csum::compute(csum_type, &node[16..])?;
        node[0..8].copy_from_slice(&csum.to_le_bytes());
        let len = node.len().div_ceil(block) * block;
        if len > node_sectors as usize * 512 {
            return Err(Error::Corrupt("the keys do not fit a node".into()));
        }
        node.resize(len, 0);
        let dev_sector = bucket * bucket_sectors;
        self.dev.write_at(dev_sector * 512, &node)?;

        let mut v = Vec::with_capacity(48);
        v.extend_from_slice(&0u64.to_le_bytes());
        v.extend_from_slice(&seq.to_le_bytes());
        v.extend_from_slice(&((len / 512) as u16).to_le_bytes());
        v.extend_from_slice(&0u16.to_le_bytes());
        v.extend_from_slice(&min.snapshot.to_le_bytes());
        v.extend_from_slice(&min.offset.to_le_bytes());
        v.extend_from_slice(&min.inode.to_le_bytes());
        v.extend_from_slice(&(1u64 | dev_sector << 4 | gen << 56).to_le_bytes());
        let ptr = Bkey {
            key_type: key_type::BTREE_PTR_V2,
            size: 0,
            version_hi: 0,
            version_lo: 0,
            pos: max,
            value: v,
        };
        self.mark_btree_bitmap(dev_sector, node_sectors)?;
        self.account_btree_bucket(id, level, bucket, gen, max, node_sectors, t)?;
        Ok(ptr)
    }

    /// Rewrite the full node `ptr_key` points at, with `extra` keys applied
    /// over its live keys: one node, or two when they would fill most of
    /// one. The old node's bucket is freed. Returns the pointer keys that
    /// replace `ptr_key` in its parent, by position.
    pub(super) fn rewrite_node(
        &mut self,
        id: u8,
        level: u8,
        ptr_key: &Bkey,
        extra: &[Bkey],
        t: &mut Txn,
    ) -> Result<Vec<Bkey>> {
        let ptr = NodePtr::from_key(ptr_key)?;
        let node = btree::read_node(&self.dev, &self.sb, &ptr)?;
        let mut live: std::collections::BTreeMap<Bpos, Bkey> =
            node.keys.into_iter().map(|k| (k.pos, k)).collect();
        for k in extra {
            if k.key_type == key_type::DELETED || k.key_type == key_type::WHITEOUT {
                live.remove(&k.pos);
            } else {
                live.insert(k.pos, k.clone());
            }
        }
        let keys: Vec<Bkey> = live.into_values().collect();
        let bytes: usize = keys.iter().map(|k| 40 + k.value.len()).sum();
        let room =
            (self.sb.btree_node_size() as usize * 512 - 160) * FILL_NUMERATOR / FILL_DENOMINATOR;
        let (min, max) = (node.min_key, node.max_key);
        let out = if bytes <= room || keys.len() < 2 {
            vec![self.new_node(id, level, min, max, &keys, t)?]
        } else {
            // Split by bytes into as many nodes as the keys need, each
            // filled about evenly (#113: a reclaim inserts hundreds of keys
            // into one node at once, more than two halves can hold). Each
            // node ends at its last key; the last one ends at the old max.
            let parts = bytes.div_ceil(room).max(2);
            let target = bytes.div_ceil(parts);
            let mut nodes = Vec::with_capacity(parts);
            let (mut from, mut acc, mut lo) = (0, 0, min);
            for (i, k) in keys.iter().enumerate() {
                acc += 40 + k.value.len();
                let last = i + 1 == keys.len();
                if !last && acc >= target && nodes.len() + 1 < parts {
                    let hi = k.pos;
                    nodes.push(self.new_node(id, level, lo, hi, &keys[from..=i], t)?);
                    (from, acc, lo) = (i + 1, 0, successor(hi));
                }
            }
            nodes.push(self.new_node(id, level, lo, max, &keys[from..], t)?);
            nodes
        };
        self.free_node(id, level, &ptr, ptr_key.pos, t)?;
        // The pointer standing for the node keeps its position (the node's
        // max key); a split adds one before it.
        Ok(out
            .into_iter()
            .map(|mut k| {
                if k.pos == max {
                    k.pos = ptr_key.pos;
                }
                k
            })
            .collect())
    }
}

// Allocation bookkeeping for btree buckets, beside the data buckets'.
impl<D: BlockDevice> Writer<D> {
    /// The keys and counters a bucket holding one node of btree `id` at
    /// `level` carries (S8): its alloc key (data type btree, the whole node
    /// dirty), its backpointer naming the pointer key one level up, and the
    /// btree's share of the accounting.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn account_btree_bucket(
        &mut self,
        id: u8,
        level: u8,
        bucket: u64,
        gen: u64,
        ptr_pos: Bpos,
        node_sectors: u64,
        t: &mut Txn,
    ) -> Result<()> {
        let bucket_sectors = self.bucket_sectors()?;
        let clock = self.write_clock()?;
        let mut a = Vec::with_capacity(64);
        for w in [
            self.journal_seq,
            u64::from(alloc::DATA_BTREE) << 48 | gen << 40 | gen << 32 | 0x23,
            node_sectors,
            1,
            clock,
            0,
            0,
            0,
        ] {
            a.extend_from_slice(&w.to_le_bytes());
        }
        t.put_uncounted(
            alloc::aids::ALLOC,
            Bkey {
                key_type: alloc::aids::ALLOC_V4,
                size: 0,
                version_hi: 0,
                version_lo: 0,
                pos: Bpos {
                    inode: 0,
                    offset: bucket,
                    snapshot: 0,
                },
                value: a,
            },
        );
        let mut bp = Vec::with_capacity(32);
        bp.extend_from_slice(&[id, level + 1, alloc::DATA_BTREE, 0, 0, 0, 0, 0]);
        bp.extend_from_slice(&(node_sectors as u32).to_le_bytes());
        bp.extend_from_slice(&ptr_pos.snapshot.to_le_bytes());
        bp.extend_from_slice(&ptr_pos.offset.to_le_bytes());
        bp.extend_from_slice(&ptr_pos.inode.to_le_bytes());
        t.put_uncounted(
            alloc::aids::BACKPOINTERS,
            Bkey {
                key_type: alloc::aids::BACKPOINTER,
                size: 0,
                version_hi: 0,
                version_lo: 0,
                pos: Bpos {
                    inode: 0,
                    offset: (bucket * bucket_sectors) << 16,
                    snapshot: 0,
                },
                value: bp,
            },
        );
        // A node smaller than its bucket leaves it partly filled, which
        // takes a fragmentation lru entry as a partly filled data bucket does
        // (the reference checker's "missing fragmentation lru entry" for the
        // buckets of new nodes, CI run 38051787458). The lru btree gets its
        // root first if it has none; the root's own node is in that btree.
        if node_sectors < bucket_sectors {
            if id != alloc::aids::LRU && self.root(alloc::aids::LRU).is_err() {
                self.create_root(alloc::aids::LRU, t)?;
            }
            t.put_uncounted(
                alloc::aids::LRU,
                Bkey {
                    key_type: alloc::aids::SET,
                    size: 0,
                    version_hi: 0,
                    version_lo: 0,
                    pos: Bpos {
                        inode: (1 << 61) | ((node_sectors << 31) / bucket_sectors),
                        offset: bucket,
                        snapshot: 0,
                    },
                    value: Vec::new(),
                },
            );
        }
        alloc::count_btree_bucket(t, id, level, node_sectors as i64, bucket_sectors as i64, 1);
        Ok(())
    }

    /// Free the bucket of a node no longer in its btree: as an emptied data
    /// bucket is freed (need_discard, a new generation), with its
    /// backpointer and its btree's accounting gone.
    pub(super) fn free_node(
        &mut self,
        id: u8,
        level: u8,
        ptr: &NodePtr,
        _ptr_pos: Bpos,
        t: &mut Txn,
    ) -> Result<()> {
        let bucket_sectors = self.bucket_sectors()?;
        let node_sectors = u64::from(self.sb.btree_node_size());
        let p = ptr.ptrs[0];
        let bucket = p.offset / bucket_sectors;
        // The bucket's alloc key as this transaction leaves it so far: an
        // earlier node of the same bucket may have been freed in it already.
        let pending = t
            .keys
            .get(&alloc::aids::ALLOC)
            .into_iter()
            .flatten()
            .chain(
                t.inflight
                    .iter()
                    .filter(|(id, _)| *id == alloc::aids::ALLOC)
                    .flat_map(|(_, keys)| keys),
            )
            .rfind(|k| k.pos.inode == 0 && k.pos.offset == bucket)
            .cloned();
        let a = match pending {
            Some(a) => a,
            None => self
                .keys(alloc::aids::ALLOC)?
                .into_iter()
                .find(|k| k.pos.inode == 0 && k.pos.offset == bucket)
                .ok_or_else(|| Error::Corrupt(format!("node bucket {bucket} has no alloc key")))?,
        };
        t.delete_uncounted(
            alloc::aids::BACKPOINTERS,
            Bpos {
                inode: 0,
                offset: p.offset << 16,
                snapshot: 0,
            },
        );
        // A bucket the reference filled with several nodes (#109) keeps the
        // others: only this node's sectors go, and the bucket is emptied
        // once none is left. Emptying it with live nodes in it let a later
        // write reuse the bucket over them (#132's reuse, CI run
        // 38048734742: "btree node: bad magic").
        let dirty = crate::util::le64(&a.value, 16) & 0xffff_ffff;
        // A partly filled bucket has a fragmentation lru entry (S8), btree
        // buckets as data ones: it moves with the bucket's dirty count, and
        // goes when the bucket is emptied (the reference checker's
        // "incorrect lru entry" and "missing fragmentation lru entry",
        // CI run 38051787458).
        let lru_at = |sectors: u64| Bpos {
            inode: (1 << 61) | ((sectors << 31) / bucket_sectors),
            offset: bucket,
            snapshot: 0,
        };
        if dirty < bucket_sectors {
            t.delete_uncounted(alloc::aids::LRU, lru_at(dirty));
        }
        if dirty > node_sectors {
            let mut v = a.value.clone();
            v[16..20].copy_from_slice(&((dirty - node_sectors) as u32).to_le_bytes());
            t.put_uncounted(
                alloc::aids::ALLOC,
                Bkey {
                    value: v,
                    ..a.clone()
                },
            );
            t.put_uncounted(
                alloc::aids::LRU,
                Bkey {
                    key_type: alloc::aids::SET,
                    size: 0,
                    version_hi: 0,
                    version_lo: 0,
                    pos: lru_at(dirty - node_sectors),
                    value: Vec::new(),
                },
            );
            alloc::count_btree_node(t, id, level, -(node_sectors as i64), -1);
            return Ok(());
        }
        self.empty_bucket(&a, t)?;
        alloc::count_btree_bucket(
            t,
            id,
            level,
            -(node_sectors as i64),
            bucket_sectors as i64,
            -1,
        );
        t.count(
            alloc::acct_dev_data_type(0, alloc::DATA_NEED_DISCARD),
            3,
            0,
            1,
        );
        Ok(())
    }
}
