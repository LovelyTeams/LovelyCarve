//! Inodes (`inode_v3`) and directory entries.
//!
//! Provenance (docs/clean-room.md): that inodes live in the inodes btree,
//! that v3 is the current "compact encoding", and the names and order of
//! the inode's fields come from the documentation and the reference
//! lister's printout of each inode (S1, S3). The encoding itself was found
//! by hexdump against those printed values (S4):
//!
//! * the inodes btree keys an inode by its number in the OFFSET field of
//!   the position (the lister prints `0:4096:U32_MAX` for inode 4096);
//! * the value starts with journal_seq u64, hash_seed u64, and a u64 whose
//!   bits 0..31 are flags (bits 24..31 hold the number of varint fields
//!   that follow, bits 20..23 the string hash type) and bits 36..51 the
//!   mode; then bi_sectors, bi_size and bi_version as u64s (in that
//!   order: sectors first, unlike the lister's printing order); then the
//!   remaining fields as varints, in the lister's order, each time field
//!   taking two varints;
//! * a varint's length is one more than the number of trailing one bits of
//!   its first byte; a 9-byte varint is the next eight bytes verbatim;
//!   otherwise the value is the little-endian bytes shifted right by the
//!   length.
//!
//! A dirent is keyed (directory inode, name hash); its value is the target
//! inode (u64), the type (u8, `DT_*` numbering) and the name, NUL-padded.
//! In a casefolded directory the type byte has bit 7 set and the value
//! holds two names, as given and folded, with their lengths (see
//! [`Dirent::from_key`]).

use crate::bkey::{key_type, Bkey};
use crate::error::{Error, Result};
use crate::util::{le16, le64};

/// The root directory's inode number (the lister lists `/`'s entries
/// under it).
pub const ROOT_INO: u64 = 4096;

/// The string hash type an inode uses for its directory entries and
/// extended attributes (flags bits 20..23). S1 (7.7) names three,
/// `crc32c`, `crc64` and `siphash` (the default); SipHash is 3 by
/// observation (S3, S4: every inode of every fixture carries 3 and the
/// lister prints `hash_type=siphash` for each). crc32c is 0 (S3, S8: every
/// inode of the `--str_hash=crc32c` images); crc64's number is open
/// question 17 in docs/clean-room.md.
pub const HASH_TYPE_SIPHASH: u8 = 3;
/// See [`HASH_TYPE_SIPHASH`].
pub const HASH_TYPE_CRC32C: u8 = 0;

/// Inode flag bit 10, which the reference lister prints as
/// `has_case_insensitive`. OBSERVED (S3): set on every inode of a
/// filesystem formatted with `--casefold` (the `casefold` set's 50, and
/// the four the reference mount made on another), and on none of the
/// 10199 inodes of every other image. A directory carrying it keeps its
/// entries in the casefolded layout and finds them by their folded name.
pub const INODE_HAS_CASE_INSENSITIVE: u32 = 1 << 10;

/// The inode flag set on an inode with options, set on it or inherited
/// (S3 + S4: bit 12 of bi_flags, on every inode of the kernel image that
/// carries one and on none that does not).
pub const INODE_HAS_OPTIONS: u64 = 1 << 12;

/// The option fields (of [`FIELD_NAMES`]) a new inode takes from its
/// directory (#142): the ones the reference tool sets per file
/// (`set-file-option`), and the targets and erasure coding it lists with
/// them. `project` is a quota, not one of them.
pub const OPTION_FIELDS: [&str; 8] = [
    "data_checksum",
    "compression",
    "background_compression",
    "data_replicas",
    "promote_target",
    "foreground_target",
    "background_target",
    "erasure_code",
];

/// Bit 7 of a dirent's type byte marks the casefolded layout. OBSERVED
/// (S4): 0x88, 0x8a and 0x84 for a file, a symlink and a directory in the
/// `casefold` set, against 8, 10 and 4 everywhere else.
const DIRENT_CASEFOLDED: u8 = 0x80;

