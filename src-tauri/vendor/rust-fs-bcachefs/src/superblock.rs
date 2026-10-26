//! The superblock: identity, geometry, versions, options, and the typed
//! variable-length fields that follow it.
//!
//! Provenance (docs/clean-room.md): the location (4 KiB; layout copy at
//! 3584 bytes), the list of contents and the field-type names come from the
//! Principles of Operation (S1). Every byte offset below was found by
//! hexdump of reference-formatted images and matched against the reference
//! superblock printer's report of the same image (S3, S4).

use crate::error::{Error, Result};
use crate::util::{le16, le32, le64, uuid_at};
use fs_core::BlockRead;

/// Byte offset of the primary superblock (sector 8).
pub const SB_OFFSET: u64 = 4096;
/// Byte offset of the standalone layout copy (sector 7).
pub const LAYOUT_OFFSET: u64 = 3584;
/// Bytes of the fixed header before the variable-length fields.
pub const SB_HEADER_BYTES: usize = 0x2f0;
/// The magic the reference printer reports as "Magic number", as it is
/// stored on disk.
pub const BCACHEFS_MAGIC: [u8; 16] = [
    0xc6, 0x85, 0x73, 0xf6, 0x66, 0xce, 0x90, 0xa9, 0xd9, 0x6a, 0x60, 0xcf, 0x80, 0x3d, 0xf7, 0xef,
];
/// Upper bound on what we will read for one superblock: the layout's
/// `sb_max_size_bits` is a power of two in sectors, and the reference
/// formatter uses 2^11 sectors (1 MiB).
const SB_MAX_BYTES: usize = 1 << 20;

/// Field types, in the order the Principles of Operation lists them; the
/// numbering is confirmed by the reference printer's "Sections" line
/// against the type numbers in the image.
pub const FIELD_NAMES: &[&str] = &[
    "journal",
    "members_v1",
    "crypt",
    "replicas_v0",
    "quota",
    "disk_groups",
    "clean",
    "replicas",
    "journal_seq_blacklist",
    "journal_v2",
    "counters",
    "members_v2",
    "errors",
    "ext",
    "downgrade",
    "recovery_passes",
    "extent_type_u64s",
    "errors_v2",
];

pub const FIELD_CRYPT: u32 = 2;
pub const FIELD_CLEAN: u32 = 6;
pub const FIELD_JOURNAL_SEQ_BLACKLIST: u32 = 8;
pub const FIELD_MEMBERS_V2: u32 = 11;

/// Names of the 1.x metadata versions, from the version history in the
/// Principles of Operation (S1, section 11.6). Index = minor.
pub const VERSION_NAMES_1X: &[&str] = &[
    "major_minor",
    "snapshot_skiplists",
    "deleted_inodes",
    "rebalance_work",
    "member_seq",
    "subvolume_fs_parent",
    "btree_subvolume_children",
    "mi_btree_bitmap",
    "bucket_stripe_sectors",
    "disk_accounting_v2",
    "disk_accounting_v3",
    "disk_accounting_inum",
    "rebalance_work_acct_fix",
    "inode_has_child_snapshots",
    "backpointer_bucket_gen",
    "disk_accounting_big_endian",
    "reflink_p_may_update_opts",
    "inode_depth",
    "persistent_inode_cursors",
    "autofix_errors",
    "directory_size",
    "cached_backpointers",
    "stripe_backpointers",
    "stripe_lru",
    "casefolding",
    "extent_flags",
    "snapshot_deletion_v2",
    "fast_device_removal",
    "inode_has_case_insensitive",
    "extent_snapshot_whiteouts",
    "31bit_dirent_offset",
    "btree_node_accounting",
    "sb_field_extent_type_u64s",
    "reconcile",
    "extented_key_type_error",
    "bucket_stripe_index",
    "no_sb_user_data_replicas",
    "erasure_coding",
    "need_discard_by_journal_seq",
    "per_dev_fragmentation_lru",
];

