//! Allocation, for files too large to store inline: whole free buckets are
//! taken from the freespace btree, the data written into them with
//! checksummed extents, and every key the reference implementation was
//! seen to write for a large file is written with them (the write study's
//! create-large pair, S8; docs/clean-room.md "Allocating space").

use super::{ids, pos, Txn, Writer};
use crate::bkey::{key_type, Bkey, Bpos};
use crate::error::{Error, Result};
use crate::util::le64;
use fs_core::BlockDevice;

/// Btrees and key types allocation touches (S1's orders; checked against
/// the reference lister on the write study's images).
/// The reflink btree (S1 11.3): indirect extents, at inode 0 and the end
/// of their range.
const REFLINK: u8 = crate::btree::btree_id::REFLINK;

pub(super) mod aids {
    pub const ALLOC: u8 = 4;
    pub const LRU: u8 = 10;
    pub const FREESPACE: u8 = 11;
    pub const BACKPOINTERS: u8 = 13;
    pub const BUCKET_GENS: u8 = 14;
    pub const SET: u8 = 25;
    pub const ALLOC_V4: u8 = 27;
    pub const BACKPOINTER: u8 = 28;
    pub const BUCKET_GENS_KEY: u8 = 30;
}

/// The largest extent one crc32 entry describes: its sizes are 7 bits,
/// stored minus one (src/extent.rs).
const MAX_EXTENT_SECTORS: u64 = 128;

/// Data types in alloc keys, accounting and backpointers (S1's order; the
/// lister names 1 sb, 2 journal, 3 btree, 4 user).
pub(super) const DATA_USER: u8 = 4;

/// A bucket's data type sits in byte 6 of the alloc_v4 value's second
/// word; the low byte of that word held 0x23 in every bucket the reference
/// filled with data (S8, every user bucket of every fixture).
const ALLOC_FLAGS_USER: u64 = (DATA_USER as u64) << 48 | 0x23;

/// The accounting key kinds and their positions (S8): the first byte of the
/// position's inode field is the kind, and the fields follow it byte by
/// byte.
fn acct_replicas_user(dev: u8) -> Bpos {
    Bpos {
        inode: 0x0204_0101_0000_0000 | u64::from(dev) << 24,
        offset: 0,
        snapshot: 0,
    }
}

pub(super) fn acct_dev_data_type(dev: u8, data_type: u8) -> Bpos {
    Bpos {
        inode: 0x0300_0000_0000_0000 | u64::from(dev) << 48 | u64::from(data_type) << 40,
        offset: 0,
        snapshot: 0,
    }
}

/// The per-inode accounting key: kind 8, then the inode number's bytes in
/// little-endian order, read as a big-endian position (S8: inode
/// 0x80000003 at 0x0803000080000000).
fn acct_inum(ino: u64) -> Bpos {
    let mut b = [0u8; 16];
    b[0] = 8;
    b[1..9].copy_from_slice(&ino.to_le_bytes());
    Bpos {
        inode: u64::from_be_bytes(b[0..8].try_into().expect("8 bytes")),
        offset: u64::from_be_bytes(b[8..16].try_into().expect("8 bytes")),
        snapshot: 0,
    }
}

/// One extent this writer is about to write.
pub(super) struct Planned {
    pub sectors: u64,
    pub dev_sector: u64,
    pub bucket: u64,
    pub gen: u8,
}

/// Buckets data may not take: they are kept for the btree nodes a commit
/// writes (#168). A commit writes each btree's change to disk as it goes, so
/// a node split that finds no bucket after the extents btree has been written
/// left a write that returned an error but had changed the file. A data write
/// that would leave fewer is refused before anything is written. 32 is about
/// 1.6% of every fixture's 2048 buckets, and more than the nodes of every
/// btree one transaction touches splitting at once.
pub(super) const NODE_RESERVE: u64 = 32;

impl<D: BlockDevice> Writer<D> {
    pub(super) fn bucket_sectors(&self) -> Result<u64> {
        let members = self.sb.members()?;
        let m = members
            .first()
            .ok_or_else(|| Error::Corrupt("no member device".into()))?;
        Ok(u64::from(m.bucket_size))
    }

    /// Take `n` whole free buckets, lowest first, from the freespace btree's
    /// runs (keys of type `set` at `dev:end`, `size` buckets long, S8),
    /// leaving at least `keep` free after them. Returns the buckets and the
    /// keys that shrink the runs. A shortfall is refused before anything is
    /// recorded in `t`.
    pub(super) fn take_buckets(&mut self, n: u64, keep: u64, t: &mut Txn) -> Result<Vec<u64>> {
        let runs: Vec<Bkey> = self
            .keys(aids::FREESPACE)?
            .into_iter()
            .filter(|k| k.key_type == aids::SET && k.pos.inode == 0)
            .collect();
        let mut free = 0u64;
        for run in &runs {
            let end = run.pos.offset;
            let start = end.saturating_sub(u64::from(run.size));
            free += (start..end).filter(|b| !self.reserved.contains(b)).count() as u64;
        }
        if free < n.saturating_add(keep) {
            return Err(Error::Unsupported(if keep == 0 {
                format!("{n} free buckets are needed and only {free} are free")
            } else {
                format!(
                    "{n} free buckets are needed and only {free} are free, of which {keep} \
                     are kept for btree nodes"
                )
            }));
        }
        // Buckets with an alloc key of any data type but free (byte 6 of
        // the second word, 0) are not free, whatever freespace says.
        let used: std::collections::BTreeSet<u64> = self
            .keys(aids::ALLOC)?
            .iter()
            .filter(|k| k.pos.inode == 0 && k.value.len() >= 16 && k.value[14] != 0)
            .map(|k| k.pos.offset)
            .collect();
        let mut out = Vec::new();
        for run in runs {
            if out.len() as u64 == n {
                break;
            }
            let end = run.pos.offset;
            let start = end
                .checked_sub(u64::from(run.size))
                .ok_or_else(|| Error::Corrupt("a freespace run before bucket 0".into()))?;
            let mut first_unused = start;
            // Buckets this writer took earlier: the run on disk may not show
            // them taken yet.
            while first_unused < end && self.reserved.contains(&first_unused) {
                first_unused += 1;
            }
            while first_unused < end && (out.len() as u64) < n {
                if self.reserved.contains(&first_unused) {
                    first_unused += 1;
                    continue;
                }
                if used.contains(&first_unused) {
                    return Err(Error::Corrupt(format!(
                        "bucket {first_unused} is free and has an alloc key"
                    )));
                }
                out.push(first_unused);
                first_unused += 1;
            }
            // The run now starts later: the same end, fewer buckets; or it
            // is gone.
            if first_unused == end {
                t.delete_uncounted(aids::FREESPACE, run.pos);
            } else if first_unused > start {
                t.put_uncounted(
                    aids::FREESPACE,
                    Bkey {
                        size: (end - first_unused) as u32,
                        ..run.clone()
                    },
                );
            }
        }
        if (out.len() as u64) < n {
            return Err(Error::Unsupported(format!(
                "{n} free buckets are needed and only {} are free",
                out.len()
            )));
        }
        self.reserved.extend(out.iter().copied());
        Ok(out)
    }

