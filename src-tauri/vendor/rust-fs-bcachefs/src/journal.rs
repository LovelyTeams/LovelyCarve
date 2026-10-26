//! The journal: reading its entries, and replaying the newest of them over
//! the btrees in memory when a filesystem was not shut down cleanly.
//!
//! Provenance (docs/clean-room.md). Documented (S1, 9.7 and 11.2): the
//! journal is a ring of buckets per device holding `jset`s with
//! monotonically increasing sequence numbers; each carries typed
//! sub-entries (btree keys, btree roots on every write, ...); recovery
//! replays, in order, every entry from the newest flush entry's `last_seq`
//! up to that flush entry; entries after the last flush are not replayed;
//! btree data referencing sequence numbers that were never committed is
//! ignored ("blacklisted"). Inferred from hexdumps checked against the
//! reference's `list_journal` (S3, S4, S8):
//!
//! ```text
//! journal_v2 field: (start bucket u64, nr buckets u64) pairs
//! jset:   csum[16], magic u64 @16, seq u64 @24, version u32 @32,
//!         flags u32 @36 (low 4 bits checksum type, bit 5 = not a flush),
//!         u64s u32 @40, 4 bytes, last_seq u64 @48, sub-entries @56
//! entry:  u16 u64s, u8 btree_id, u8 level, u8 type, 3 bytes, u64s*8 bytes
//! magic:  0x245235c1a3625032 XOR the first 8 bytes of the internal UUID
//! csum:   over bytes 16 .. 56 + u64s*8, like a bset's
//! ```

use std::collections::BTreeMap;

use crate::bkey::{self, Bkey, BkeyFormat};
use crate::error::{Error, Result};
use crate::superblock::Superblock;
use crate::util::{le16, le32, le64};
use fs_core::BlockRead;

/// A jset's magic is this constant XOR the first eight bytes of the
/// internal UUID, read little-endian. INFERRED: the XOR is the same for
/// every entry of every fixture.
pub const JSET_MAGIC_BASE: u64 = 0x2452_35c1_a362_5032;
const JSET_HEADER: usize = 56;
const FLAG_NO_FLUSH: u32 = 1 << 5;
/// The superblock field naming the journal's buckets.
pub const FIELD_JOURNAL_V2: u32 = 9;

/// One journal entry (`jset`) as found on the device.
#[derive(Debug, Clone)]
pub struct Jset {
    pub seq: u64,
    /// The oldest entry still needed when this one was written (0 on an
    /// entry that is not a flush).
    pub last_seq: u64,
    pub flush: bool,
    pub version: u32,
    /// Where it starts, in 512-byte sectors from the start of the device.
    pub sector: u64,
    /// Its length in bytes: the header and its sub-entries.
    pub bytes: usize,
    pub entries: Vec<JsetEntry>,
}

/// One sub-entry of a jset.
#[derive(Debug, Clone)]
pub struct JsetEntry {
    pub entry_type: u8,
    pub btree_id: u8,
    pub level: u8,
    /// The keys of a `btree_keys`, `btree_root` or `overwrite` entry.
    pub keys: Vec<Bkey>,
}

/// Sub-entry types, in the order the Principles of Operation lists them
/// (S1, 11.2); the two used here are checked against the reference's
/// listing.
pub mod entry_type {
    pub const BTREE_KEYS: u8 = 0;
    pub const BTREE_ROOT: u8 = 1;
    /// A single blacklisted sequence number, and a range of them (S1 11.2).
    pub const BLACKLIST: u8 = 3;
    pub const BLACKLIST_V2: u8 = 4;
    pub const OVERWRITE: u8 = 10;
}