/// Incompatible-feature bit names. INFERRED: the reference printer lists
/// the names of the set bits in ascending bit order, and these are the
/// bits set in every fixture paired with the names it printed. A bit not
/// in this table is reported as `bit<N>`.
pub const FEATURE_NAMES: &[(u32, &str)] = &[
    (0, "lz4"),
    (1, "gzip"),
    (2, "zstd"),
    (5, "journal_seq_blacklist_v3"),
    (7, "new_siphash"),
    (8, "inline_data"),
    (9, "new_extent_overwrite"),
    (11, "btree_ptr_v2"),
    (12, "extents_above_btree_updates"),
    (13, "btree_updates_journalled"),
    (15, "new_varint"),
    (16, "journal_no_flush"),
    (17, "alloc_v2"),
    (18, "extents_across_btree_nodes"),
    (19, "incompat_version_field"),
];

/// Compatible-feature bit names, inferred the same way.
pub const COMPAT_NAMES: &[(u32, &str)] = &[
    (0, "alloc_info"),
    (1, "alloc_metadata"),
    (2, "extents_above_btree_updates_done"),
    (3, "bformat_overflow_done"),
    (4, "no_stale_ptrs"),
    (5, "stripe_frag_accounting"),
    (6, "inode_opts_propagated"),
];

/// A metadata version: `major << 10 | minor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u16);

impl Version {
    pub fn major(self) -> u16 {
        self.0 >> 10
    }
    pub fn minor(self) -> u16 {
        self.0 & 0x3ff
    }
    /// The name the version history gives it, for 1.x.
    pub fn name(self) -> Option<&'static str> {
        if self.major() == 1 {
            VERSION_NAMES_1X.get(self.minor() as usize).copied()
        } else {
            None
        }
    }
}

/// Where the superblock copies are (the `bch_sb_layout`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub layout_type: u8,
    pub sb_max_size_bits: u8,
    /// Sector offsets of every superblock copy.
    pub sb_offsets: Vec<u64>,
}

impl Layout {
    pub const BYTES: usize = 16 + 8 + 61 * 8;

    pub fn parse(b: &[u8]) -> Result<Self> {
        if b.len() < Self::BYTES {
            return Err(Error::Corrupt("layout shorter than its fixed size".into()));
        }
        if b[..16] != BCACHEFS_MAGIC {
            return Err(Error::BadMagic {
                what: "superblock layout",
            });
        }
        let nr = b[18] as usize;
        if nr > 61 {
            return Err(Error::Corrupt(format!(
                "layout claims {nr} superblocks (max 61)"
            )));
        }
        let sb_offsets = (0..nr).map(|i| le64(b, 24 + i * 8)).collect();
        Ok(Layout {
            layout_type: b[16],
            sb_max_size_bits: b[17],
            sb_offsets,
        })
    }
}

/// One variable-length field: its type and its body (after the 8-byte
/// `u64s`/`type` header).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub field_type: u32,
    pub body: Vec<u8>,
}

impl Field {
    pub fn name(&self) -> String {
        FIELD_NAMES
            .get(self.field_type as usize)
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("field{}", self.field_type))
    }
}

/// One member device, from `members_v2`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub uuid: [u8; 16],
    pub nbuckets: u64,
    pub first_bucket: u16,
    /// Bucket size in 512-byte sectors.
    pub bucket_size: u16,
    /// Seconds since the Unix epoch.
    pub last_mount: u64,
}

/// A btree root recorded in the `clean` field: the btree id, its level, and
/// the raw `bkey_i` of the root's pointer (decoded by `crate::bkey`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootEntry {
    pub btree_id: u8,
    pub level: u8,
    pub key: Vec<u8>,
}

/// A parsed, checksum-verified superblock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Superblock {
    pub csum: [u8; 16],
    pub version: Version,
    pub version_min: Version,
    pub magic: [u8; 16],
    /// The internal (immutable) UUID.
    pub uuid: [u8; 16],
    /// The external, user-visible UUID.
    pub user_uuid: [u8; 16],
    pub label: [u8; 32],
    /// Sector this copy was read from.
    pub offset: u64,
    pub seq: u64,
    /// Block size in 512-byte sectors.
    pub block_size: u16,
    pub dev_idx: u8,
    pub nr_devices: u8,
    pub u64s: u32,
    pub time_base_lo: u64,
    pub time_base_hi: u32,
    pub time_precision: u32,
    pub flags: [u64; 8],
    pub features: [u64; 2],
    pub compat: [u64; 2],
    pub layout: Layout,
    pub fields: Vec<Field>,
}

