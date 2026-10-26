//! Extent values: a list of typed entries (device pointers and checksum
//! entries), and what they say about where a file's data is.
//!
//! Provenance (docs/clean-room.md): an entry's type is the position of the
//! lowest set bit of its first word; a pointer has a device, a 44-bit
//! sector offset and a generation; crc32/crc64/crc128 entries are 8/16/24
//! bytes and carry compressed size, uncompressed size, offset, nonce,
//! checksum type, compression type and the checksum; a crc entry applies
//! to the pointers after it (S1, 9.1.3). Bit positions were found by
//! hexdump against the reference lister's printed values (S3, S4):
//!
//! ```text
//! ptr    bit 0 = 1; bits 1..3 flags (0b100 unwritten, the rest unread);
//!        bits 4..47 sector offset;
//!        bits 48..55 device; bits 56..63 generation   (device and
//!        generation: only ever 0 in the fixtures -- an open question)
//! crc32  bits 0..1 = 0b10; bits 2..8 compressed_size-1; 9..15
//!        uncompressed_size-1; 16..22 offset; 24..27 csum type;
//!        28..31 compression type; 32..63 checksum
//! crc64  bits 0..2 = 0b100; 3..11 compressed_size-1; 12..20
//!        uncompressed_size-1; 21..29 offset; 30..39 nonce; 40..43 csum
//!        type; 44..47 compression type; 48..63 checksum high 16 bits;
//!        second word: checksum low 64 bits
//! crc128 bits 0..3 = 0b1000; 4..16 compressed_size-1; 17..29
//!        uncompressed_size-1; 30..42 offset; 43..55 nonce (0 in every
//!        extent seen; not read unless 0); 56..59 csum type; 60..63
//!        compression type; second and third words: checksum low and high
//!        64 bits
//! stripe bits 0..4 = 0b1_0000; 5..12 block; 13..16 not printed (1 in
//!        every one seen); 17..63 stripe index; one word (refused)
//! flags  bits 0..6 = 0b100_0000; bit 7 poisoned; one word
//! ```

use crate::error::{Error, Result};
use crate::util::le64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ptr {
    pub dev: u8,
    pub offset: u64,
    pub gen: u8,
    pub flags: u8,
}

/// A pointer's flags when it marks an unwritten extent: space allocated
/// and never written, which reads as zeros (S1's metadata version history,
/// `unwritten_extents`: "Preallocated extents that read as zeros").
/// OBSERVED (#158, S3 + S4): the reference kernel module, asked to
/// `fallocate` a MiB in a file the reference tool marked `nocow`, wrote
/// extents whose pointers carry flags 0b100 (word bit 3), each printed
/// `unwritten` by the reference lister, and one pointer with no flag where
/// 4 KiB were then written. The same in a file without `nocow` made
/// `reservation` keys instead. The other flag bits, which S1 says mark
/// cached pointers, are still unobserved.
pub const PTR_UNWRITTEN: u8 = 0b100;

/// The generation a bucket's `alloc_v4` key records: byte 4 of its second
/// word (docs/clean-room.md, "Allocating space": word 1 holds the flags in
/// byte 0, the generation in byte 4, the oldest generation in byte 5).
pub fn alloc_v4_gen(value: &[u8]) -> Option<u8> {
    value.get(12).copied()
}

impl Ptr {
    /// Whether this pointer marks an unwritten extent ([`PTR_UNWRITTEN`]).
    pub fn is_unwritten(&self) -> bool {
        self.flags == PTR_UNWRITTEN
    }

    /// Whether this pointer may be read on device `dev_idx`, whose bucket
    /// `bucket_gen` is the generation the allocator currently records for
    /// the pointer's bucket (`None` when the bucket has no alloc key).
    ///
    /// S1 9.1.3.1: a pointer carries "a generation number that must match
    /// the bucket's current generation to be valid (stale pointers are
    /// detected and dropped during reads)", and flags that "distinguish
    /// cached pointers ... from dirty pointers, and mark unwritten
    /// reservations". A pointer at another device is not on this one; a
    /// stale pointer names a reused bucket; an unwritten pointer is judged
    /// like any other ([`PTR_UNWRITTEN`]), and a pointer with any other flag
    /// has a meaning (cached) this reader has never observed and refuses.
    pub fn check(&self, dev_idx: u8, bucket_gen: Option<u8>) -> crate::Result<()> {
        if self.dev != dev_idx {
            return Err(crate::Error::Corrupt(format!(
                "pointer to sector {} is for device {}, and this is device {dev_idx} of a \
                 single-device filesystem",
                self.offset, self.dev
            )));
        }
        if self.flags != 0 && !self.is_unwritten() {
            return Err(crate::Error::Unsupported(format!(
                "pointer to sector {} carries flags {:#05b} (cached?): not read",
                self.offset, self.flags
            )));
        }
        match bucket_gen {
            Some(g) if g != self.gen => Err(crate::Error::Corrupt(format!(
                "stale pointer to sector {}: generation {}, the bucket's is {g}",
                self.offset, self.gen
            ))),
            Some(_) => Ok(()),
            None => Err(crate::Error::Corrupt(format!(
                "pointer to sector {} names a bucket with no alloc key",
                self.offset
            ))),
        }
    }
}