/// Btree names in the order the Principles of Operation lists them (S1,
/// 11.3). The ids of those the fixtures' journals touch are checked against
/// the reference's listing; the rest follow the documented order.
const BTREE_NAMES: &[&str] = &[
    "extents",
    "inodes",
    "dirents",
    "xattrs",
    "alloc",
    "quotas",
    "stripes",
    "reflink",
    "subvolumes",
    "snapshots",
    "lru",
    "freespace",
    "need_discard",
    "backpointers",
    "bucket_gens",
    "snapshot_trees",
    "deleted_inodes",
    "logged_ops",
    "reconcile_work",
    "subvolume_children",
    "accounting",
    "reconcile_hipri",
    "reconcile_pending",
    "reconcile_scan",
    "reconcile_work_phys",
];

/// A btree's name as the reference tools print it.
pub fn btree_name(id: u8) -> String {
    BTREE_NAMES
        .get(id as usize)
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("btree{id}"))
}

/// The jset magic for a filesystem with this internal UUID.
pub fn jset_magic(uuid: &[u8; 16]) -> u64 {
    JSET_MAGIC_BASE ^ le64(uuid, 0)
}

/// The journal's buckets, as `(first bucket, count)` ranges.
fn bucket_ranges(sb: &Superblock) -> Result<Vec<(u64, u64)>> {
    let f = sb
        .field(FIELD_JOURNAL_V2)
        .ok_or_else(|| Error::Unsupported("no journal_v2 field".into()))?;
    let b = &f.body;
    if b.len() % 16 != 0 {
        return Err(Error::Corrupt(
            "journal_v2 is not whole (start, nr) pairs".into(),
        ));
    }
    Ok(b.as_chunks::<16>()
        .0
        .iter()
        .map(|c| (le64(c, 0), le64(c, 8)))
        .collect())
}

/// Every valid entry in the journal, by sequence number. An entry whose
/// magic or checksum does not hold ends the scan of its bucket; it is not
/// an error, because a ring holds stale and torn writes by design. One
/// whose checksum holds but whose sub-entries do not parse is an error.
pub fn read_entries(dev: &dyn BlockRead, sb: &Superblock) -> Result<Vec<Jset>> {
    scan(dev, sb)?
        .into_iter()
        .map(|(j, damage)| match damage {
            Some(e) => Err(e),
            None => Ok(j),
        })
        .collect()
}

/// [`read_entries`], with the damage of an entry whose sub-entries do not
/// parse beside its header (its `entries` empty) rather than as the error:
/// whether that damage matters depends on where the entry is, which only
/// the whole journal tells (#165).
fn scan(dev: &dyn BlockRead, sb: &Superblock) -> Result<Vec<(Jset, Option<Error>)>> {
    let members = sb.members()?;
    let m = members
        .get(sb.dev_idx as usize)
        .ok_or_else(|| Error::Corrupt("no member entry for this device".into()))?;
    let bucket_bytes = m.bucket_size as u64 * 512;
    if bucket_bytes == 0 {
        return Err(Error::Corrupt("bucket size 0".into()));
    }
    let block = (sb.block_size as usize * 512).max(512);
    let magic = jset_magic(&sb.uuid);
    let mut found: BTreeMap<u64, (Jset, Option<Error>)> = BTreeMap::new();
    for (start, nr) in bucket_ranges(sb)? {
        for bucket in start..start.saturating_add(nr) {
            let base = bucket
                .checked_mul(bucket_bytes)
                .ok_or_else(|| Error::Corrupt("journal bucket offset overflows".into()))?;
            let mut buf = vec![0u8; bucket_bytes as usize];
            dev.read_at(base, &mut buf)?;
            let mut at = 0usize;
            while at + JSET_HEADER <= buf.len() {
                let Some((j, damage)) = parse_jset_checked(&buf[at..], magic)? else {
                    break;
                };
                let len = j.bytes.div_ceil(block) * block;
                let j = Jset {
                    sector: (base + at as u64) / 512,
                    ..j
                };
                found.entry(j.seq).or_insert((j, damage));
                at += len;
            }
        }
    }
    Ok(found.into_values().collect())
}