impl Superblock {
    /// Parse a superblock from `b`, which starts at the superblock and holds
    /// at least its header and fields. Verifies magic and checksum.
    pub fn parse(b: &[u8]) -> Result<Self> {
        let mut sb = Self::parse_header(b)?;
        let end = SB_HEADER_BYTES + sb.u64s as usize * 8;
        sb.verify_checksum(&b[..end])?;
        sb.fields = parse_fields(&b[SB_HEADER_BYTES..end])?;
        Ok(sb)
    }

    /// [`Superblock::parse`] without verifying the checksum: for a writer
    /// that has changed the bytes and has not checksummed them yet.
    pub fn parse_unchecked(b: &[u8]) -> Result<Self> {
        let mut sb = Self::parse_header(b)?;
        let end = SB_HEADER_BYTES + sb.u64s as usize * 8;
        sb.fields = parse_fields(&b[SB_HEADER_BYTES..end])?;
        Ok(sb)
    }

    /// Header and bounds only: copy selection precedes optional-field
    /// validation (S3, #261). Checksum validation still precedes selection.
    fn parse_header(b: &[u8]) -> Result<Self> {
        if b.len() < SB_HEADER_BYTES {
            return Err(Error::Corrupt("superblock shorter than its header".into()));
        }
        let mut magic = [0u8; 16];
        magic.copy_from_slice(&b[0x18..0x28]);
        if magic != BCACHEFS_MAGIC {
            return Err(Error::BadMagic { what: "superblock" });
        }
        let u64s = le32(b, 0x7c);
        (u64s as usize)
            .checked_mul(8)
            .and_then(|n| n.checked_add(SB_HEADER_BYTES))
            .filter(|&e| e <= b.len().min(SB_MAX_BYTES))
            .ok_or_else(|| {
                Error::Corrupt(format!("superblock u64s {u64s} runs past the buffer"))
            })?;

        let mut csum = [0u8; 16];
        csum.copy_from_slice(&b[0..16]);
        let mut label = [0u8; 32];
        label.copy_from_slice(&b[0x48..0x68]);
        let mut flags = [0u64; 8];
        for (i, f) in flags.iter_mut().enumerate() {
            *f = le64(b, 0x90 + i * 8);
        }
        let sb = Superblock {
            csum,
            version: Version(le16(b, 0x10)),
            version_min: Version(le16(b, 0x12)),
            magic,
            uuid: uuid_at(b, 0x28),
            user_uuid: uuid_at(b, 0x38),
            label,
            offset: le64(b, 0x68),
            seq: le64(b, 0x70),
            block_size: le16(b, 0x78),
            dev_idx: b[0x7a],
            nr_devices: b[0x7b],
            u64s,
            time_base_lo: le64(b, 0x80),
            time_base_hi: le32(b, 0x88),
            time_precision: le32(b, 0x8c),
            flags,
            features: [le64(b, 0xd0), le64(b, 0xd8)],
            compat: [le64(b, 0xe0), le64(b, 0xe8)],
            layout: Layout::parse(&b[0xf0..SB_HEADER_BYTES])?,
            fields: Vec::new(),
        };
        Ok(sb)
    }

    /// Read the superblock from a device: every copy the layout names,
    /// the one with the highest valid `seq` winning, and the later copy
    /// winning a tie.
    ///
    /// The Principles of Operation (S1, 9.5.1): each write of the
    /// superblock carries a `seq` one above the last, and of the copies a
    /// reader finds valid, the largest `seq` is the one to believe; the
    /// standalone layout at sector 7 is a fallback, read only if the
    /// primary superblock is unreadable. So: the primary first, its embedded
    /// layout naming the copies; failing that, the layout at 3584; then
    /// every copy's header and checksum. Optional fields are parsed only
    /// after selecting the winner (S3, #261); an invalid selected field is
    /// an error, not a reason to use an older copy. Observed (S4): every
    /// fixture carries three copies, at sectors 8 and 2056 and at the end
    /// of the device, all with the same `seq`.
    pub fn read(dev: &dyn BlockRead) -> Result<Self> {
        Self::read_copies(dev).map(|(sb, _)| sb)
    }