/// The varint fields of an `inode_v3`, after the fixed part, in order.
/// Times take two varints each.
const FIELDS: &[(&str, usize)] = &[
    ("atime", 2),
    ("ctime", 2),
    ("mtime", 2),
    ("otime", 2),
    ("uid", 1),
    ("gid", 1),
    ("nlink", 1),
    ("generation", 1),
    ("dev", 1),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inode {
    pub ino: u64,
    pub mode: u32,
    pub flags: u32,
    pub size: u64,
    pub sectors: u64,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u32,
    pub atime: u64,
    pub ctime: u64,
    pub mtime: u64,
    /// The seed of a directory's name hash (fixed part, bytes 8..16).
    pub hash_seed: u64,
    /// The per-inode option fields as stored (#81): each holds its value
    /// plus one, 0 when the inode has none; see [`Inode::option`].
    pub options: InodeOptions,
}

/// The option fields of an inode as stored: `data_checksum`,
/// `compression`, `background_compression` and `data_replicas` hold the
/// option's value plus one (0: not set, the filesystem's own applies), and
/// `fields_set` has one bit per option set on this inode itself, counted
/// from `data_checksum` in [`FIELD_NAMES`] order; an inherited option is
/// stored but has no bit (S3 + S4: the reference tool's `set-file-option`
/// on files and a directory in the kernel oracle's VM, read back with
/// `get-file-option` and the lister, docs/clean-room.md open question 7).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InodeOptions {
    pub data_checksum: u64,
    pub compression: u64,
    pub background_compression: u64,
    pub data_replicas: u64,
    pub fields_set: u64,
}

/// Data checksum types, by stored value minus one.
const CHECKSUM_NAMES: &[&str] = &["none", "crc32c", "crc64", "xxhash"];
/// Compression types, by stored value minus one.
const COMPRESSION_NAMES: &[&str] = &["none", "lz4", "gzip", "zstd"];

impl Inode {
    pub fn is_dir(&self) -> bool {
        self.mode & 0o170000 == 0o040000
    }
    pub fn is_symlink(&self) -> bool {
        self.mode & 0o170000 == 0o120000
    }
    pub fn is_file(&self) -> bool {
        self.mode & 0o170000 == 0o100000
    }

    /// Whether this inode carries [`INODE_HAS_CASE_INSENSITIVE`]: as a
    /// directory, its entries are found by their folded names.
    pub fn is_casefolded(&self) -> bool {
        self.flags & INODE_HAS_CASE_INSENSITIVE != 0
    }

    /// The string hash type of this inode's names: [`HASH_TYPE_SIPHASH`] on
    /// every fixture.
    pub fn hash_type(&self) -> u8 {
        ((self.flags >> 20) & 0xf) as u8
    }

    /// The link count a mount reports for this inode (`st_nlink`).
    /// INFERRED from the aged fixture, where every inode's stored count
    /// was compared with what the mount reported: a file or symlink stores
    /// one less than its links (a file with three names stores 2), and a
    /// directory stores its number of subdirectories, to which the mount
    /// adds 2 for its own entry and `.`.
    pub fn link_count(&self) -> u32 {
        if self.is_dir() {
            self.nlink.saturating_add(2)
        } else {
            self.nlink.saturating_add(1)
        }
    }