/// Parse one jset at the start of `b`; `None` when it is not one (wrong
/// magic or a checksum that does not hold). A matching header whose length
/// exceeds the available bucket bytes is corruption, even outside replay.
pub fn parse_jset(b: &[u8], magic: u64) -> Result<Option<Jset>> {
    match parse_jset_checked(b, magic)? {
        None => Ok(None),
        Some((_, Some(e))) => Err(e),
        Some((j, None)) => Ok(Some(j)),
    }
}

/// [`parse_jset`], with damage in the sub-entries of a jset whose checksum
/// holds returned beside its header, whose `entries` are then empty.
fn parse_jset_checked(b: &[u8], magic: u64) -> Result<Option<(Jset, Option<Error>)>> {
    if b.len() < JSET_HEADER || le64(b, 16) != magic {
        return Ok(None);
    }
    let flags = le32(b, 36);
    let end = u64::from(le32(b, 40)) * 8 + JSET_HEADER as u64;
    if end > b.len() as u64 {
        // #262's black-box controls: the reference reports this before
        // checksum validation, including in stale entries outside replay.
        return Err(Error::Corrupt(format!(
            "jset_past_bucket_end: journal entry seq {} too big ({end} bytes, {} available)",
            le64(b, 24),
            b.len()
        )));
    }
    let end = end as usize;
    let csum_type = (flags & 0xf) as u8;
    if !crate::csum::is_known(csum_type) {
        return Err(Error::Unsupported(format!(
            "journal checksum type {csum_type}"
        )));
    }
    let stored = le64(b, 0)
        & if matches!(csum_type, 1 | 5) {
            0xffff_ffff
        } else {
            u64::MAX
        };
    if crate::csum::verify(csum_type, &b[16..end], stored).is_err() {
        return Ok(None);
    }
    let (entries, damage) = match parse_entries(b, end) {
        Ok(entries) => (entries, None),
        Err(e) => (Vec::new(), Some(e)),
    };
    Ok(Some((
        Jset {
            seq: le64(b, 24),
            last_seq: le64(b, 48),
            flush: flags & FLAG_NO_FLUSH == 0,
            version: le32(b, 32),
            sector: 0,
            bytes: end,
            entries,
        },
        damage,
    )))
}

/// The sub-entries of the jset at the start of `b`, which ends at `end`.
fn parse_entries(b: &[u8], end: usize) -> Result<Vec<JsetEntry>> {
    let mut entries = Vec::new();
    let mut p = JSET_HEADER;
    while p + 8 <= end {
        let n = le16(b, p) as usize;
        let (btree_id, level, entry_type) = (b[p + 2], b[p + 3], b[p + 4]);
        let body_end = p + 8 + n * 8;
        if body_end > end {
            return Err(Error::Corrupt(
                "journal sub-entry runs past its jset".into(),
            ));
        }
        let keys = match entry_type {
            entry_type::BTREE_KEYS | entry_type::BTREE_ROOT | entry_type::OVERWRITE => {
                decode_keys(&b[p + 8..body_end])?
            }
            _ => Vec::new(),
        };
        entries.push(JsetEntry {
            entry_type,
            btree_id,
            level,
            keys,
        });
        p = body_end;
    }
    Ok(entries)
}

/// The unpacked keys of a sub-entry body.
fn decode_keys(b: &[u8]) -> Result<Vec<Bkey>> {
    let unpacked = BkeyFormat {
        key_u64s: 5,
        nr_fields: 6,
        bits: [0; 6],
        field_offset: [0; 6],
    };
    let mut out = Vec::new();
    let mut p = 0;
    while p < b.len() {
        let n = b[p] as usize;
        if n == 0 {
            break; // padding at the end of an entry
        }
        let end = p + n * 8;
        if end > b.len() {
            return Err(Error::Corrupt("journal key runs past its entry".into()));
        }
        out.push(bkey::decode(&b[p..end], &unpacked)?);
        p = end;
    }
    Ok(out)
}

#[cfg(test)]
mod length_tests {
    use super::*;