    /// [`Superblock::read`], also returning copies whose header or checksum
    /// could not be read, by sector, for the checker. Invalid optional
    /// fields in the selected copy are an error.
    pub fn read_copies(dev: &dyn BlockRead) -> Result<(Self, Vec<(u64, Error)>)> {
        let mut failed: Vec<(u64, Error)> = Vec::new();
        let primary = Self::read_header_at(dev, SB_OFFSET / 512);
        let layout = match &primary {
            Ok(sb) => sb.layout.clone(),
            Err(e) => {
                failed.push((SB_OFFSET / 512, e.clone()));
                match Self::read_layout(dev) {
                    Ok(l) => l,
                    // Neither the primary nor the layout: the primary's
                    // error is the one that says what is there.
                    Err(_) => return Err(e.clone()),
                }
            }
        };
        let mut best = primary.ok().map(|sb| (SB_OFFSET / 512, sb));
        for &sector in &layout.sb_offsets {
            if sector == SB_OFFSET / 512 {
                continue;
            }
            match Self::read_header_at(dev, sector) {
                Ok(sb) => {
                    if best.as_ref().is_none_or(|(_, b)| sb.seq >= b.seq) {
                        best = Some((sector, sb));
                    }
                }
                Err(e) => failed.push((sector, e)),
            }
        }
        match best {
            Some((sector, _)) => Self::read_at(dev, sector).map(|sb| (sb, failed)),
            None => Err(failed
                .into_iter()
                .next()
                .map(|(_, e)| e)
                .unwrap_or(Error::BadMagic { what: "superblock" })),
        }
    }

    /// The standalone layout copy at sector 7.
    pub fn read_layout(dev: &dyn BlockRead) -> Result<Layout> {
        let mut b = vec![0u8; Layout::BYTES];
        dev.read_at(LAYOUT_OFFSET, &mut b)?;
        Layout::parse(&b)
    }

    /// One superblock copy, at `sector`, parsed and checksummed.
    pub fn read_at(dev: &dyn BlockRead, sector: u64) -> Result<Self> {
        Self::parse(&Self::read_bytes_at(dev, sector)?)
    }

    fn read_header_at(dev: &dyn BlockRead, sector: u64) -> Result<Self> {
        let bytes = Self::read_bytes_at(dev, sector)?;
        let sb = Self::parse_header(&bytes)?;
        sb.verify_checksum(&bytes)?;
        Ok(sb)
    }

    fn read_bytes_at(dev: &dyn BlockRead, sector: u64) -> Result<Vec<u8>> {
        let at = sector
            .checked_mul(512)
            .ok_or_else(|| Error::Corrupt(format!("superblock sector {sector} overflows")))?;
        let mut head = vec![0u8; SB_HEADER_BYTES];
        dev.read_at(at, &mut head)?;
        if head[0x18..0x28] != BCACHEFS_MAGIC {
            return Err(Error::BadMagic { what: "superblock" });
        }
        let total = SB_HEADER_BYTES + le32(&head, 0x7c) as usize * 8;
        if total > SB_MAX_BYTES {
            return Err(Error::Corrupt(format!("superblock claims {total} bytes")));
        }
        let mut b = vec![0u8; total];
        dev.read_at(at, &mut b)?;
        Ok(b)
    }

    /// Whether the filesystem is encrypted: it carries a `crypt` field
    /// (type 2). INFERRED: the reference formatter's `--encrypted` adds
    /// exactly that section, and no other fixture has it (S3).
    pub fn is_encrypted(&self) -> bool {
        self.field(FIELD_CRYPT).is_some()
    }

    /// The crypt field of an encrypted filesystem ([`crate::crypt`]);
    /// `None` for a filesystem that is not encrypted.
    pub fn crypt(&self) -> Result<Option<crate::crypt::CryptField>> {
        self.field(FIELD_CRYPT)
            .map(|f| crate::crypt::CryptField::parse(&f.body))
            .transpose()
    }

    /// Whether the filesystem was shut down cleanly. INFERRED: flags[0]
    /// bit 1, set wherever the reference printer says `Clean: 1` (every
    /// formatter-made fixture and the replayed aged image) and clear on the
    /// one image a mount left without a clean shutdown (aged-unclean); no
    /// other bit of flags[0] differs between those two.
    pub fn is_clean(&self) -> bool {
        self.flags[0] & 0b10 != 0
    }