    /// A bucket's generation: byte `bucket % 256` of the bucket_gens key at
    /// `dev:bucket / 256` (type 30, 256 one-byte generations; S3 + S4: the
    /// lister prints them in order and the alloc keys of reused buckets
    /// carry the same), 0 when there is none.
    pub(super) fn bucket_gen(&self, bucket: u64) -> Result<u8> {
        let gens = match self.keys(aids::BUCKET_GENS) {
            Err(Error::NotFound(_)) => return Ok(0),
            r => r?,
        };
        Ok(gens
            .iter()
            .find(|k| {
                k.key_type == aids::BUCKET_GENS_KEY
                    && k.pos.inode == 0
                    && k.pos.offset == bucket >> 8
            })
            .and_then(|k| k.value.get((bucket & 0xff) as usize).copied())
            .unwrap_or(0))
    }

    /// The largest write clock any bucket recorded (alloc_v4 word 4), for
    /// the buckets this writer fills.
    pub(super) fn write_clock(&self) -> Result<u64> {
        Ok(self
            .keys(aids::ALLOC)?
            .iter()
            .filter(|k| k.key_type == aids::ALLOC_V4 && k.value.len() >= 40)
            .map(|k| le64(&k.value, 32))
            .max()
            .unwrap_or(1))
    }