    #[test]
    fn an_impossible_journal_length_is_not_a_missing_entry() {
        let magic = 0x1234u64;
        let mut b = vec![0; JSET_HEADER];
        b[16..24].copy_from_slice(&magic.to_le_bytes());
        b[24..32].copy_from_slice(&206u64.to_le_bytes());
        b[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        let e = parse_jset(&b, magic).unwrap_err().to_string();
        assert!(e.contains("jset_past_bucket_end"), "{e}");
        assert!(e.contains("34359738416"), "{e}");
        b[16..24].fill(0);
        assert!(parse_jset(&b, magic).unwrap().is_none());
    }
}

/// What a replay of the journal adds to the btrees on disk.
#[derive(Debug, Clone, Default)]
pub struct Replay {
    /// The newest flush entry: btree data from later sequence numbers was
    /// never committed and is ignored.
    pub seq: u64,
    /// The btree roots that entry recorded: `(btree id, level, root key)`.
    pub roots: Vec<(u8, u8, Bkey)>,
    /// Per `(btree id, level)`, the keys of the replayed entries in the
    /// order they apply; a later key at the same position replaces an
    /// earlier one. Level 0 holds leaf keys; higher levels hold the
    /// pointers of interior nodes that were split or rewritten.
    pub keys: BTreeMap<(u8, u8), Vec<Bkey>>,
}

/// Collect what replaying the journal applies: the entries from the newest
/// flush entry's `last_seq` to that entry, in order.
pub fn replay(dev: &dyn BlockRead, sb: &Superblock) -> Result<Replay> {
    let scanned = scan(dev, sb)?;
    let newest = scanned
        .iter()
        .map(|(j, _)| j)
        .rev()
        .find(|j| j.flush)
        .ok_or_else(|| Error::Corrupt("the journal holds no flush entry".into()))?;
    let (from, to) = (newest.last_seq, newest.seq);
    // Damage inside the window is damage to what recovery applies, and is
    // refused. An entry older than the window is a stale write the ring has
    // not overwritten yet, and one newer than its last flush was never
    // committed: recovery reads neither, and the reference replays such a
    // journal without looking inside them (#165).
    let mut entries = Vec::with_capacity(scanned.len());
    for (j, damage) in scanned {
        if let Some(e) = damage {
            if (from..=to).contains(&j.seq) {
                return Err(e);
            }
        }
        entries.push(j);
    }
    // Every entry in the window must be there: a gap means lost updates.
    for seq in from..=to {
        if !entries.iter().any(|j| j.seq == seq) {
            return Err(Error::Corrupt(format!(
                "journal entry {seq} is missing from the replay window {from}..={to}"
            )));
        }
    }
    let mut out = Replay {
        seq: to,
        ..Replay::default()
    };
    for j in entries.iter().filter(|j| j.seq >= from && j.seq <= to) {
        for e in &j.entries {
            match e.entry_type {
                // A blacklist carried by the journal applies to the bsets
                // this replay reads, and its payload has never been seen
                // (no fixture's journal holds one): refused, not guessed.
                entry_type::BLACKLIST | entry_type::BLACKLIST_V2 => {
                    return Err(Error::Unsupported(format!(
                        "journal entry {} carries a blacklist entry (type {}), whose layout \
                         is not known to this reader",
                        j.seq, e.entry_type
                    )));
                }
                entry_type::BTREE_KEYS => {
                    out.keys
                        .entry((e.btree_id, e.level))
                        .or_default()
                        .extend(e.keys.iter().cloned());
                }
                entry_type::BTREE_ROOT if j.seq == to => {
                    if let Some(k) = e.keys.first() {
                        out.roots.push((e.btree_id, e.level, k.clone()));
                    }
                }
                _ => {}
            }
        }
    }
    if out.roots.is_empty() {
        return Err(Error::Corrupt(format!(
            "journal entry {to} records no btree roots"
        )));
    }
    Ok(out)
}