    /// Checksum type of the superblock itself. INFERRED: flags[0] bits
    /// 2..5; 0 for a filesystem formatted with metadata checksums off, 1
    /// for crc32c, 2 for crc64, 7 for xxhash (fixtures nocsum, default,
    /// crc64, xxhash).
    pub fn csum_type(&self) -> u8 {
        ((self.flags[0] >> 2) & 0xf) as u8
    }

    /// Btree node size in sectors. INFERRED: flags[0] bits 12..27, equal
    /// to the reference printer's btree_node_size in every fixture.
    pub fn btree_node_size(&self) -> u32 {
        ((self.flags[0] >> 12) & 0xffff) as u32
    }

    /// Option values for metadata and data checksums (none=0, crc32c=1,
    /// crc64=2, xxhash=3), INFERRED from flags[0] bits 40..43 and 44..47.
    pub fn metadata_checksum_opt(&self) -> u8 {
        ((self.flags[0] >> 40) & 0xf) as u8
    }
    pub fn data_checksum_opt(&self) -> u8 {
        ((self.flags[0] >> 44) & 0xf) as u8
    }

    /// Compression option (none=0, lz4=1, gzip=2, zstd=3), INFERRED from
    /// flags[1] bits 4..7.
    pub fn compression_opt(&self) -> u8 {
        ((self.flags[1] >> 4) & 0xf) as u8
    }

    fn verify_checksum(&self, b: &[u8]) -> Result<()> {
        let stored = le64(&self.csum, 0);
        let t = self.csum_type();
        if !crate::csum::is_known(t) {
            return Err(Error::Unsupported(format!("superblock checksum type {t}")));
        }
        crate::csum::verify(t, &b[16..], stored).map_err(|computed| Error::BadChecksum {
            what: "superblock",
            stored,
            computed,
        })
    }

    pub fn field(&self, field_type: u32) -> Option<&Field> {
        self.fields.iter().find(|f| f.field_type == field_type)
    }

    /// Names of the variable-length fields, ordered by type number (the
    /// order the reference printer lists them in).
    pub fn field_names(&self) -> Vec<String> {
        let mut f: Vec<&Field> = self.fields.iter().collect();
        f.sort_by_key(|f| f.field_type);
        f.into_iter().map(Field::name).collect()
    }

    pub fn feature_names(&self) -> Vec<String> {
        bit_names(self.features[0], FEATURE_NAMES)
    }

    pub fn compat_names(&self) -> Vec<String> {
        bit_names(self.compat[0], COMPAT_NAMES)
    }