    /// Write `data` for inode `ino` into fresh buckets and add every key
    /// that goes with it to `t`. Returns the sectors allocated.
    /// `opts` is the file's data checksum and compression as superblock
    /// option numbers ([`super::data_options`]). `base` is the file sector
    /// `data` starts at, a whole block.
    pub(super) fn allocate_data(
        &mut self,
        ino: u64,
        opts: (u64, u64),
        base: u64,
        data: &[u8],
        t: &mut Txn,
    ) -> Result<u64> {
        let bucket = self.bucket_sectors()?;
        if bucket == 0 {
            return Err(Error::Corrupt("bucket size 0".into()));
        }
        // The entry before each pointer, by the data checksum option, as the
        // formatter's fixtures show it (S3, #105): crc32c, a crc32 entry of
        // checksum type 5 (from zero, not inverted, S4); crc64 and xxhash, a
        // crc64 entry of type 6 and 7, the whole 64-bit checksum in its
        // second word; none, no entry, only the pointer. Type 6 for crc64
        // data is INFERRED (5 is crc32c's data-side type; csum.rs).
        let csum_type = match opts.0 {
            0 => 0u8,
            1 => 5,
            2 => 6,
            3 => 7,
            o => {
                return Err(Error::Unsupported(format!(
                    "data checksum option {o} is not known"
                )))
            }
        };
        // The filesystem's compression (#105): the option lz4 1, gzip 2,
        // zstd 3 (superblock::compression_opt) is crc compression type 3, 2
        // and 4. Each piece is compressed and kept so only when that saves
        // a whole block; otherwise it is stored as it is and marked
        // incompressible, as the reference stores data it cannot compress
        // (S3: the lz4 fixture's random file).
        use crate::extent::compression;
        let codec = match opts.1 {
            0 => None,
            1 => Some(compression::LZ4),
            // gzip as a raw deflate stream, as the reference kernel module
            // stores it and reads back what this writer stores (#141).
            2 => Some(compression::GZIP),
            3 => Some(compression::ZSTD),
            o => {
                return Err(Error::Unsupported(format!(
                    "compression option {o} is not known"
                )))
            }
        };
        if codec.is_some() && csum_type == 0 {
            return Err(Error::Unsupported(
                "compression without a data checksum: no entry has been observed to carry it"
                    .into(),
            ));
        }
        // Data is written in whole blocks: the last one is zero-padded,
        // and the extent and its checksum cover it (S8: 1500 bytes are 3
        // sectors on 512-byte blocks and 8 on 4096-byte ones). Buckets
        // and the longest extent are whole blocks, so every extent the plan
        // cuts starts and ends on a block.
        let block = u64::from(self.sb.block_size).max(1);
        if !bucket.is_multiple_of(block) || !MAX_EXTENT_SECTORS.is_multiple_of(block) {
            return Err(Error::Unsupported(format!(
                "{block}-sector blocks do not divide the bucket ({bucket} sectors)"
            )));
        }
        let sectors = (data.len() as u64).div_ceil(block * 512) * block;

        // The pieces: at most 128 sectors of the file each, and no more than
        // a bucket, then what is
        // stored for each, compressed or not, padded to whole blocks.
        struct Piece {
            file_sector: u64,
            sectors: u64,
            stored: Vec<u8>,
            stored_sectors: u64,
            compression: u8,
        }
        let mut pieces = Vec::new();
        let mut at = 0u64;
        while at < sectors {
            // No piece is longer than a bucket, so each fits one.
            let n = MAX_EXTENT_SECTORS.min(bucket).min(sectors - at);
            let from = (at * 512) as usize;
            let to = (from + (n * 512) as usize).min(data.len());
            let mut raw = data[from..to].to_vec();
            raw.resize((n * 512) as usize, 0);
            let (stored, ty) = match codec {
                Some(t) => {
                    let mut c = crate::compress::compress(t, &raw)?;
                    let c_sectors = (c.len() as u64).div_ceil(block * 512) * block;
                    if c_sectors < n {
                        c.resize((c_sectors * 512) as usize, 0);
                        (c, t)
                    } else {
                        (raw, compression::INCOMPRESSIBLE)
                    }
                }
                None => (raw, compression::NONE),
            };
            let stored_sectors = stored.len() as u64 / 512;
            pieces.push(Piece {
                file_sector: at,
                sectors: n,
                stored,
                stored_sectors,
                compression: ty,
            });
            at += n;
        }

        // Pack the stored pieces into buckets in order, none across two.
        let mut layout: Vec<(usize, u64)> = Vec::new(); // (bucket index, offset)
        let (mut idx, mut used) = (0usize, 0u64);
        for p in &pieces {
            if used + p.stored_sectors > bucket {
                idx += 1;
                used = 0;
            }
            layout.push((idx, used));
            used += p.stored_sectors;
        }
        let data_buckets = layout.last().map(|l| l.0 as u64 + 1).unwrap_or(0);
        let disk_sectors: u64 = pieces.iter().map(|p| p.stored_sectors).sum();
        // A bucket left partly empty needs an lru entry, and a filesystem
        // that never had one has no lru btree: its root is made here, in one
        // more bucket (S8: the reference made exactly that for its first
        // large file).
        if !disk_sectors.is_multiple_of(bucket) && self.root(aids::LRU).is_err() {
            self.create_root(aids::LRU, t)?;
        }
        // Data leaves NODE_RESERVE buckets free, so the btree nodes this
        // transaction rewrites always have room (#168).
        let buckets = self.take_buckets(data_buckets, NODE_RESERVE, t)?;
        let clock = self.write_clock()?;
        let mut plan = Vec::new();
        for (p, &(i, off)) in pieces.iter().zip(&layout) {
            let b = buckets[i];
            plan.push(Planned {
                sectors: p.stored_sectors,
                dev_sector: b * bucket + off,
                bucket: b,
                gen: self.bucket_gen(b)?,
            });
        }

        for (p, piece) in plan.iter().zip(&pieces) {
            self.dev.write_at(p.dev_sector * 512, &piece.stored)?;
            let csum = crate::csum::compute(csum_type, &piece.stored)?;
            let ptr: u64 = 1 | p.dev_sector << 4 | u64::from(p.gen) << 56;
            let (c_size, u_size) = (piece.stored_sectors, piece.sectors);
            let mut value = Vec::with_capacity(24);
            match csum_type {
                0 => {}
                5 => {
                    // crc32 entry (type bit 1).
                    let crc: u64 = 0b10
                        | (c_size - 1) << 2
                        | (u_size - 1) << 9
                        | u64::from(csum_type) << 24
                        | u64::from(piece.compression) << 28
                        | (csum & 0xffff_ffff) << 32;
                    value.extend_from_slice(&crc.to_le_bytes());
                }
                _ => {
                    // crc64 entry (type bit 2): the checksum's high 16 bits
                    // in the first word, its low 64 in the second.
                    let crc: u64 = 0b100
                        | (c_size - 1) << 3
                        | (u_size - 1) << 12
                        | u64::from(csum_type) << 40
                        | u64::from(piece.compression) << 44;
                    value.extend_from_slice(&crc.to_le_bytes());
                    value.extend_from_slice(&csum.to_le_bytes());
                }
            }
            // Then the pointer (type bit 0).
            value.extend_from_slice(&ptr.to_le_bytes());
            let extent = Bkey {
                key_type: key_type::EXTENT,
                size: u_size as u32,
                version_hi: 0,
                version_lo: 0,
                pos: pos(ino, base + piece.file_sector + u_size),
                value,
            };
            t.put(ids::EXTENTS, None, extent.clone());
            if piece.compression != compression::NONE {
                // The compression counter (S4: the lz4 fixture): extents,
                // sectors before, sectors after.
                let at = acct_compression(piece.compression);
                t.count(at, 3, 0, 1);
                t.count(at, 3, 1, u_size as i64);
                t.count(at, 3, 2, c_size as i64);
            }
            t.count(super::btree_counter_pos(ids::EXTENTS), 3, 2, c_size as i64);

            // The backpointer: at the data's device sector << 16, naming
            // the extent (S8: btree, level, data type, bucket_len, pos).
            let mut bp = Vec::with_capacity(32);
            bp.extend_from_slice(&[ids::EXTENTS, 0, DATA_USER, 0, 0, 0, 0, 0]);
            bp.extend_from_slice(&(c_size as u32).to_le_bytes());
            bp.extend_from_slice(&extent.pos.snapshot.to_le_bytes());
            bp.extend_from_slice(&extent.pos.offset.to_le_bytes());
            bp.extend_from_slice(&extent.pos.inode.to_le_bytes());
            t.put_uncounted(
                aids::BACKPOINTERS,
                Bkey {
                    key_type: aids::BACKPOINTER,
                    size: 0,
                    version_hi: 0,
                    version_lo: 0,
                    pos: Bpos {
                        inode: 0,
                        offset: p.dev_sector << 16,
                        snapshot: 0,
                    },
                    value: bp,
                },
            );
        }

        // One alloc key per bucket, and an lru entry for one not full.
        for &b in &buckets {
            let dirty: u64 = plan
                .iter()
                .filter(|p| p.bucket == b)
                .map(|p| p.sectors)
                .sum();
            let mut v = Vec::with_capacity(64);
            let gen = u64::from(
                plan.iter()
                    .find(|p| p.bucket == b)
                    .map(|p| p.gen)
                    .unwrap_or(0),
            );
            for w in [
                self.journal_seq,
                ALLOC_FLAGS_USER | gen << 40 | gen << 32,
                dirty,
                1,
                clock,
                0,
                0,
                0,
            ] {
                v.extend_from_slice(&w.to_le_bytes());
            }
            t.put_uncounted(
                aids::ALLOC,
                Bkey {
                    key_type: aids::ALLOC_V4,
                    size: 0,
                    version_hi: 0,
                    version_lo: 0,
                    pos: Bpos {
                        inode: 0,
                        offset: b,
                        snapshot: 0,
                    },
                    value: v,
                },
            );
            if dirty < bucket {
                // Fragmentation lru (S8): (1 << 61) | dirty/bucket in 2^31ths.
                let frag = (dirty << 31) / bucket;
                t.put_uncounted(
                    aids::LRU,
                    Bkey {
                        key_type: aids::SET,
                        size: 0,
                        version_hi: 0,
                        version_lo: 0,
                        pos: Bpos {
                            inode: 1 << 61 | frag,
                            offset: b,
                            snapshot: 0,
                        },
                        value: Vec::new(),
                    },
                );
            }
        }

        // Accounting (S8): user sectors on the device, the device's user
        // buckets, sectors and the unused part of them, one free bucket
        // fewer each, and the inode's extents and sectors.
        // The device and replicas count sectors as stored; the inode's
        // counter holds its extents, their sectors and the sectors stored
        // (S4: the lz4 fixture's inodes, e.g. 8 extents, 2071, 204).
        let n = buckets.len() as i64;
        let d = disk_sectors as i64;
        t.count(acct_replicas_user(0), 1, 0, d);
        let user = acct_dev_data_type(0, DATA_USER);
        t.count(user, 3, 0, n);
        t.count(user, 3, 1, d);
        t.count(user, 3, 2, n * bucket as i64 - d);
        t.count(acct_dev_data_type(0, 0), 3, 0, -n);
        let inum = acct_inum(ino);
        t.count(inum, 3, 0, plan.len() as i64);
        t.count(inum, 3, 1, sectors as i64);
        t.count(inum, 3, 2, d);
        Ok(sectors)
    }
}