    /// Option `name` (a field of [`InodeOptions`]) as the reference tool
    /// names it: `"lz4"`, `"crc64"`, `"1"`; `None` when the inode does not
    /// carry it (the filesystem's own applies) or `name` is not one of
    /// them. Inherited options are included. A value no reference image
    /// has shown comes back as `"unknown N"`, never as a guess.
    pub fn option(&self, name: &str) -> Option<String> {
        let (stored, names) = match name {
            "data_checksum" => (self.options.data_checksum, Some(CHECKSUM_NAMES)),
            "compression" => (self.options.compression, Some(COMPRESSION_NAMES)),
            "background_compression" => {
                (self.options.background_compression, Some(COMPRESSION_NAMES))
            }
            "data_replicas" => (self.options.data_replicas, None),
            _ => return None,
        };
        let v = stored.checked_sub(1)?;
        Some(match names {
            Some(n) => n
                .get(v as usize)
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("unknown {v}")),
            None => v.to_string(),
        })
    }

    pub fn from_key(k: &Bkey) -> Result<Self> {
        if k.key_type != key_type::INODE_V3 {
            return Err(Error::Unsupported(format!("inode key type {}", k.key_type)));
        }
        let v = &k.value;
        if v.len() < 48 {
            return Err(Error::Corrupt(
                "inode_v3 shorter than its fixed part".into(),
            ));
        }
        let fm = le64(v, 16);
        let nr_fields = ((fm >> 24) & 0xff) as usize;
        let mut ino = Inode {
            ino: k.pos.offset,
            mode: ((fm >> 36) & 0xffff) as u32,
            flags: fm as u32,
            sectors: le64(v, 24),
            size: le64(v, 32),
            uid: 0,
            gid: 0,
            nlink: 0,
            atime: 0,
            ctime: 0,
            mtime: 0,
            hash_seed: le64(v, 8),
            options: InodeOptions::default(),
        };
        let mut p = 48;
        for (i, (name, count)) in FIELDS.iter().enumerate() {
            if i >= nr_fields {
                break;
            }
            let mut first = 0;
            for j in 0..*count {
                let (val, n) = varint(&v[p.min(v.len())..])?;
                if j == 0 {
                    first = val;
                }
                p += n;
            }
            match *name {
                "atime" => ino.atime = first,
                "ctime" => ino.ctime = first,
                "mtime" => ino.mtime = first,
                "uid" => ino.uid = first as u32,
                "gid" => ino.gid = first as u32,
                "nlink" => ino.nlink = first as u32,
                _ => {}
            }
        }
        let raw = InodeV3Raw::parse(v)?;
        let f = |n: &str| raw.field(n).unwrap_or(0);
        ino.options = InodeOptions {
            data_checksum: f("data_checksum"),
            compression: f("compression"),
            background_compression: f("background_compression"),
            data_replicas: f("data_replicas"),
            fields_set: f("fields_set"),
        };
        Ok(ino)
    }
}

/// Decode one varint; returns the value and its length in bytes.
pub fn varint(b: &[u8]) -> Result<(u64, usize)> {
    let first = *b
        .first()
        .ok_or_else(|| Error::Corrupt("varint past the end of the inode".into()))?;
    let len = first.trailing_ones() as usize + 1;
    if len >= 9 {
        if b.len() < 9 {
            return Err(Error::Corrupt("9-byte varint truncated".into()));
        }
        return Ok((le64(b, 1), 9));
    }
    if b.len() < len {
        return Err(Error::Corrupt("varint truncated".into()));
    }
    let mut raw = [0u8; 8];
    raw[..len].copy_from_slice(&b[..len]);
    Ok((u64::from_le_bytes(raw) >> len, len))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirent {
    pub dir: u64,
    pub name: Vec<u8>,
    pub inum: u64,
    pub d_type: u8,
}

impl Dirent {
    pub fn from_key(k: &Bkey) -> Result<Self> {
        if k.key_type != key_type::DIRENT {
            return Err(Error::Unsupported(format!(
                "dirent key type {}",
                k.key_type
            )));
        }
        let v = &k.value;
        if v.len() < 10 {
            return Err(Error::Corrupt("dirent shorter than its fixed part".into()));
        }
        if v[8] & DIRENT_CASEFOLDED != 0 {
            let (name, _) = casefolded_names(v)?;
            return Ok(Dirent {
                dir: k.pos.inode,
                name: name.to_vec(),
                inum: le64(v, 0),
                d_type: v[8] & !DIRENT_CASEFOLDED,
            });
        }
        let raw = &v[9..];
        let n = raw
            .iter()
            .rposition(|&c| c != 0)
            .map(|i| i + 1)
            .unwrap_or(0);
        if n == 0 {
            return Err(Error::Corrupt("dirent with an empty name".into()));
        }
        Ok(Dirent {
            dir: k.pos.inode,
            name: raw[..n].to_vec(),
            inum: le64(v, 0),
            d_type: v[8],
        })
    }

    /// The type's name as the reference lister prints it.
    pub fn type_name(&self) -> &'static str {
        match self.d_type {
            1 => "fifo",
            2 => "chr",
            4 => "dir",
            6 => "blk",
            8 => "reg",
            10 => "lnk",
            12 => "sock",
            _ => "unknown",
        }
    }
}