    /// The member devices from `members_v2`. Layout INFERRED from hexdump:
    /// the body starts with a u16 per-member size and 6 bytes of padding;
    /// each member is uuid(16), nbuckets u64, first_bucket u16,
    /// bucket_size u16, 4 bytes we do not interpret, last_mount u64.
    pub fn members(&self) -> Result<Vec<Member>> {
        let f = self
            .field(FIELD_MEMBERS_V2)
            .ok_or_else(|| Error::NotFound("members_v2 field".into()))?;
        let b = &f.body;
        if b.len() < 8 {
            return Err(Error::Corrupt("members_v2 too short".into()));
        }
        let member_bytes = le16(b, 0) as usize;
        if member_bytes < 48 {
            return Err(Error::Corrupt(format!(
                "members_v2 member size {member_bytes}"
            )));
        }
        let n = self.nr_devices as usize;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let off = 8 + i * member_bytes;
            if off + 48 > b.len() {
                return Err(Error::Corrupt("members_v2 shorter than nr_devices".into()));
            }
            out.push(Member {
                uuid: uuid_at(b, off),
                nbuckets: le64(b, off + 16),
                first_bucket: le16(b, off + 24),
                bucket_size: le16(b, off + 26),
                last_mount: le64(b, off + 32),
            });
        }
        Ok(out)
    }

    /// The blacklisted journal sequence ranges, `(start, end)` with the end
    /// EXCLUSIVE: a bset whose journal sequence is in `start <= seq < end`
    /// belongs to a journal entry that was never committed and is ignored
    /// (S1 1.3, 9.7.5). Layout INFERRED: pairs of u64 (S4: the aged image's
    /// field reads 307..4403 after the reference replayed entries 304-306
    /// and blacklisted 64 past the last entry read). The end is exclusive
    /// because two live bsets of that image carry journal sequence 4403
    /// exactly and the reference lister reads their keys (S3, S4).
    pub fn journal_seq_blacklist(&self) -> Result<Vec<(u64, u64)>> {
        let Some(f) = self.field(FIELD_JOURNAL_SEQ_BLACKLIST) else {
            return Ok(Vec::new());
        };
        if !f.body.len().is_multiple_of(16) {
            return Err(Error::Corrupt(
                "journal_seq_blacklist is not whole (start, end) pairs".into(),
            ));
        }
        let mut out: Vec<(u64, u64)> = f
            .body
            .as_chunks::<16>()
            .0
            .iter()
            .map(|c| (le64(c, 0), le64(c, 8)))
            .collect();
        if out.iter().any(|&(s, e)| s > e) {
            return Err(Error::Corrupt(
                "journal_seq_blacklist range ends before it starts".into(),
            ));
        }
        out.sort_unstable();
        Ok(out)
    }

    /// The btree roots recorded in the `clean` field. Layout INFERRED: the
    /// body is flags u32, two u16 clocks, journal_seq u64, then journal
    /// entries; each entry is u16 u64s, u8 btree_id, u8 level, u8 type,
    /// 3 bytes padding, then u64s*8 bytes. Type 1 is `btree_root`, its
    /// position in the Principles of Operation's list of journal entry
    /// types, and its payload is the root's key.
    pub fn btree_roots(&self) -> Result<Vec<RootEntry>> {
        let f = self
            .field(FIELD_CLEAN)
            .ok_or_else(|| Error::Unsupported("no clean field: the filesystem was not shut down cleanly; its roots are in the journal (crate::journal::replay)".into()))?;
        let b = &f.body;
        let mut out = Vec::new();
        let mut p = 16;
        while p + 8 <= b.len() {
            let u64s = le16(b, p) as usize;
            let (btree_id, level, ty) = (b[p + 2], b[p + 3], b[p + 4]);
            let end = p + 8 + u64s * 8;
            if end > b.len() {
                return Err(Error::Corrupt("clean field entry runs past its end".into()));
            }
            if ty == 1 && u64s > 0 {
                out.push(RootEntry {
                    btree_id,
                    level,
                    key: b[p + 8..end].to_vec(),
                });
            }
            p = end;
        }
        Ok(out)
    }

    /// The label as a string, NUL padding removed.
    pub fn label_str(&self) -> String {
        let n = self.label.iter().position(|&c| c == 0).unwrap_or(32);
        String::from_utf8_lossy(&self.label[..n]).into_owned()
    }
}

fn bit_names(v: u64, table: &[(u32, &str)]) -> Vec<String> {
    (0..64)
        .filter(|b| v & (1u64 << b) != 0)
        .map(|b| {
            table
                .iter()
                .find(|(bit, _)| *bit == b)
                .map(|(_, n)| n.to_string())
                .unwrap_or_else(|| format!("bit{b}"))
        })
        .collect()
}

fn parse_fields(b: &[u8]) -> Result<Vec<Field>> {
    let mut out = Vec::new();
    let mut p = 0;
    while p + 8 <= b.len() {
        let u64s = le32(b, p) as usize;
        let field_type = le32(b, p + 4);
        if u64s == 0 {
            return Err(Error::Corrupt(format!(
                "superblock field at {p} has zero length"
            )));
        }
        let end = u64s
            .checked_mul(8)
            .and_then(|n| n.checked_add(p))
            .filter(|&e| e <= b.len())
            .ok_or_else(|| {
                Error::Corrupt(format!("superblock field at {p} runs past the superblock"))
            })?;
        out.push(Field {
            field_type,
            body: b[p + 8..end].to_vec(),
        });
        p = end;
    }
    Ok(out)
}