/// A checksum/compression entry, unpacked. Sizes are in sectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crc {
    pub compressed_size: u32,
    pub uncompressed_size: u32,
    pub offset: u32,
    pub nonce: u32,
    pub csum_type: u8,
    pub compression_type: u8,
    pub csum_hi: u64,
    pub csum_lo: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtentEntry {
    Ptr(Ptr),
    Crc(Crc),
}

/// Compression types as stored in a crc entry. INFERRED from the lz4,
/// gzip and zstd fixtures against the lister's `compress` column: 0 none,
/// 2 gzip, 3 lz4, 4 zstd, 5 incompressible (stored raw) -- not the
/// numbering of the filesystem option. Type 1 never appeared.
pub mod compression {
    pub const NONE: u8 = 0;
    pub const GZIP: u8 = 2;
    pub const LZ4: u8 = 3;
    pub const ZSTD: u8 = 4;
    pub const INCOMPRESSIBLE: u8 = 5;
}

fn bits(w: u64, lo: u32, n: u32) -> u64 {
    (w >> lo) & ((1u64 << n) - 1)
}

/// The name of an extent entry kind, by the position of its first set bit,
/// each OBSERVED against the lister's line for it (docs/clean-room.md,
/// open question 19): crc128 on the `crc128` fixture (`crc128:`), flags on
/// the `poison` image (`flags: poisoned`), reconcile on `bgcompress`
/// (`reconcile: need_rb=...`), and the stripe pointer on the fixture
/// build's erasure-coding probe (`stripe_ptr: idx N block B`, three
/// devices). 5 is the one kind left unnamed.
pub fn entry_kind_name(first_set_bit: u32) -> &'static str {
    match first_set_bit {
        0 => "ptr",
        1 => "crc32",
        2 => "crc64",
        3 => "crc128",
        4 => "stripe_ptr",
        6 => "flags",
        7 => "reconcile",
        _ => "unknown",
    }
}

/// The flags entry's poisoned bit (S1 5.5.5, 9.1.3.4: the extent's data
/// failed its checksum with no good copy left, and reads of it fail).
/// OBSERVED on the `poison` image: the reference's read of a corrupted
/// extent put the word 0xc0 in front of its crc32 entry, and the lister
/// printed `flags: poisoned` for it.
const FLAG_POISONED: u64 = 1 << 7;

/// Parse an extent value into its entries.
/// A `reflink_p`'s index into the reflink btree, in sectors: the low 56
/// bits of its first word (S1 9.1.6 gives 56 bits; docs/clean-room.md,
/// "Reflinks", the lister's view of the reference kernel's reflink).
pub fn reflink_p_idx(v: &[u8]) -> Result<u64> {
    if v.len() < 16 {
        return Err(Error::Corrupt(format!(
            "a reflink_p value of {} bytes, not 16",
            v.len()
        )));
    }
    Ok(crate::util::le64(v, 0) & ((1 << 56) - 1))
}

pub fn parse_entries(v: &[u8]) -> Result<Vec<ExtentEntry>> {
    Ok(entries(v)?.0)
}

/// Whether an extent value carries a flags entry marking it poisoned:
/// its data is known to be bad, and the reference refuses to read it.
pub fn poisoned(v: &[u8]) -> Result<bool> {
    Ok((entries(v)?.1 & FLAG_POISONED) != 0)
}