impl<D: BlockDevice> Writer<D> {
    /// Make an empty root node for btree `id` in `bucket`: a header and one
    /// empty bset, as the reference wrote for the lru btree it created
    /// (S8): the btree id and level in the header's flags, the other flag
    /// bits as an existing node of this filesystem carries them, min key
    /// POS_MIN, max key SPOS_MAX, the 64/64/32 unpacked key format. The
    /// root key goes into the superblock's clean field; the bucket's alloc
    /// key, its backpointer and the accounting go into `t`.
    pub(super) fn create_root(&mut self, id: u8, t: &mut Txn) -> Result<()> {
        let root = self.new_node(id, 0, Bpos::default(), SPOS_MAX, &[], t)?;
        if self.session.is_some() {
            // Journalled: every entry from now on records the new root.
            self.session_add_root(id, 0, root);
            self.write_superblock()?;
        } else {
            self.add_root_entry(id, &super::encode_key(&root)?)?;
        }
        Ok(())
    }

    /// Mark the device range of a btree node in the member's btree
    /// allocated bitmap: a u64 at member offset 128, one bit per 2^shift
    /// sectors, the shift a byte at member offset 28 (S3 + S4: the printer's
    /// "Btree allocated bitmap" read most significant bit first, blocksize
    /// 128, against the bytes; the reference set bit 48 for a node at sector
    /// 6144). The reference checker names a node outside it.
    pub(super) fn mark_btree_bitmap(&mut self, dev_sector: u64, sectors: u64) -> Result<()> {
        let mut p = super::SB_HEADER_BYTES;
        let raw = &mut self.sb_raw;
        while p + 8 <= raw.len() {
            let u64s = crate::util::le32(raw, p) as usize;
            let ty = crate::util::le32(raw, p + 4);
            if u64s == 0 {
                break;
            }
            if ty == crate::superblock::FIELD_MEMBERS_V2 {
                let body = p + 8;
                let member_size = crate::util::le16(raw, body) as usize;
                let m = body + 8; // device 0
                if member_size < 136 || m + 136 > p + u64s * 8 {
                    return Err(Error::Corrupt(
                        "members_v2 too short for the btree bitmap".into(),
                    ));
                }
                let mut shift = u32::from(raw[m + 28]);
                let mut bits = le64(raw, m + 128);
                // A node past the 64 regions the bitmap covers: each region
                // doubles, the bits folded pairwise, until it fits -- a
                // superset of what was marked, which is all the checker asks
                // (it names nodes outside the bitmap, S8).
                while (dev_sector + sectors - 1) >> shift >= 64 {
                    let mut folded = 0u64;
                    for i in 0..32 {
                        if (bits >> (2 * i)) & 0b11 != 0 {
                            folded |= 1 << i;
                        }
                    }
                    bits = folded;
                    shift += 1;
                }
                raw[m + 28] = shift as u8;
                let first = dev_sector >> shift;
                let last = (dev_sector + sectors - 1) >> shift;
                for b in first..=last {
                    bits |= 1 << b;
                }
                raw[m + 128..m + 136].copy_from_slice(&bits.to_le_bytes());
                self.sb = crate::superblock::Superblock::parse_unchecked(raw)?;
                return Ok(());
            }
            p += u64s * 8;
        }
        Err(Error::Corrupt("no members_v2 field".into()))
    }