/// A casefolded dirent value's two names, as given and folded. After the
/// target inode and the type byte come two zero bytes, the name's length
/// and the folded name's length (u16 each), the two names back to back,
/// and zeros to a whole u64. OBSERVED (S4): every casefolded dirent of the
/// `casefold` set, against the lister's `Name (casefold name)`; e.g.
/// `Hello.TXT` is `00 00 09 00 09 00`, `Hello.TXThello.txt`, 7 zeros.
fn casefolded_names(v: &[u8]) -> Result<(&[u8], &[u8])> {
    if v.len() < 15 {
        return Err(Error::Corrupt(
            "casefolded dirent shorter than its fixed part".into(),
        ));
    }
    if le16(v, 9) != 0 {
        return Err(Error::Unsupported(format!(
            "casefolded dirent with {:#06x} after its type, 0 in every one seen: not read",
            le16(v, 9)
        )));
    }
    let n = usize::from(le16(v, 11));
    let f = usize::from(le16(v, 13));
    let end = 15 + n + f;
    if n == 0 || f == 0 || end > v.len() || v[end..].iter().any(|&b| b != 0) {
        return Err(Error::Corrupt(format!(
            "casefolded dirent names of {n} and {f} bytes do not fit its {}-byte value",
            v.len()
        )));
    }
    Ok((&v[15..15 + n], &v[15 + n..end]))
}

/// A name's folded form in a casefolded directory (#111): Unicode full case
/// folding after canonical decomposition, then decomposed again (NFD), as
/// the `casefold` set's folded names show it: `Straße` folds to `strasse`
/// and `ÉCOLE` to `e`, U+0301, `cole` (S4). An ASCII name folds to its
/// lowercase. `None` for a name that is not UTF-8, which has no folded
/// form; such a name is matched as stored. The tables are the crates'
/// (`caseless`, `unicode-normalization`), of a newer Unicode than the
/// encoding the superblock names (utf8-12.1.0): a character whose folding
/// changed since then would fold differently (open question 18).
pub fn casefold(name: &[u8]) -> Option<Vec<u8>> {
    use unicode_normalization::UnicodeNormalization;
    if name.is_ascii() {
        return Some(name.to_ascii_lowercase());
    }
    let s = std::str::from_utf8(name).ok()?;
    let decomposed: String = s.nfd().collect();
    let folded = caseless::default_case_fold_str(&decomposed);
    Some(folded.nfd().collect::<String>().into_bytes())
}

/// The folded name a dirent is found by, when it is in the casefolded
/// layout; `None` for any other dirent.
pub fn dirent_folded_name(k: &Bkey) -> Result<Option<Vec<u8>>> {
    Dirent::from_key(k)?;
    if k.value[8] & DIRENT_CASEFOLDED == 0 {
        return Ok(None);
    }
    Ok(Some(casefolded_names(&k.value)?.1.to_vec()))
}

/// A dirent's offset in its directory: SipHash-2-4 of the name, keyed with
/// the directory's `hash_seed` and 0, shifted right by one. INFERRED by
/// computing candidates against the dirents of the write study and checked
/// against every dirent of every fixture (docs/clean-room.md).
pub fn dirent_hash(dir_hash_seed: u64, name: &[u8]) -> u64 {
    crate::siphash::siphash24(dir_hash_seed, 0, name) >> 1
}