/// Format a 16-byte UUID the usual way.
pub fn format_uuid(u: &[u8; 16]) -> String {
    let h: Vec<String> = u.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        h[0..4].concat(),
        h[4..6].concat(),
        h[6..8].concat(),
        h[8..10].concat(),
        h[10..16].concat()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal superblock built from the facts above, checksummed.
    pub(crate) fn synthetic() -> Vec<u8> {
        let fields_u64s = 2u32; // one empty-bodied field of 16 bytes
        let mut b = vec![0u8; SB_HEADER_BYTES + fields_u64s as usize * 8];
        b[0x10..0x12].copy_from_slice(&((1u16 << 10) | 39).to_le_bytes());
        b[0x12..0x14].copy_from_slice(&((1u16 << 10) | 36).to_le_bytes());
        b[0x18..0x28].copy_from_slice(&BCACHEFS_MAGIC);
        b[0x28] = 0xaa;
        b[0x38] = 0xbb;
        b[0x48..0x4d].copy_from_slice(b"label");
        b[0x68] = 8;
        b[0x70] = 21;
        b[0x78] = 1;
        b[0x7b] = 1;
        b[0x7c..0x80].copy_from_slice(&fields_u64s.to_le_bytes());
        b[0x90..0x98].copy_from_slice(&(0x0004_0107u64).to_le_bytes());
        b[0xd0] = 0x80;
        b[0xe0] = 0x7f;
        b[0xf0..0x100].copy_from_slice(&BCACHEFS_MAGIC);
        b[0x101] = 11;
        b[0x102] = 1;
        b[0x108] = 8;
        let f = SB_HEADER_BYTES;
        b[f..f + 4].copy_from_slice(&2u32.to_le_bytes());
        b[f + 4..f + 8].copy_from_slice(&13u32.to_le_bytes());
        let c = crate::csum::crc32c_nonzero(&b[16..]);
        b[0..4].copy_from_slice(&c.to_le_bytes());
        b
    }

    #[test]
    fn a_synthetic_superblock_parses() {
        let sb = Superblock::parse(&synthetic()).unwrap();
        assert_eq!(sb.version, Version(1063));
        assert_eq!(sb.version.name(), Some("per_dev_fragmentation_lru"));
        assert_eq!(sb.version_min.name(), Some("no_sb_user_data_replicas"));
        assert_eq!(sb.seq, 21);
        assert_eq!(sb.block_size, 1);
        assert_eq!(sb.btree_node_size(), 64);
        assert_eq!(sb.csum_type(), 1);
        assert_eq!(sb.label_str(), "label");
        assert_eq!(sb.layout.sb_offsets, vec![8]);
        assert_eq!(sb.field_names(), vec!["ext"]);
        assert_eq!(sb.feature_names(), vec!["new_siphash"]);
        assert_eq!(sb.compat_names().len(), 7);
    }

    #[test]
    fn a_flipped_byte_fails_the_checksum() {
        let mut b = synthetic();
        b[0x70] ^= 1;
        assert!(matches!(
            Superblock::parse(&b),
            Err(Error::BadChecksum { .. })
        ));
    }

    #[test]
    fn a_wrong_magic_is_refused() {
        let mut b = synthetic();
        b[0x18] ^= 1;
        assert_eq!(
            Superblock::parse(&b),
            Err(Error::BadMagic { what: "superblock" })
        );
    }

    #[test]
    fn fields_running_past_the_end_are_corrupt() {
        let mut b = synthetic();
        b[0x7c] = 200;
        assert!(matches!(Superblock::parse(&b), Err(Error::Corrupt(_))));
        let mut b = synthetic();
        let f = SB_HEADER_BYTES;
        b[f] = 0;
        let c = crate::csum::crc32c_nonzero(&b[16..]);
        b[0..4].copy_from_slice(&c.to_le_bytes());
        assert!(matches!(Superblock::parse(&b), Err(Error::Corrupt(_))));
    }

    #[test]
    fn short_input_never_panics() {
        let b = synthetic();
        for n in 0..b.len() {
            let _ = Superblock::parse(&b[..n]);
        }
    }

    #[test]
    fn uuids_format_the_usual_way() {
        let mut u = [0u8; 16];
        u[0] = 0x61;
        u[15] = 0xaf;
        assert_eq!(format_uuid(&u), "61000000-0000-0000-0000-0000000000af");
    }
}