    /// Add a `btree_root` entry for btree `id` at the end of the clean
    /// field, growing the field and the superblock.
    fn add_root_entry(&mut self, id: u8, key: &[u8]) -> Result<()> {
        let mut p = super::SB_HEADER_BYTES;
        let raw = &mut self.sb_raw;
        while p + 8 <= raw.len() {
            let u64s = crate::util::le32(raw, p) as usize;
            let ty = crate::util::le32(raw, p + 4);
            if u64s == 0 {
                break;
            }
            let end = p + u64s * 8;
            if ty == crate::superblock::FIELD_CLEAN {
                let mut entry = Vec::with_capacity(8 + key.len());
                entry.extend_from_slice(&((key.len() / 8) as u16).to_le_bytes());
                entry.extend_from_slice(&[id, 0, 1, 0, 0, 0]);
                entry.extend_from_slice(key);
                let grow = entry.len() / 8;
                raw.splice(end..end, entry);
                raw[p..p + 4].copy_from_slice(&((u64s + grow) as u32).to_le_bytes());
                let total = crate::util::le32(raw, 0x7c) as usize + grow;
                raw[0x7c..0x80].copy_from_slice(&(total as u32).to_le_bytes());
                self.sb = crate::superblock::Superblock::parse_unchecked(raw)?;
                return Ok(());
            }
            p = end;
        }
        Err(Error::Corrupt("no clean field to add a root to".into()))
    }
}

const SPOS_MAX: Bpos = Bpos {
    inode: u64::MAX,
    offset: u64::MAX,
    snapshot: u32::MAX,
};

pub(super) const DATA_BTREE: u8 = 3;

pub(super) fn acct_replicas(data_type: u8, dev: u8) -> Bpos {
    Bpos {
        inode: 0x0200_0101_0000_0000 | u64::from(data_type) << 48 | u64::from(dev) << 24,
        offset: 0,
        snapshot: 0,
    }
}

/// Data type of a bucket emptied and waiting for a discard (S3: the lister
/// names 9 need_discard).
pub(super) const DATA_NEED_DISCARD: u8 = 9;
const NEED_DISCARD: u8 = 12;