/// A name's hash slot in a directory of string hash type `hash_type`, or
/// `None` for a type whose hash is not known. SipHash is
/// [`dirent_hash`]; crc32c is CRC-32C, starting from all ones with no final
/// XOR, over the directory's `hash_seed` (eight bytes, little-endian) and
/// then the name. INFERRED by computing candidates against the write
/// study's crc32c images and checked against every dirent of them and of
/// the `strhash` fixture (docs/clean-room.md).
pub fn name_hash(hash_type: u8, dir_hash_seed: u64, name: &[u8]) -> Option<u64> {
    match hash_type {
        HASH_TYPE_SIPHASH => Some(dirent_hash(dir_hash_seed, name)),
        HASH_TYPE_CRC32C => {
            let mut msg = dir_hash_seed.to_le_bytes().to_vec();
            msg.extend_from_slice(name);
            // The standard crc32c inverts its result; this hash does not.
            Some(u64::from(!crate::csum::crc32c_nonzero(&msg)))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Hello.TXT (casefold hello.txt) -> 2147483649 type reg`, in the root
    /// of the `casefold` set, as its image holds it.
    #[test]
    fn a_casefolded_dirent_gives_its_name_as_given_and_its_folded_name() {
        let mut value = vec![1, 0, 0, 0x80, 0, 0, 0, 0, 0x88, 0, 0, 9, 0, 9, 0];
        value.extend_from_slice(b"Hello.TXThello.txt");
        value.resize(40, 0);
        let k = Bkey {
            key_type: key_type::DIRENT,
            size: 0,
            version_hi: 0,
            version_lo: 0,
            pos: crate::bkey::Bpos {
                inode: ROOT_INO,
                offset: 6_254_422_998_957_400_532,
                snapshot: u32::MAX,
            },
            value,
        };
        let d = Dirent::from_key(&k).unwrap();
        assert_eq!(d.name, b"Hello.TXT");
        assert_eq!((d.inum, d.d_type), (2_147_483_649, 8));
        let folded = dirent_folded_name(&k).unwrap();
        assert_eq!(folded.as_deref(), Some(&b"hello.txt"[..]));
        // Bytes 9..10 were 0 in every one seen; lengths past the value are
        // corrupt.
        let mut odd = k.clone();
        odd.value[9] = 1;
        assert!(matches!(Dirent::from_key(&odd), Err(Error::Unsupported(_))));
        let mut long = k.clone();
        long.value[13] = 40;
        assert!(matches!(Dirent::from_key(&long), Err(Error::Corrupt(_))));
        let mut plain = k;
        plain.value[8] = 8;
        assert_eq!(dirent_folded_name(&plain).unwrap(), None);
    }

    #[test]
    fn crc32c_name_hashes_are_the_offsets_the_lister_showed() {
        // The write study's collide image: /d (seed bfe7aaa9b28e0da5) and
        // the root (seed 998647e3513019f6), offsets as the lister printed.
        let d = 0xbfe7_aaa9_b28e_0da5;
        let root = 0x9986_47e3_5130_19f6;
        for (seed, name, offset) in [
            (d, &b"plain"[..], 1_929_410_222),
            (d, b"CAAAAAAAAAAAAAAA", 762_833_578),
            (root, b"d", 2_651_817_589),
            (root, b"lost+found", 277_940_468),
        ] {
            assert_eq!(name_hash(HASH_TYPE_CRC32C, seed, name), Some(offset));
        }
        assert_eq!(name_hash(1, d, b"plain"), None);
    }

    #[test]
    fn varints_decode_as_measured() {
        // atime 273717049 was stored as 2f e7 12 0a 02.
        assert_eq!(
            varint(&[0x2f, 0xe7, 0x12, 0x0a, 0x02]).unwrap(),
            (273_717_049, 5)
        );
        // nlink 2 as 04; inode 4096 as 01 40.
        assert_eq!(varint(&[0x04]).unwrap(), (2, 1));
        assert_eq!(varint(&[0x01, 0x40]).unwrap(), (4096, 2));
        let mut nine = vec![0xff];
        nine.extend_from_slice(&2710689797173725757u64.to_le_bytes());
        assert_eq!(varint(&nine).unwrap(), (2710689797173725757, 9));
        assert!(varint(&[]).is_err());
        assert!(varint(&[0xff, 1]).is_err());
        assert!(varint(&[0x03]).is_err());
    }
}

/// Encode a varint: the inverse of [`varint`]. The shortest length `n` (1 to
/// 8 bytes) whose `7 * n` value bits hold it, marked by `n - 1` trailing one
/// bits and a zero; past 56 bits, 0xff and the 8 bytes verbatim.
pub fn varint_encode(v: u64, out: &mut Vec<u8>) {
    for n in 1..=8usize {
        if v < 1u64 << (7 * n) {
            let word = (v << n) | ((1u64 << (n - 1)) - 1);
            out.extend_from_slice(&word.to_le_bytes()[..n]);
            return;
        }
    }
    out.push(0xff);
    out.extend_from_slice(&v.to_le_bytes());
}

/// An `inode_v3` value as stored, every field kept: the fixed part, then
/// the varints exactly as many as were stored (each time is two varints).
/// Decoding then encoding gives back the same bytes (checked against every
/// inode of every fixture).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InodeV3Raw {
    pub journal_seq: u64,
    pub hash_seed: u64,
    /// bi_flags in bits 0..20, hash type 20..24, number of varint fields
    /// 24..32, four bits not yet understood 32..36, mode 36..52.
    pub flags: u64,
    pub sectors: u64,
    pub size: u64,
    pub version: u64,
    pub varints: Vec<u64>,
}

/// The varint fields that take two varints: the four times, which come
/// first.
const TWO_VARINT_FIELDS: usize = 4;

/// The single-varint fields after the four times, in the order they are
/// stored and the reference lister prints them (`bi_<name>`). Checked by
/// name against the lister for every inode of every dumped image
/// (tests/oracle_inode_fields.rs, #81). A file stores 21 fields, through
/// `dir_offset`; a directory 25, through `depth`; a missing field is 0.
pub const FIELD_NAMES: &[&str] = &[
    "uid",
    "gid",
    "nlink",
    "generation",
    "dev",
    "data_checksum",
    "compression",
    "project",
    "background_compression",
    "data_replicas",
    "promote_target",
    "foreground_target",
    "background_target",
    "erasure_code",
    "fields_set",
    "dir",
    "dir_offset",
    "subvol",
    "parent_subvol",
    "nocow",
    "depth",
    "inodes_32bit",
    "casefold",
    "unused_ec_max_data_blocks",
];

impl InodeV3Raw {
    /// The field named `name` (one of [`FIELD_NAMES`]), 0 when the inode
    /// stores fewer fields; `None` for a name not in the list.
    pub fn field(&self, name: &str) -> Option<u64> {
        let i = FIELD_NAMES.iter().position(|&n| n == name)?;
        Some(
            self.varints
                .get(2 * TWO_VARINT_FIELDS + i)
                .copied()
                .unwrap_or(0),
        )
    }

    /// The time named `name` (`atime`, `ctime`, `mtime` or `otime`, the
    /// lister's `bi_<name>`) as stored: the first of its two varints, 0 when
    /// the inode stores fewer fields; `None` for another name.
    pub fn time(&self, name: &str) -> Option<u64> {
        let i = FIELDS[..TWO_VARINT_FIELDS]
            .iter()
            .position(|&(n, _)| n == name)?;
        Some(self.varints.get(2 * i).copied().unwrap_or(0))
    }

    pub fn parse(v: &[u8]) -> Result<Self> {
        if v.len() < 48 {
            return Err(Error::Corrupt(
                "inode_v3 shorter than its fixed part".into(),
            ));
        }
        let flags = le64(v, 16);
        let nr_fields = ((flags >> 24) & 0xff) as usize;
        let n = nr_fields + nr_fields.min(TWO_VARINT_FIELDS);
        let mut varints = Vec::with_capacity(n);
        let mut p = 48;
        for _ in 0..n {
            let (val, len) = varint(&v[p.min(v.len())..])?;
            varints.push(val);
            p += len;
        }
        if v[p..].iter().any(|&b| b != 0) {
            return Err(Error::Corrupt(
                "inode_v3 has bytes after its last field".into(),
            ));
        }
        Ok(InodeV3Raw {
            journal_seq: le64(v, 0),
            hash_seed: le64(v, 8),
            flags,
            sectors: le64(v, 24),
            size: le64(v, 32),
            version: le64(v, 40),
            varints,
        })
    }

    pub fn mode(&self) -> u32 {
        ((self.flags >> 36) & 0xffff) as u32
    }

    /// The string hash type (flags bits 20..23), see [`HASH_TYPE_SIPHASH`].
    pub fn hash_type(&self) -> u8 {
        ((self.flags >> 20) & 0xf) as u8
    }

    /// The value bytes, zero-padded to a whole u64.
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(96);
        for f in [
            self.journal_seq,
            self.hash_seed,
            self.flags,
            self.sectors,
            self.size,
            self.version,
        ] {
            b.extend_from_slice(&f.to_le_bytes());
        }
        for &v in &self.varints {
            varint_encode(v, &mut b);
        }
        b.resize(b.len().div_ceil(8) * 8, 0);
        b
    }
}

impl Dirent {
    /// The value bytes: target inode u64, DT_* type u8, the name, zero
    /// padding to a whole u64.
    pub fn encode_value(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(16 + self.name.len());
        b.extend_from_slice(&self.inum.to_le_bytes());
        b.push(self.d_type);
        b.extend_from_slice(&self.name);
        b.resize(b.len().div_ceil(8) * 8, 0);
        b
    }
}