/// The data entries of a value, and the bits of its flags entry (0 when
/// it has none).
fn entries(v: &[u8]) -> Result<(Vec<ExtentEntry>, u64)> {
    let mut out = Vec::new();
    let mut flags = 0;
    let mut p = 0;
    while p + 8 <= v.len() {
        let w = le64(v, p);
        if w == 0 {
            break; // padding at the end of the value
        }
        match w.trailing_zeros() {
            0 => {
                out.push(ExtentEntry::Ptr(Ptr {
                    flags: bits(w, 1, 3) as u8,
                    offset: bits(w, 4, 44),
                    dev: bits(w, 48, 8) as u8,
                    gen: bits(w, 56, 8) as u8,
                }));
                p += 8;
            }
            1 => {
                out.push(ExtentEntry::Crc(Crc {
                    compressed_size: bits(w, 2, 7) as u32 + 1,
                    uncompressed_size: bits(w, 9, 7) as u32 + 1,
                    offset: bits(w, 16, 7) as u32,
                    nonce: 0,
                    csum_type: bits(w, 24, 4) as u8,
                    compression_type: bits(w, 28, 4) as u8,
                    csum_hi: 0,
                    csum_lo: bits(w, 32, 32),
                }));
                p += 8;
            }
            2 => {
                if p + 16 > v.len() {
                    return Err(Error::Corrupt("crc64 entry runs past the value".into()));
                }
                out.push(ExtentEntry::Crc(Crc {
                    compressed_size: bits(w, 3, 9) as u32 + 1,
                    uncompressed_size: bits(w, 12, 9) as u32 + 1,
                    offset: bits(w, 21, 9) as u32,
                    nonce: bits(w, 30, 10) as u32,
                    csum_type: bits(w, 40, 4) as u8,
                    compression_type: bits(w, 44, 4) as u8,
                    csum_hi: bits(w, 48, 16),
                    csum_lo: le64(v, p + 8),
                }));
                p += 16;
            }
            3 => {
                // Three words, MEASURED on the `crc128` fixture: every key
                // with a crc128 and one pointer is 9 u64s (the 5 of the key,
                // then 3 and 1), and for `c_size 832 size 832 ... csum
                // crc32c 0:d6f4f09 compress incompressible` the words read
                // 0x5500_0000_067e_33f8, 0x0d6f_4f09, 0.
                if p + 24 > v.len() {
                    return Err(Error::Corrupt("crc128 entry runs past the value".into()));
                }
                // The offset is bits 30..42, MEASURED on the
                // `crc128-overwrite` image: of one extent split by a 4K
                // overwrite, the part after it (`offset 16`) differs from
                // the part before (`offset 0`) in 16 << 30 alone. The nonce is
                // 0 in every entry seen, and S1 gives it only encryption,
                // which this reader refuses: a value there is not read.
                if bits(w, 43, 13) != 0 {
                    return Err(Error::Unsupported(format!(
                        "crc128 entry with nonce bits {:#x}: never seen, so the extent is not \
                         read (docs/clean-room.md, open question 19)",
                        bits(w, 43, 13)
                    )));
                }
                out.push(ExtentEntry::Crc(Crc {
                    compressed_size: bits(w, 4, 13) as u32 + 1,
                    uncompressed_size: bits(w, 17, 13) as u32 + 1,
                    offset: bits(w, 30, 13) as u32,
                    nonce: 0,
                    csum_type: bits(w, 56, 4) as u8,
                    compression_type: bits(w, 60, 4) as u8,
                    csum_hi: le64(v, p + 16),
                    csum_lo: le64(v, p + 8),
                }));
                p += 24;
            }
            4 => {
                // A stripe pointer: the extent is erasure coded, which
                // takes several devices (S1 9.1.3.3), and this reader
                // reads one. OBSERVED on the fixture build's three-device
                // probe: `stripe_ptr: idx 1 block 1` is the word 0x22030
                // between the crc32 and the pointer.
                return Err(Error::Unsupported(format!(
                    "extent entry type 4 (stripe_ptr idx {} block {}): an erasure-coded extent \
                     is not read",
                    w >> 17,
                    bits(w, 5, 8)
                )));
            }
            6 => {
                // Flags: one word (S3, S4: the poisoned extent's key is
                // 8 u64s, the flags word, then crc32 and ptr). S1 names one
                // flag; any other bit is a meaning not seen, and refused.
                let f = w & !0x7f;
                if (f & !FLAG_POISONED) != 0 {
                    return Err(Error::Unsupported(format!(
                        "extent flags {f:#x}: a flag other than poisoned is not known, so the \
                         extent is not read"
                    )));
                }
                flags |= f;
                p += 8;
            }
            7 => {
                // Reconcile: pending background work and the IO options it
                // is for, not where the data is. One word, MEASURED: on the
                // `bgcompress` fixture every crc32+ptr extent is 7 u64s
                // without it and 8 with it (302 of 302), and its word reads
                // 0x0000_0010_9010_0080 for `need_rb=background_compression
                // replicas=1 checksum=crc32c background_compression=lz4`.
                // Its fields are not decoded; the data reads without them.
                p += 8;
            }
            t => {
                return Err(Error::Unsupported(format!(
                    "extent entry type {t} ({}): its layout has not been observed, so the \
                     extent is not read (docs/clean-room.md, open question 19)",
                    entry_kind_name(t)
                )))
            }
        }
    }
    Ok((out, flags))
}