impl<D: BlockDevice> Writer<D> {
    /// Remove an inode's extents -- inline ones and allocated ones -- and
    /// give allocated space back the way the reference does (the write
    /// study's unlink-large pair, S8): each bucket loses the extent's
    /// sectors; an emptied bucket becomes need_discard with its generation
    /// and oldest generation one higher, its journal_seq_empty set and
    /// need_inc_gen cleared, gets a need_discard key at
    /// `journal_seq_empty:bucket`, and its generation is recorded in
    /// bucket_gens; backpointers and lru entries go; accounting moves the
    /// buckets from user to need_discard.
    pub(super) fn free_extents(&mut self, ino: u64, extents: &[Bkey], t: &mut Txn) -> Result<()> {
        let bucket = self.bucket_sectors()?;
        let mut freed: std::collections::BTreeMap<u64, u64> = std::collections::BTreeMap::new();
        // Extents, and sectors, by the inode their accounting is under:
        // the file's own, or inode 0 for shared data the reflink btree held
        // (S8: the kernel image's 96 indirect extents, 6144 sectors, are
        // counted at inode 0, and its reflinked files count none).
        let mut per_inode: std::collections::BTreeMap<u64, (i64, i64)> = Default::default();
        // What this file's data is: its own extents, and the indirect
        // extents its reflink pointers drop the last reference to (#140).
        let mut data: Vec<(Bkey, bool)> = Vec::new();
        // Refcounts already lowered in this call, by indirect extent.
        let mut lowered: std::collections::BTreeMap<Bpos, Bkey> = Default::default();
        for k in extents {
            t.delete(ids::EXTENTS, k);
            match k.key_type {
                key_type::INLINE_DATA => {}
                key_type::EXTENT => data.push((k.clone(), false)),
                // A reflink pointer is one reference to each indirect
                // extent it covers (S1 9.1.6.2); the last one frees it.
                key_type::REFLINK_P => {
                    for v in self.unreference(ino, k, &mut lowered)? {
                        data.push((v, true));
                    }
                }
                t => {
                    return Err(Error::Unsupported(format!(
                        "freeing an extent of key type {t}"
                    )))
                }
            }
        }
        for v in lowered.into_values() {
            if v.key_type == key_type::DELETED {
                t.delete_uncounted(REFLINK, v.pos);
            } else {
                t.put_uncounted(REFLINK, v);
            }
        }
        for (k, shared) in &data {
            let e = if *shared {
                crate::extent::DataExtent::from_key(&Bkey {
                    key_type: key_type::EXTENT,
                    value: k.value[8..].to_vec(),
                    ..k.clone()
                })?
            } else {
                crate::extent::DataExtent::from_key(k)?
            };
            if e.ptr.dev != 0 {
                return Err(Error::Unsupported("extents on another device".into()));
            }
            // A piece of a checksummed region (#169): an extent whose crc
            // entry covers more than it does, as the reference leaves where
            // it overwrote part of one. Its bucket counts its live sectors,
            // not the region's, and its backpointer sits at the region's
            // sector << 16 plus its offset in the region (S8, the `aged`
            // image: bucket 105 holds pieces of 111 and 120 sectors of one
            // 256-sector region and counts 231; the piece at offset 136 has
            // its backpointer at 26880 << 16 | 136). Only uncompressed
            // pieces have been seen; a compressed one is refused.
            let compressed = e
                .crc
                .is_some_and(|c| c.compression_type != crate::extent::compression::NONE);
            let piece = e
                .crc
                .is_some_and(|c| c.offset != 0 || u64::from(c.uncompressed_size) != e.len);
            if compressed && piece {
                return Err(Error::Unsupported(
                    "freeing part of a compressed extent is not implemented".into(),
                ));
            }
            let sectors = match e.crc {
                Some(c) if compressed => u64::from(c.compressed_size),
                _ => e.len,
            };
            t.delete_uncounted(
                aids::BACKPOINTERS,
                Bpos {
                    inode: 0,
                    offset: e.ptr.offset << 16 | u64::from(e.crc.map_or(0, |c| c.offset)),
                    snapshot: 0,
                },
            );
            *freed.entry(e.ptr.offset / bucket).or_default() += sectors;
            if let Some(c) = e
                .crc
                .filter(|c| c.compression_type != crate::extent::compression::NONE)
            {
                let at = acct_compression(c.compression_type);
                t.count(at, 3, 0, -1);
                t.count(at, 3, 1, -i64::from(c.uncompressed_size));
                t.count(at, 3, 2, -i64::from(c.compressed_size));
            }
            // The extents btree counts the sectors of its own extents; the
            // reflink btree has no such counter (S8: none in the kernel
            // image's accounting).
            if !*shared {
                t.count(
                    super::btree_counter_pos(ids::EXTENTS),
                    3,
                    2,
                    -(sectors as i64),
                );
            }
            let owner = per_inode.entry(if *shared { 0 } else { ino }).or_default();
            owner.0 += 1;
            owner.1 += sectors as i64;
        }
        if freed.is_empty() {
            return Ok(());
        }
        let total: u64 = freed.values().sum();

        let allocs: std::collections::BTreeMap<u64, Bkey> = self
            .keys(aids::ALLOC)?
            .into_iter()
            .filter(|k| k.pos.inode == 0 && freed.contains_key(&k.pos.offset))
            .map(|k| (k.pos.offset, k))
            .collect();
        let emptied: Vec<u64> = freed
            .iter()
            .filter(|(b, s)| {
                allocs
                    .get(b)
                    .is_some_and(|a| le64(&a.value, 16) as u32 as u64 == **s)
            })
            .map(|(b, _)| *b)
            .collect();
        // Roots the emptied buckets need, made in buckets taken now.
        let mut need_roots = Vec::new();
        if !emptied.is_empty() {
            for id in [NEED_DISCARD, aids::BUCKET_GENS] {
                if self.root(id).is_err() {
                    need_roots.push(id);
                }
            }
        }
        for id in need_roots {
            self.create_root(id, t)?;
        }

        let mut frag_delta = 0i64;
        for (&b, &s) in &freed {
            let a = allocs
                .get(&b)
                .ok_or_else(|| Error::Corrupt(format!("bucket {b} has data and no alloc key")))?;
            let mut v = a.value.clone();
            let old = le64(&v, 16) & 0xffff_ffff;
            let new = old
                .checked_sub(s)
                .ok_or_else(|| Error::Corrupt(format!("bucket {b} frees more than it holds")))?;
            if old < bucket {
                t.delete_uncounted(
                    aids::LRU,
                    Bpos {
                        inode: (1 << 61) | ((old << 31) / bucket),
                        offset: b,
                        snapshot: 0,
                    },
                );
            }
            v[16..20].copy_from_slice(&(new as u32).to_le_bytes());
            if new == 0 {
                self.empty_bucket(
                    &Bkey {
                        value: v,
                        ..a.clone()
                    },
                    t,
                )?;
                frag_delta -= (bucket - old) as i64;
                continue;
            } else {
                t.put_uncounted(
                    aids::LRU,
                    Bkey {
                        key_type: aids::SET,
                        size: 0,
                        version_hi: 0,
                        version_lo: 0,
                        pos: Bpos {
                            inode: (1 << 61) | ((new << 31) / bucket),
                            offset: b,
                            snapshot: 0,
                        },
                        value: Vec::new(),
                    },
                );
                frag_delta += (old - new) as i64;
            }
            t.put_uncounted(
                aids::ALLOC,
                Bkey {
                    value: v,
                    ..a.clone()
                },
            );
        }
        let n_emptied = emptied.len() as i64;
        t.count(acct_replicas_user(0), 1, 0, -(total as i64));
        let user = acct_dev_data_type(0, DATA_USER);
        t.count(user, 3, 0, -n_emptied);
        t.count(user, 3, 1, -(total as i64));
        t.count(user, 3, 2, frag_delta);
        t.count(acct_dev_data_type(0, DATA_NEED_DISCARD), 3, 0, n_emptied);
        for (owner, (nr, sectors)) in per_inode {
            let inum = acct_inum(owner);
            t.count(inum, 3, 0, -nr);
            t.count(inum, 3, 1, -sectors);
            t.count(inum, 3, 2, -sectors);
        }
        Ok(())
    }

    /// Drop the reference the reflink pointer `p` of inode `ino` holds on
    /// each indirect extent it covers: each one's refcount, the first word
    /// of its value (S8: 2 for each of the kernel image's clone pair), goes
    /// down by one in `lowered`, and one that reaches 0 is marked deleted
    /// there and returned, its data to be freed.
    fn unreference(
        &self,
        ino: u64,
        p: &Bkey,
        lowered: &mut std::collections::BTreeMap<Bpos, Bkey>,
    ) -> Result<Vec<Bkey>> {
        let idx = crate::extent::reflink_p_idx(&p.value)?;
        let end = idx + u64::from(p.size);
        let missing = |at: u64| {
            Error::Corrupt(format!(
                "inode {ino}: the reflink btree has nothing at sector {at}, which the \
                 `reflink_p` at {} points into",
                p.pos
            ))
        };
        let mut out = Vec::new();
        let mut at = idx;
        // Through a cursor, read as far as the pointer's run goes: from the
        // first indirect extent ending past `idx` to the one that reaches
        // `end`, or the gap before it. Collecting every key from `idx` on
        // read the whole rest of the reflink btree first (#193).
        let mut c = match self.cursor(REFLINK) {
            // A btree with no root holds nothing: a pointer into it points
            // at nothing.
            Err(Error::NotFound(_)) if self.root(REFLINK).is_err() => {
                return if at < end { Err(missing(at)) } else { Ok(out) };
            }
            r => r?,
        };
        c.seek(Bpos {
            inode: 0,
            offset: idx + 1,
            snapshot: 0,
        })?;
        while let Some(v) = c.next_key()? {
            if v.pos.inode != 0 || at >= end || v.start_offset() > at {
                break;
            }
            if v.key_type != key_type::REFLINK_V || v.value.len() < 8 {
                return Err(Error::Unsupported(format!(
                    "inode {ino}: reflinked data in a key of type {} at {}",
                    v.key_type, v.pos
                )));
            }
            // A pointer that covers part of an indirect extent would split
            // it, which no image has shown yet.
            if v.start_offset() < idx || v.pos.offset > end {
                return Err(Error::Unsupported(format!(
                    "inode {ino}: the `reflink_p` at {} covers part of the indirect extent at {}",
                    p.pos, v.pos
                )));
            }
            at = v.pos.offset;
            let mut v = lowered.remove(&v.pos).unwrap_or(v);
            let count = le64(&v.value, 0);
            if count == 0 || v.key_type == key_type::DELETED {
                return Err(Error::Corrupt(format!(
                    "inode {ino}: the indirect extent at {} has no reference left to drop",
                    v.pos
                )));
            }
            v.value[..8].copy_from_slice(&(count - 1).to_le_bytes());
            if count == 1 {
                out.push(v.clone());
                v.key_type = key_type::DELETED;
            }
            lowered.insert(v.pos, v);
        }
        if at < end {
            return Err(missing(at));
        }
        Ok(out)
    }
}

impl<D: BlockDevice> Writer<D> {
    /// Return the buckets earlier commits emptied to the free pool, as the
    /// reference's discard does on a device it does not discard (#132: the
    /// reference kernel module's `kernel-freed` image, S10, in
    /// docs/clean-room.md, "Allocating space"). A bucket
    /// waiting in need_discard becomes free with its generation kept, its
    /// need_discard flag and both journal sequence numbers cleared; its
    /// need_discard key goes; freespace gets it back, in a run of the freed
    /// buckets beside it (the reference also merges it into an existing
    /// run; this writer leaves those alone, see below); accounting moves it
    /// from need_discard to free. Buckets of
    /// generation 16 or more are left waiting: whether a freespace key
    /// carries a generation's high bits has not been seen
    /// (docs/clean-room.md, "Allocating space"). Committed on its own, so a bucket freed by the transaction
    /// that is being built is never handed out again by it.
    pub(super) fn discard_freed(&mut self) -> Result<()> {
        let waiting = match self.keys(NEED_DISCARD) {
            Err(Error::NotFound(_)) => return Ok(()),
            r => r?,
        };
        let waiting: std::collections::BTreeMap<u64, Bpos> = waiting
            .into_iter()
            .filter(|k| k.key_type == aids::SET)
            .map(|k| (k.pos.offset, k.pos))
            .collect();
        if waiting.is_empty() {
            return Ok(());
        }
        let freed: Vec<Bkey> = self
            .keys(aids::ALLOC)?
            .into_iter()
            .filter(|k| {
                k.pos.inode == 0
                    && waiting.contains_key(&k.pos.offset)
                    && k.key_type == aids::ALLOC_V4
                    && k.value.len() >= 56
                    && k.value[14] == DATA_NEED_DISCARD
                    && k.value[12] < 16
            })
            .collect();
        if freed.is_empty() {
            return Ok(());
        }
        let mut t = Txn::default();
        let mut free: std::collections::BTreeSet<u64> = std::collections::BTreeSet::new();
        for a in &freed {
            let b = a.pos.offset;
            let mut v = a.value.clone();
            let w1 = le64(&v, 8);
            // Data type free, need_discard (flag bit 0) cleared; the
            // generations and the other flags stay.
            let w1 = (w1 & !(0xff << 48)) & !1;
            v[8..16].copy_from_slice(&w1.to_le_bytes());
            v[0..8].copy_from_slice(&0u64.to_le_bytes());
            v[48..56].copy_from_slice(&0u64.to_le_bytes());
            t.put_uncounted(
                aids::ALLOC,
                Bkey {
                    value: v,
                    ..a.clone()
                },
            );
            t.delete_uncounted(NEED_DISCARD, waiting[&b]);
            free.insert(b);
            self.reserved.remove(&b);
        }
        // Runs of their own, one per stretch of adjacent freed buckets.
        // An existing run is never rewritten here: a btree node this
        // commit rewrites takes its bucket from the runs as they stand on
        // disk, and its shrunk run, put under the same key, would replace
        // a merged one and lose the buckets merged in (CI run 38024306750:
        // the reference checker's "bucket incorrectly unset in freespace
        // btree").
        let mut runs: Vec<(u64, u64)> = Vec::new(); // (end, length)
        for &b in &free {
            match runs.last_mut() {
                Some((end, len)) if *end == b => {
                    *end += 1;
                    *len += 1;
                }
                _ => runs.push((b + 1, 1)),
            }
        }
        for (end, len) in runs {
            t.put_uncounted(
                aids::FREESPACE,
                Bkey {
                    key_type: aids::SET,
                    size: len as u32,
                    version_hi: 0,
                    version_lo: 0,
                    pos: Bpos {
                        inode: 0,
                        offset: end,
                        snapshot: 0,
                    },
                    value: Vec::new(),
                },
            );
        }
        let n = free.len() as i64;
        t.count(acct_dev_data_type(0, DATA_NEED_DISCARD), 3, 0, -n);
        t.count(acct_dev_data_type(0, 0), 3, 0, n);
        self.commit(t)
    }