/// One piece of a file's data: `len` sectors of the file starting at file
/// sector `file_start`, found `skip` sectors into the (decompressed) data
/// the pointer and crc describe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataExtent {
    pub file_start: u64,
    pub len: u64,
    pub ptr: Ptr,
    pub crc: Option<Crc>,
    pub skip: u64,
}

impl DataExtent {
    /// From an extent key (whose position is its end) and its value: the
    /// first pointer, with the crc entry that precedes it.
    pub fn from_key(k: &crate::bkey::Bkey) -> Result<Self> {
        if u64::from(k.size) > k.pos.offset {
            return Err(Error::Corrupt("extent size exceeds its logical end".into()));
        }
        let mut crc = None;
        for e in parse_entries(&k.value)? {
            match e {
                ExtentEntry::Crc(c) => crc = Some(c),
                ExtentEntry::Ptr(ptr) => {
                    return Ok(DataExtent {
                        file_start: k.start_offset(),
                        len: k.size as u64,
                        ptr,
                        skip: crc.map(|c| c.offset as u64).unwrap_or(0),
                        crc,
                    })
                }
            }
        }
        Err(Error::Corrupt(format!(
            "extent at {} has no pointer",
            k.pos
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crc32_entry_decodes_as_the_lister_printed_it() {
        // "crc32: c_size 5 size 50 offset 0 csum crc32c 0:d192fb38 compress lz4"
        let w: u64 = 0xd192_fb38_3500_6212;
        let e = parse_entries(&w.to_le_bytes()).unwrap();
        let ExtentEntry::Crc(c) = e[0] else {
            panic!("{e:?}")
        };
        assert_eq!(
            (c.compressed_size, c.uncompressed_size, c.offset),
            (5, 50, 0)
        );
        assert_eq!(
            (c.csum_type, c.compression_type, c.csum_lo),
            (5, compression::LZ4, 0xd192_fb38)
        );
    }

    #[test]
    fn a_crc64_entry_and_pointer_decode() {
        // "crc64: c_size 50 size 512 offset 0 ... csum crc32c 0:8fd3928b compress lz4"
        let mut v = 0x0000_3500_001f_f18cu64.to_le_bytes().to_vec();
        v.extend_from_slice(&0x8fd3_928bu64.to_le_bytes());
        v.extend_from_slice(&0x17401u64.to_le_bytes());
        let e = parse_entries(&v).unwrap();
        let ExtentEntry::Crc(c) = e[0] else {
            panic!("{e:?}")
        };
        assert_eq!(
            (c.compressed_size, c.uncompressed_size, c.csum_lo),
            (50, 512, 0x8fd3_928b)
        );
        assert_eq!(
            e[1],
            ExtentEntry::Ptr(Ptr {
                dev: 0,
                offset: 0x1740,
                gen: 0,
                flags: 0
            })
        );
    }

    #[test]
    fn unknown_or_truncated_entries_are_refused_by_name() {
        // The erasure-coding probe's "stripe_ptr: idx 1 block 1".
        match parse_entries(&0x2_2030u64.to_le_bytes()) {
            Err(Error::Unsupported(m)) => {
                assert!(m.contains("type 4 (stripe_ptr idx 1 block 1)"), "{m}")
            }
            other => panic!("{other:?}"),
        }
        match parse_entries(&0x20u64.to_le_bytes()) {
            Err(Error::Unsupported(m)) => assert!(m.contains("type 5 (unknown)"), "{m}"),
            other => panic!("{other:?}"),
        }
        assert!(parse_entries(&0x4u64.to_le_bytes()).is_err());
        let truncated = parse_entries(&0x8u64.to_le_bytes());
        assert!(matches!(truncated, Err(Error::Corrupt(_))));
    }

    fn words(ws: &[u64]) -> Vec<u8> {
        ws.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    /// The `crc128` fixture's entries and pointers, as found in its image
    /// beside the lister's lines for them.
    #[test]
    fn a_crc128_entry_decodes_as_the_lister_printed_it() {
        // "crc128: c_size 832 size 832 offset 0 nonce 0 csum crc32c
        // 0:d6f4f09 compress incompressible", "ptr: 0:57:192 gen 0"
        let v = words(&[0x5500_0000_067e_33f8, 0x0d6f_4f09, 0, 0xe_4c01]);
        let e = parse_entries(&v).unwrap();
        let ExtentEntry::Crc(c) = e[0] else {
            panic!("{e:?}")
        };
        assert_eq!(
            (c.compressed_size, c.uncompressed_size, c.offset, c.nonce),
            (832, 832, 0, 0)
        );
        assert_eq!(
            (c.csum_type, c.compression_type, c.csum_hi, c.csum_lo),
            (5, compression::INCOMPRESSIBLE, 0, 0x0d6f_4f09)
        );
        assert!(matches!(e[1], ExtentEntry::Ptr(p) if p.offset == 58560));
        // "crc128: c_size 256 size 2048 ... csum crc32c 0:c50c3e7e compress lz4"
        let v = words(&[0x3500_0000_0ffe_0ff8, 0xc50c_3e7e, 0]);
        let e = parse_entries(&v).unwrap();
        let ExtentEntry::Crc(c) = e[0] else {
            panic!("{e:?}")
        };
        assert_eq!(
            (c.compressed_size, c.uncompressed_size, c.compression_type),
            (256, 2048, compression::LZ4)
        );
        // The `crc128-overwrite` image's "crc128: c_size 1024 size 1024
        // offset 16 ... 0:f1c6c44 compress none".
        let v = words(&[0x0500_0004_07fe_3ff8, 0x0f1c_6c44, 0]);
        let e = parse_entries(&v).unwrap();
        let ExtentEntry::Crc(c) = e[0] else {
            panic!("{e:?}")
        };
        assert_eq!(
            (c.offset, c.compressed_size, c.csum_lo),
            (16, 1024, 0x0f1c_6c44)
        );
        // A nonce was never seen: refused, not guessed.
        let nonce = parse_entries(&words(&[0x0500_0000_07fe_3ff8 | 1 << 43, 0, 0]));
        match nonce {
            Err(Error::Unsupported(m)) => assert!(m.contains("nonce"), "{m}"),
            other => panic!("{other:?}"),
        }
        let truncated = parse_entries(&words(&[0x5500_0000_067e_33f8, 0]));
        assert!(matches!(truncated, Err(Error::Corrupt(_))));
    }

    /// The `poison` image's poisoned extent: a flags word of 0xc0, then the
    /// crc32 and pointer it had before the reference's read failed.
    #[test]
    fn a_poisoned_flags_entry_is_seen_and_the_data_entries_still_decode() {
        let v = words(&[0xc0, 0xffa6_681b_0500_7efe, 0x1_7001]);
        assert!(poisoned(&v).unwrap());
        let e = parse_entries(&v).unwrap();
        assert!(matches!(e[0], ExtentEntry::Crc(c) if c.csum_lo == 0xffa6_681b));
        assert!(matches!(e[1], ExtentEntry::Ptr(p) if p.offset == 5888));
        assert!(!poisoned(&words(&[0xffa6_681b_0500_7efe, 0x1_7001])).unwrap());
        // A flags word with only its kind bit set marks nothing; any flag
        // but poisoned is refused.
        assert!(!poisoned(&words(&[0x40, 0x1_7001])).unwrap());
        match parse_entries(&words(&[0x140, 0x1_7001])) {
            Err(Error::Unsupported(m)) => assert!(m.contains("extent flags 0x100"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    /// The `bgcompress` fixture's crc32, ptr and reconcile words, as found
    /// in its image: the reconcile word is passed over and the data
    /// entries around it decode.
    #[test]
    fn a_reconcile_entry_is_passed_over() {
        let mut v = Vec::new();
        for w in [0x40f8_e88f_0500_0002u64, 0x4_dea1, 0x10_9010_0080] {
            v.extend_from_slice(&w.to_le_bytes());
        }
        let e = parse_entries(&v).unwrap();
        assert_eq!(e.len(), 2, "{e:?}");
        assert!(matches!(e[0], ExtentEntry::Crc(_)) && matches!(e[1], ExtentEntry::Ptr(_)));
    }
}