    /// An emptied bucket, its alloc key `a` (dirty sectors already 0): it
    /// becomes need_discard with its generation and oldest generation one
    /// higher, need_inc_gen cleared and journal_seq_empty set, gets a
    /// need_discard key at `journal_seq_empty:bucket`, and bucket_gens
    /// records the new generation (S8: unlink-large). The btrees it needs
    /// get roots when they have none.
    pub(super) fn empty_bucket(&mut self, a: &Bkey, t: &mut Txn) -> Result<()> {
        for id in [NEED_DISCARD, aids::BUCKET_GENS] {
            if self.root(id).is_err() {
                self.create_root(id, t)?;
            }
        }
        let b = a.pos.offset;
        let mut v = a.value.clone();
        v[16..20].copy_from_slice(&0u32.to_le_bytes());
        let w1 = le64(&v, 8);
        let gen = ((w1 >> 32) as u8).wrapping_add(1);
        let flags = (w1 & 0xff) & !0b10;
        let w1 = u64::from(DATA_NEED_DISCARD) << 48
            | u64::from(gen) << 40
            | u64::from(gen) << 32
            | flags;
        v[8..16].copy_from_slice(&w1.to_le_bytes());
        v[48..56].copy_from_slice(&self.journal_seq.to_le_bytes());
        t.put_uncounted(
            aids::ALLOC,
            Bkey {
                value: v,
                ..a.clone()
            },
        );
        t.put_uncounted(
            NEED_DISCARD,
            Bkey {
                key_type: aids::SET,
                size: 0,
                version_hi: 0,
                version_lo: 0,
                pos: Bpos {
                    inode: self.journal_seq,
                    offset: b,
                    snapshot: 0,
                },
                value: Vec::new(),
            },
        );
        // The bucket_gens key of its group, as this transaction has it so
        // far (an earlier bucket of the same group may be in it already).
        let at = Bpos {
            inode: 0,
            offset: b >> 8,
            snapshot: 0,
        };
        let inflight = t
            .inflight
            .as_ref()
            .filter(|(id, _)| *id == aids::BUCKET_GENS)
            .map(|(_, ks)| ks.as_slice())
            .unwrap_or(&[]);
        // Newest first: what this transaction queued, then what is being
        // written right now.
        let pending = inflight
            .iter()
            .chain(
                t.keys
                    .get(&aids::BUCKET_GENS)
                    .map(|ks| ks.as_slice())
                    .unwrap_or(&[]),
            )
            .rev()
            .find(|k| k.pos == at)
            .cloned();
        let mut k = match pending {
            Some(k) => k,
            None => match self.keys(aids::BUCKET_GENS) {
                Err(Error::NotFound(_)) => None,
                r => r?
                    .into_iter()
                    .find(|k| k.key_type == aids::BUCKET_GENS_KEY && k.pos == at),
            }
            .unwrap_or(Bkey {
                key_type: aids::BUCKET_GENS_KEY,
                size: 0,
                version_hi: 0,
                version_lo: 0,
                pos: at,
                value: vec![0; 256],
            }),
        };
        k.value[(b & 0xff) as usize] = gen;
        t.put_uncounted(aids::BUCKET_GENS, k);
        Ok(())
    }
}

/// The compression accounting key for compression type `ty` (S4, #105:
/// the lz4 fixture holds `0x0405 << 48` = [404, 6585, 6585] for its 404
/// incompressible extents and `0x0403 << 48` for its lz4 ones, as the
/// checker's "compression incompressible" names it): kind 4 in the top
/// byte, the type in the next. Its value is extents, uncompressed sectors,
/// compressed sectors.
pub(super) fn acct_compression(ty: u8) -> Bpos {
    Bpos {
        inode: 4 << 56 | u64::from(ty) << 48,
        offset: 0,
        snapshot: 0,
    }
}

/// One node of a bucket that holds others (#109), added or removed: btree
/// sectors in the replicas and on the device, the part of the device's
/// btree buckets no node uses, and the btree's own node count; the bucket
/// itself stays counted.
pub(super) fn count_btree_node(t: &mut Txn, id: u8, level: u8, sectors: i64, nodes: i64) {
    t.count(acct_replicas(DATA_BTREE, 0), 1, 0, sectors);
    let dev = acct_dev_data_type(0, DATA_BTREE);
    t.count(dev, 3, 1, sectors);
    t.count(dev, 3, 2, -sectors);
    let per_btree = Bpos {
        inode: 6 << 56 | u64::from(id) << 48,
        offset: 0,
        snapshot: 0,
    };
    t.count(per_btree, 3, 0, sectors);
    t.count(per_btree, 3, 1, nodes);
    if level > 0 {
        t.count(per_btree, 3, 2, nodes);
    }
}

/// A btree bucket's share of the accounting (S8): btree sectors in the
/// replicas and on the device, the device's btree buckets and the part of
/// them no node uses, the btree's own node count, and one free bucket fewer
/// for each bucket taken.
pub(super) fn count_btree_bucket(
    t: &mut Txn,
    id: u8,
    level: u8,
    sectors: i64,
    bucket_sectors: i64,
    nodes: i64,
) {
    t.count(acct_replicas(DATA_BTREE, 0), 1, 0, sectors);
    let dev = acct_dev_data_type(0, DATA_BTREE);
    t.count(dev, 3, 0, nodes);
    t.count(dev, 3, 1, sectors);
    t.count(dev, 3, 2, nodes * bucket_sectors - sectors);
    let per_btree = Bpos {
        inode: 6 << 56 | u64::from(id) << 48,
        offset: 0,
        snapshot: 0,
    };
    t.count(per_btree, 3, 0, sectors);
    t.count(per_btree, 3, 1, nodes);
    // The third counts interior nodes (S8: the checker's "btree btree=inodes
    // ... should be 256 4 1" once a root split made one; the aged fixture's
    // two-level btrees carry 1).
    if level > 0 {
        t.count(per_btree, 3, 2, nodes);
    }
    if nodes > 0 {
        t.count(acct_dev_data_type(0, 0), 3, 0, -nodes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounting_positions_are_the_observed_ones() {
        assert_eq!(acct_inum(0x8000_0003).inode, 0x0803_0000_8000_0000);
        assert_eq!(acct_dev_data_type(0, 4).inode, 0x0300_0400_0000_0000);
        assert_eq!(acct_dev_data_type(0, 0).inode, 0x0300_0000_0000_0000);
        assert_eq!(acct_replicas_user(0).inode, 0x0204_0101_0000_0000);
        assert_eq!(acct_replicas(DATA_BTREE, 0).inode, 0x0203_0101_0000_0000);
    }
}
