//! The superblock's crypt field (type 2), which an encrypted filesystem
//! carries (S1 9.1.4, #159).
//!
//! S1 (9.1.4.1) says what the field is for, in prose: it keeps a random
//! 256-bit master key, sealed by a key derived from the passphrase through
//! scrypt with parameters the field also stores, and a magic value stored
//! with the key verifies the passphrase. The byte layout is S4, observed
//! on the two encrypted fixtures (docs/clean-room.md, "Encryption"):
//!
//! ```text
//! bytes  0..8    zero on both
//! bytes  8..16   zero without a passphrase (`encrypted`);
//!                0e 00 03 00 04 00 00 00 with one (`encrypted-passphrase`)
//! bytes 16..24   "bch**key", in the clear without a passphrase
//! bytes 24..56   the master key, in the clear without a passphrase
//! ```
//!
//! With a passphrase bytes 16..56 do not read the magic: the magic and the
//! key are sealed together; unsealing them is #273's.
//!
//! An unsealed key decrypts and authenticates the btree nodes (#271). S1
//! (9.1.4) says metadata is encrypted with ChaCha20 under the master key,
//! with a 128-bit nonce and separate domain separators, and authenticated
//! with a 128-bit Poly1305 MAC stored where a checksum would be. The
//! ciphers are RFC 8439's. Everything else is S4: candidate computations
//! over `encrypted.img`, each kept only where it reproduced the stored MAC
//! of every record of every node, or decrypted a header to the bounds its
//! parent's pointer names (docs/clean-room.md, "Encryption: btree nodes"):
//!
//! ```text
//! nonce:     four little-endian u32 words, ChaCha20's state words 12..15:
//!            word 0 its 32-bit block counter, words 1..3 RFC 8439's nonce
//! node:      word 0 the record's byte offset in the node, words 1..2 the
//!            node's seq, word 3 0x2000_0000 ^ the bset's journal_seq
//! Poly1305:  keyed with the first 32 keystream bytes of the same nonce
//!            with 0x8000_0000 XORed into word 3
//! ```
//!
//! The data extents are decrypted and authenticated with the same key
//! (#272). S1 (9.1.4.1, 9.1.4.3) says an extent's nonce is made from its
//! version, compression type and uncompressed size with a per-CRC nonce
//! offset, and that its MAC is Poly1305 truncated to 80 bits in its crc
//! entry. The layout is S4 again, every candidate kept only where it
//! decrypted all 441 extents of `encrypted.img` to the tree it was
//! formatted from and reproduced every stored MAC (docs/clean-room.md,
//! "Encryption: data extents"):
//!
//! ```text
//! extent:    word 0 zero (the block counter from the extent's start),
//!            word 1 the key's version, word 2 zero, word 3 0x1000_0000
//! MAC:       Poly1305 over the stored bytes, keyed as for a node; its
//!            first 8 bytes are the crc entry's low checksum word, the next
//!            2 its high 16 bits
//! ```

use crate::error::{Error, Result};
use crate::extent::{compression, Crc};
use chacha20::cipher::{
    consts::U10, KeyIvInit, StreamCipher, StreamCipherCore, StreamCipherSeek, StreamCipherSeekCore,
};
use poly1305::universal_hash::KeyInit;

/// The key magic: the bytes "bch**key". OBSERVED at bytes 16..24 of the
/// `encrypted` fixture's crypt field, which was formatted without a
/// passphrase, so the magic S1 says is stored with the master key is
/// stored there in the clear.
pub const KEY_MAGIC: [u8; 8] = *b"bch**key";

/// The length of a crypt field's body: 56 bytes on both encrypted
/// fixtures, with and without a passphrase. OBSERVED.
pub const FIELD_BYTES: usize = 56;

/// An encrypted filesystem's crypt field.
#[derive(Clone, PartialEq, Eq)]
pub struct CryptField {
    /// Bytes 0..16: zero without a passphrase, non-zero in bytes 8..16
    /// with one. Whether those are S1's scrypt parameters, and how they
    /// are encoded, is #273's to establish.
    pub head: [u8; 16],
    /// Bytes 16..56: the key magic and the master key after it, in the
    /// clear or sealed by the passphrase's key.
    pub key: [u8; 40],
}

/// Prints whether the key is sealed, never the key: without a passphrase
/// those bytes are the master key itself.
impl std::fmt::Debug for CryptField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CryptField")
            .field("head", &self.head)
            .field("sealed", &self.is_sealed())
            .finish_non_exhaustive()
    }
}

impl CryptField {
    /// The field from its body. A body shorter than the key is corrupt;
    /// a longer one has not been observed, and is refused rather than
    /// read as though its extra bytes meant nothing.
    pub fn parse(body: &[u8]) -> Result<Self> {
        if body.len() < FIELD_BYTES {
            return Err(Error::Corrupt(format!(
                "a crypt field of {} bytes, too short for its key",
                body.len()
            )));
        }
        if body.len() > FIELD_BYTES {
            return Err(Error::Unsupported(format!(
                "a crypt field of {} bytes: only {FIELD_BYTES} have been observed",
                body.len()
            )));
        }
        let mut head = [0u8; 16];
        head.copy_from_slice(&body[..16]);
        let mut key = [0u8; 40];
        key.copy_from_slice(&body[16..FIELD_BYTES]);
        Ok(CryptField { head, key })
    }

    /// Whether a passphrase seals the master key: its magic does not read
    /// in the clear.
    pub fn is_sealed(&self) -> bool {
        self.key[..8] != KEY_MAGIC
    }

    /// The master key, when it is stored unsealed; `None` when a
    /// passphrase seals it. The 32 bytes after the magic decrypt every
    /// node of `encrypted.img` (S4, #271).
    pub fn master_key(&self) -> Option<MasterKey> {
        if self.is_sealed() {
            return None;
        }
        let mut k = [0u8; 32];
        k.copy_from_slice(&self.key[8..]);
        Some(MasterKey(k))
    }
}

/// A btree bset's checksum type when its record is encrypted: ChaCha20
/// with a 128-bit Poly1305 MAC. OBSERVED: every bset of `encrypted.img`
/// carries type 4, and S1 (9.1.4.3) says metadata always uses 128-bit MACs.
pub const CSUM_CHACHA20_POLY1305_128: u8 = 4;

/// Word 3 of a btree node record's nonce, before the bset's journal
/// sequence number is XORed in: the domain separator of btree nodes. S1
/// says each kind of encrypted thing has its own; this value is OBSERVED,
/// the only one of the sixteen top-nibble values whose keystream decrypts
/// the nodes of `encrypted.img`.
pub const NONCE_BTREE_NODE: u32 = 0x2000_0000;

/// A data extent's checksum type when it is encrypted: ChaCha20 with a
/// Poly1305 MAC truncated to 80 bits, S1's `chacha20_poly1305_80` (9.1.4.3,
/// the default). OBSERVED: the crc entry of every extent of
/// `encrypted.img` carries type 3, and its 80 stored bits are the first 80
/// of the MAC [`decrypt_extent`] computes.
pub const CSUM_CHACHA20_POLY1305_80: u8 = 3;

/// How many bytes of an extent's MAC its crc entry keeps: 80 bits.
const EXTENT_MAC_BYTES: usize = 10;

/// How many bytes of an extent's MAC its crc entry keeps under S1's
/// `wide_macs` (9.1.4.3, checksum type [`CSUM_CHACHA20_POLY1305_128`]): all
/// 128, the first 8 in the crc entry's low checksum word and the next 8 in
/// its high one, little-endian -- the 80-bit MAC's layout carried on, which
/// `tests/oracle_encrypted_wide_macs.rs` holds to the reference's image.
const WIDE_EXTENT_MAC_BYTES: usize = 16;

/// Word 3 of a data extent's nonce: S1's `BCH_NONCE_EXTENT` domain.
/// OBSERVED: of the sixteen top-nibble values, the only one whose
/// keystream turns the stored bytes of `encrypted.img`'s extents into the
/// files it was formatted from.
pub const NONCE_EXTENT: u32 = 0x1000_0000;

/// XORed into word 3 of a nonce for the keystream whose first 32 bytes key
/// Poly1305: S1's `BCH_NONCE_POLY` domain. OBSERVED: with
/// [`NONCE_BTREE_NODE`], the value that reproduces every stored MAC of
/// `encrypted.img`.
pub const NONCE_POLY: u32 = 0x8000_0000;

/// A nonce: four words loaded, little-endian, into ChaCha20's state words
/// 12..15. Word 0 is RFC 8439's 32-bit block counter (2.3), words 1..3 its
/// 96-bit nonce; S1 calls all 128 bits the nonce.
pub type Nonce = [u32; 4];

/// The filesystem's master key. Its `Debug` never prints it.
#[derive(Clone)]
pub struct MasterKey([u8; 32]);

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MasterKey(..)")
    }
}

impl MasterKey {
    pub fn new(bytes: [u8; 32]) -> Self {
        MasterKey(bytes)
    }

    /// XOR `data` with the ChaCha20 keystream of `nonce`, from the block
    /// its word 0 names. A keystream that would run past the 32-bit block
    /// counter is refused: whether it wraps or carries into word 1 has not
    /// been observed.
    pub fn apply(&self, nonce: Nonce, data: &mut [u8]) -> Result<()> {
        let blocks = (data.len() as u64).div_ceil(64);
        let end = u64::from(nonce[0]) + blocks;
        if end > 1 << 32 {
            return Err(Error::Unsupported(format!(
                "{} bytes from ChaCha20 block {:#x} run past its 32-bit block counter",
                data.len(),
                nonce[0]
            )));
        }
        let mut iv = [0u8; 12];
        for (i, w) in nonce[1..].iter().enumerate() {
            iv[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        let mut c = chacha20::ChaCha20::new((&self.0).into(), (&iv).into());
        let out_of_range = |_| Error::Unsupported("ChaCha20 keystream out of range".into());
        c.try_seek(u64::from(nonce[0]) * 64).map_err(out_of_range)?;

        // RustCrypto's slice wrapper reserves block u32::MAX so its cursor
        // cannot wrap. Our checked range may include that final block: use
        // the wrapper for the prefix, then generate exactly one core block.
        let final_bytes = if end == 1 << 32 {
            (data.len() - 1) % 64 + 1
        } else {
            0
        };
        let prefix_len = data.len() - final_bytes;
        let (prefix, tail) = data.split_at_mut(prefix_len);
        c.try_apply_keystream(prefix).map_err(out_of_range)?;
        if !tail.is_empty() {
            let mut core = chacha20::ChaChaCore::<U10>::new((&self.0).into(), (&iv).into());
            core.set_block_pos(u32::MAX);
            let mut block = Default::default();
            core.write_keystream_block(&mut block);
            for (byte, key) in tail.iter_mut().zip(block) {
                *byte ^= key;
            }
            // Drop the core here; its wrapped counter must never be used.
        }
        Ok(())
    }

    /// The Poly1305 key for `nonce`: the first 32 keystream bytes of
    /// `nonce` with [`NONCE_POLY`] XORed into word 3.
    fn poly_key(&self, nonce: Nonce) -> Result<[u8; 32]> {
        let mut k = [0u8; 32];
        self.apply(
            [nonce[0], nonce[1], nonce[2], nonce[3] ^ NONCE_POLY],
            &mut k,
        )?;
        Ok(k)
    }

    /// The Poly1305 MAC (RFC 8439 2.5) of `data` under `nonce`.
    pub fn mac(&self, nonce: Nonce, data: &[u8]) -> Result<[u8; 16]> {
        Ok(poly1305::Poly1305::new((&self.poly_key(nonce)?).into())
            .compute_unpadded(data)
            .into())
    }
}

/// The nonce of the btree node record at byte `offset` of a node whose
/// sequence number is `seq`, and whose bset carries `journal_seq`.
/// OBSERVED on all 192 records of `encrypted.img`'s 33 nodes, whose
/// journal sequence numbers run from 0 to 0x2b: how one of 2^28 or more
/// would share word 3 with the domain separator has not been seen, so such
/// a record is refused rather than given a guessed nonce.
pub fn btree_node_nonce(offset: usize, seq: u64, journal_seq: u64) -> Result<Nonce> {
    if journal_seq >= 1 << 28 {
        return Err(Error::Unsupported(format!(
            "an encrypted bset with journal sequence number {journal_seq:#x}: how one this \
             large enters its nonce has not been observed"
        )));
    }
    let offset = u32::try_from(offset)
        .map_err(|_| Error::Corrupt(format!("a btree node record at byte {offset}")))?;
    Ok([
        offset,
        seq as u32,
        (seq >> 32) as u32,
        NONCE_BTREE_NODE ^ journal_seq as u32,
    ])
}

/// The nonce of a data extent whose crc entry is `crc`, in a key whose
/// version is `version_hi:version_lo`. OBSERVED on all 441 extents of
/// `encrypted.img`: the version (1 to 0x1b9, its high 64 bits zero) is word
/// 1, and the uncompressed size, which runs from 1 to 64 sectors there,
/// does not enter it. OBSERVED on all 34 incompressible extents of
/// `encrypted-compressed.img` (#356): the incompressible type does not
/// enter it either. OBSERVED on the incompressible extent of
/// `encrypted-compressed.img` whose crc entry the reference narrowed to the
/// 43 live sectors of a partly overwritten one (#331, nonce offset 21): the
/// offset, in sectors, advances word 0, the block counter, by eight 64-byte
/// blocks a sector, so the keystream runs from where those sectors sat in
/// the original extent. What was not seen is refused rather than given a
/// guessed nonce: a compressed extent (S1 puts the compression type in the
/// nonce, #330), and a version of 2^32 or more (whether its high bits are
/// word 2 or share word 3).
pub fn extent_nonce(crc: &Crc, version_hi: u32, version_lo: u64) -> Result<Nonce> {
    if crc.compression_type != compression::NONE
        && crc.compression_type != compression::INCOMPRESSIBLE
    {
        return Err(Error::Unsupported(format!(
            "an encrypted extent with compression type {}: how a compressed extent's nonce is \
             made has not been observed",
            crc.compression_type
        )));
    }
    let block = crc.nonce.checked_mul(512 / 64).ok_or_else(|| {
        Error::Corrupt(format!(
            "an encrypted extent's nonce offset of {} sectors is past the block counter",
            crc.nonce
        ))
    })?;
    let version = u32::try_from(version_lo)
        .ok()
        .filter(|_| version_hi == 0)
        .ok_or_else(|| {
            Error::Unsupported(format!(
                "an encrypted extent with version {version_hi:#x}:{version_lo:#x}: how a version \
                 of 2^32 or more enters its nonce has not been observed"
            ))
        })?;
    Ok([block, version, 0, NONCE_EXTENT])
}

/// Verify the MAC of an encrypted extent's stored bytes `data`, then
/// decrypt them in place; `crc` is its crc entry and `version_hi:version_lo`
/// its key's version. Nothing is decrypted unless the MAC verifies: a
/// mismatch is refused as a bad checksum naming the MAC. The MAC is the
/// default 80 bits or, under `wide_macs`, all 128; any other checksum type
/// has not been observed in an encrypted data extent, and is refused.
pub fn decrypt_extent(
    key: &MasterKey,
    crc: &Crc,
    version_hi: u32,
    version_lo: u64,
    data: &mut [u8],
) -> Result<()> {
    let (len, hi_fits) = match crc.csum_type {
        CSUM_CHACHA20_POLY1305_80 => (EXTENT_MAC_BYTES, crc.csum_hi <= u64::from(u16::MAX)),
        CSUM_CHACHA20_POLY1305_128 => (WIDE_EXTENT_MAC_BYTES, true),
        t => {
            return Err(Error::Unsupported(format!(
                "an encrypted extent with checksum type {t}: only the 80-bit and 128-bit MACs \
                 (types {CSUM_CHACHA20_POLY1305_80} and {CSUM_CHACHA20_POLY1305_128}) have \
                 been observed"
            )))
        }
    };
    let nonce = extent_nonce(crc, version_hi, version_lo)?;
    let mac = key.mac(nonce, data)?;
    let mut stored = [0u8; WIDE_EXTENT_MAC_BYTES];
    stored[..8].copy_from_slice(&crc.csum_lo.to_le_bytes());
    stored[8..].copy_from_slice(&crc.csum_hi.to_le_bytes());
    if !hi_fits || mac[..len] != stored[..len] {
        return Err(Error::BadChecksum {
            what: "extent data MAC",
            stored: crc.csum_lo,
            computed: u64::from_le_bytes(mac[..8].try_into().expect("8 bytes")),
        });
    }
    key.apply(nonce, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unsealed() -> Vec<u8> {
        let mut body = vec![0u8; FIELD_BYTES];
        body[16..24].copy_from_slice(&KEY_MAGIC);
        body[24..].copy_from_slice(&[0x5a; 32]);
        body
    }

    #[test]
    fn a_key_whose_magic_reads_in_the_clear_is_unsealed() {
        let c = CryptField::parse(&unsealed()).unwrap();
        assert!(!c.is_sealed());
        assert_eq!(c.head, [0; 16]);
        assert_eq!(c.key[8..], [0x5a; 32]);
    }

    #[test]
    fn a_key_whose_magic_does_not_read_is_sealed() {
        for at in 16..24 {
            let mut body = unsealed();
            body[at] ^= 1;
            assert!(CryptField::parse(&body).unwrap().is_sealed(), "byte {at}");
        }
    }

    #[test]
    fn a_field_of_another_length_is_refused() {
        let body = unsealed();
        assert!(matches!(
            CryptField::parse(&body[..FIELD_BYTES - 8]),
            Err(Error::Corrupt(_))
        ));
        let mut longer = body.clone();
        longer.extend_from_slice(&[0; 8]);
        assert!(matches!(
            CryptField::parse(&longer),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn debug_output_never_shows_the_key() {
        let shown = format!("{:?}", CryptField::parse(&unsealed()).unwrap());
        assert!(shown.contains("sealed: false"), "{shown}");
        assert!(
            !shown.contains("90"),
            "the key's bytes (0x5a = 90): {shown}"
        );
        let key = CryptField::parse(&unsealed())
            .unwrap()
            .master_key()
            .unwrap();
        assert_eq!(format!("{key:?}"), "MasterKey(..)");
    }

    #[test]
    fn only_an_unsealed_field_gives_its_master_key() {
        let key = CryptField::parse(&unsealed())
            .unwrap()
            .master_key()
            .unwrap();
        assert_eq!(key.0, [0x5a; 32]);
        let mut body = unsealed();
        body[16] ^= 1;
        assert!(CryptField::parse(&body).unwrap().master_key().is_none());
    }

    /// RFC 8439 2.4.2's test vector: key 00..1f, nonce
    /// 000000000000004a00000000, block counter 1.
    #[test]
    fn the_keystream_is_rfc_8439_chacha20() {
        let mut key = [0u8; 32];
        for (i, b) in key.iter_mut().enumerate() {
            *b = i as u8;
        }
        let mut data = *b"Ladies and Gentlemen of the class of '99: If I could offer you o";
        MasterKey(key)
            .apply([1, 0, 0x4a00_0000, 0], &mut data)
            .unwrap();
        assert_eq!(
            data[..16],
            [
                0x6e, 0x2e, 0x35, 0x9a, 0x25, 0x68, 0xf9, 0x80, 0x41, 0xba, 0x07, 0x28, 0xdd, 0x0d,
                0x69, 0x81
            ]
        );
    }

    /// RFC 8439 2.6.2's vector: key 80..9f, nonce 000000000001020304050607,
    /// block counter 0. `poly_key` XORs [`NONCE_POLY`] into word 3, so the
    /// vector's word 3 is given with that bit already flipped.
    #[test]
    fn the_poly1305_key_is_rfc_8439_s_one_time_key() {
        let mut key = [0u8; 32];
        for (i, b) in key.iter_mut().enumerate() {
            *b = 0x80 + i as u8;
        }
        let k = MasterKey(key)
            .poly_key([0, 0, 0x0302_0100, 0x0706_0504 ^ NONCE_POLY])
            .unwrap();
        assert_eq!(
            k,
            [
                0x8a, 0xd5, 0xa0, 0x8b, 0x90, 0x5f, 0x81, 0xcc, 0x81, 0x50, 0x40, 0x27, 0x4a, 0xb2,
                0x94, 0x71, 0xa8, 0x33, 0xb6, 0x37, 0xe3, 0xfd, 0x0d, 0xa5, 0x08, 0xdb, 0xb8, 0xe2,
                0xfd, 0xd1, 0xa6, 0x46
            ]
        );
    }

    #[test]
    fn a_keystream_past_the_block_counter_is_refused() {
        let key = MasterKey([0; 32]);
        let mut one_block = [0u8; 64];
        assert!(key.apply([u32::MAX, 0, 0, 0], &mut one_block).is_ok());
        let mut two_blocks = [0u8; 65];
        assert!(matches!(
            key.apply([u32::MAX, 0, 0, 0], &mut two_blocks),
            Err(Error::Unsupported(_))
        ));

        // OpenSSL 3.6.5 `enc -chacha20`, a zero key, IV
        // feffffff4433221188776655ccbbaa99, and 128 zero input bytes.
        // These are the final two blocks, with every nonce word nonzero.
        let expected = [
            0xfc, 0x67, 0x84, 0xce, 0xb6, 0x7d, 0x7c, 0xc1, 0xdc, 0x44, 0xa5, 0xd4, 0x96, 0x91,
            0x5e, 0x67, 0x86, 0x68, 0x9d, 0x1f, 0x7c, 0xe9, 0xf1, 0x03, 0xac, 0x1e, 0xdb, 0x5a,
            0xdb, 0xcb, 0x3d, 0x50, 0xab, 0x5a, 0xd1, 0x91, 0x95, 0xc7, 0x89, 0x6e, 0x51, 0xa3,
            0x3a, 0x8b, 0x0f, 0x89, 0x14, 0x15, 0x90, 0xd4, 0xf2, 0x46, 0x5f, 0x06, 0x0f, 0xd3,
            0xe2, 0x88, 0x99, 0xcb, 0x6f, 0x0d, 0xf7, 0x58, 0xa1, 0xcb, 0x63, 0xac, 0xd1, 0x6d,
            0x27, 0xe8, 0x3a, 0x50, 0xe4, 0x40, 0x73, 0xea, 0x2e, 0xed, 0x1b, 0xa2, 0x76, 0x9b,
            0x61, 0x03, 0xc0, 0x35, 0x60, 0x07, 0xcd, 0x7a, 0xa4, 0x9e, 0x51, 0x5d, 0x89, 0x87,
            0x0f, 0xda, 0x64, 0x27, 0x91, 0x82, 0x1b, 0x56, 0x45, 0x92, 0x6b, 0x57, 0x66, 0x15,
            0x5a, 0x85, 0xdc, 0xaa, 0x7e, 0xfb, 0x31, 0xe6, 0x3d, 0xaf, 0x6a, 0x15, 0x2d, 0xdb,
            0x2c, 0x8a,
        ];
        for (counter, at, lengths) in [
            (u32::MAX - 1, 0, &[0, 1, 63, 64, 65, 127, 128][..]),
            (u32::MAX, 64, &[0, 1, 32, 63, 64][..]),
        ] {
            let nonce = [counter, 0x1122_3344, 0x5566_7788, 0x99aa_bbcc];
            for &len in lengths {
                let mut data = vec![0x5a; len];
                key.apply(nonce, &mut data).unwrap();
                let want: Vec<_> = expected[at..at + len].iter().map(|b| b ^ 0x5a).collect();
                assert_eq!(data, want, "counter {counter:#x}, length {len}");
            }
            let mut too_long = vec![0x5a; 129 - at];
            assert!(matches!(
                key.apply(nonce, &mut too_long),
                Err(Error::Unsupported(_))
            ));
            assert!(too_long.iter().all(|&b| b == 0x5a));
        }
    }

    #[test]
    fn a_node_nonce_is_its_offset_seq_and_journal_seq() {
        assert_eq!(
            btree_node_nonce(20480, 0x0e36_ac1b_434a_a5cc, 0x23).unwrap(),
            [20480, 0x434a_a5cc, 0x0e36_ac1b, 0x2000_0023]
        );
        assert!(btree_node_nonce(0, 1, (1 << 28) - 1).is_ok());
        assert!(matches!(
            btree_node_nonce(0, 1, 1 << 28),
            Err(Error::Unsupported(_))
        ));
    }

    /// The master key of `encrypted.img`, from its crypt field (the fixtures
    /// artifact of CI run 39021762764).
    fn encrypted_img_key() -> MasterKey {
        MasterKey([
            0x4b, 0xb0, 0x49, 0x86, 0xf2, 0x3a, 0x34, 0x30, 0x7c, 0x1f, 0xf3, 0x81, 0xd3, 0x34,
            0x4a, 0x8a, 0x6a, 0xe1, 0xcf, 0x6c, 0xc2, 0x89, 0xfc, 0x8e, 0xf8, 0x4b, 0xb4, 0x10,
            0x70, 0x88, 0x9c, 0xc3,
        ])
    }

    /// `hello.txt`'s one extent in `encrypted.img`: its stored sector, and
    /// the crc entry before its pointer (`0x3ede_0300_0000_0004`, then
    /// `0x655e_30e7_6f31_fa5a`) in a key of version 0x1b8. The file is
    /// "hello world\n"; the rest of the sector is zeros.
    const HELLO_SECTOR: &[u8; 512] =
        include_bytes!("../tests/data/encrypted-hello-extent-39021762764.bin");
    const HELLO_VERSION: u64 = 0x1b8;

    fn hello_crc() -> Crc {
        let v = [0x3ede_0300_0000_0004u64, 0x655e_30e7_6f31_fa5a];
        let v: Vec<u8> = v.iter().flat_map(|w| w.to_le_bytes()).collect();
        match crate::extent::parse_entries(&v).unwrap()[..] {
            [crate::extent::ExtentEntry::Crc(c)] => c,
            ref e => panic!("{e:?}"),
        }
    }

    #[test]
    fn a_real_extent_is_verified_and_decrypted() {
        let crc = hello_crc();
        assert_eq!(
            (crc.csum_type, crc.compression_type, crc.nonce, crc.csum_hi),
            (CSUM_CHACHA20_POLY1305_80, compression::NONE, 0, 0x3ede)
        );
        let mut data = *HELLO_SECTOR;
        decrypt_extent(&encrypted_img_key(), &crc, 0, HELLO_VERSION, &mut data).unwrap();
        assert_eq!(&data[..12], b"hello world\n");
        assert!(data[12..].iter().all(|&b| b == 0));
    }

    /// One bit changed anywhere -- the data, either half of the stored MAC,
    /// the version, the key -- and the extent is refused before anything is
    /// decrypted.
    #[test]
    fn an_extent_whose_mac_does_not_verify_is_refused() {
        let key = encrypted_img_key();
        let refused = |crc: &Crc, version: u64, key: &MasterKey, mut data: [u8; 512]| {
            let before = data;
            match decrypt_extent(key, crc, 0, version, &mut data) {
                Err(Error::BadChecksum { what, .. }) => assert_eq!(what, "extent data MAC"),
                other => panic!("{other:?}"),
            }
            assert_eq!(data, before, "decrypted although refused");
        };
        for at in [0, 11, 511] {
            let mut data = *HELLO_SECTOR;
            data[at] ^= 1;
            refused(&hello_crc(), HELLO_VERSION, &key, data);
        }
        let mut crc = hello_crc();
        crc.csum_lo ^= 1 << 63;
        refused(&crc, HELLO_VERSION, &key, *HELLO_SECTOR);
        let mut crc = hello_crc();
        crc.csum_hi ^= 1;
        refused(&crc, HELLO_VERSION, &key, *HELLO_SECTOR);
        refused(&hello_crc(), HELLO_VERSION + 1, &key, *HELLO_SECTOR);
        refused(
            &hello_crc(),
            HELLO_VERSION,
            &MasterKey([0x5a; 32]),
            *HELLO_SECTOR,
        );
    }

    /// `hello.txt`'s extent as `wide_macs` would store it: checksum type 4,
    /// the whole 128-bit MAC with its first 8 bytes in the low checksum
    /// word and the next 8 in the high one. The MAC's first 80 bits are the
    /// ones `encrypted.img` stores, so the tag is the reference's own.
    fn hello_wide_crc() -> Crc {
        let key = encrypted_img_key();
        let nonce = extent_nonce(&hello_crc(), 0, HELLO_VERSION).unwrap();
        let mac = key.mac(nonce, HELLO_SECTOR).unwrap();
        let narrow = hello_crc();
        assert_eq!(mac[..8], narrow.csum_lo.to_le_bytes());
        assert_eq!(mac[8..10], (narrow.csum_hi as u16).to_le_bytes());
        Crc {
            csum_type: CSUM_CHACHA20_POLY1305_128,
            csum_lo: u64::from_le_bytes(mac[..8].try_into().unwrap()),
            csum_hi: u64::from_le_bytes(mac[8..].try_into().unwrap()),
            ..narrow
        }
    }

    /// A 128-bit MAC verifies in full: the extent decrypts with it, and one
    /// bit changed in either half -- including the 48 bits the 80-bit MAC
    /// does not keep -- is refused before anything is decrypted.
    #[test]
    fn a_128_bit_mac_is_verified_in_full() {
        let key = encrypted_img_key();
        let mut data = *HELLO_SECTOR;
        decrypt_extent(&key, &hello_wide_crc(), 0, HELLO_VERSION, &mut data).unwrap();
        assert_eq!(&data[..12], b"hello world\n");
        assert!(data[12..].iter().all(|&b| b == 0));

        let flips: [(u32, bool); 6] = [
            (0, false),
            (63, false),
            (0, true),
            (15, true),
            (16, true),
            (63, true),
        ];
        for (bit, high) in flips {
            let mut crc = hello_wide_crc();
            if high {
                crc.csum_hi ^= 1 << bit;
            } else {
                crc.csum_lo ^= 1 << bit;
            }
            let mut data = *HELLO_SECTOR;
            match decrypt_extent(&key, &crc, 0, HELLO_VERSION, &mut data) {
                Err(Error::BadChecksum { what, .. }) => assert_eq!(what, "extent data MAC"),
                other => panic!(
                    "bit {bit} of the {} word: {other:?}",
                    if high { "high" } else { "low" }
                ),
            }
            assert_eq!(data, *HELLO_SECTOR, "decrypted although refused");
        }
    }

    #[test]
    fn an_extent_nonce_is_its_version_under_the_extent_domain() {
        let crc = hello_crc();
        assert_eq!(
            extent_nonce(&crc, 0, 0x1b8).unwrap(),
            [0, 0x1b8, 0, 0x1000_0000]
        );
        assert_eq!(
            extent_nonce(&crc, 0, u64::from(u32::MAX)).unwrap()[1],
            u32::MAX
        );
    }

    /// A narrowed extent's nonce offset (#331) starts its keystream that
    /// many sectors in: offset 21 is block 168, the placement under which
    /// the reference's MAC of the 43-sector extent verified.
    #[test]
    fn a_nonce_offset_advances_the_block_counter_a_sector_at_a_time() {
        let crc = Crc {
            nonce: 21,
            ..hello_crc()
        };
        assert_eq!(
            extent_nonce(&crc, 0, 10).unwrap(),
            [168, 10, 0, 0x1000_0000]
        );
        let past = Crc {
            nonce: u32::MAX / 4,
            ..hello_crc()
        };
        assert!(matches!(extent_nonce(&past, 0, 10), Err(Error::Corrupt(_))));
    }

    /// What has not been observed is refused, not given a guessed nonce:
    /// compression, a version of 2^32 or more, and a MAC of another width
    /// than 80 or 128 bits (type 4, `wide_macs`, now reads:
    /// `a_128_bit_mac_is_verified_in_full`).
    #[test]
    fn an_extent_not_yet_observed_is_refused() {
        let unsupported = |crc: Crc, version_hi: u32, version_lo: u64| {
            let mut data = *HELLO_SECTOR;
            match decrypt_extent(
                &encrypted_img_key(),
                &crc,
                version_hi,
                version_lo,
                &mut data,
            ) {
                Err(Error::Unsupported(m)) => m,
                other => panic!("{other:?}"),
            }
        };
        for t in [compression::GZIP, compression::LZ4, compression::ZSTD] {
            let m = unsupported(
                Crc {
                    compression_type: t,
                    ..hello_crc()
                },
                0,
                HELLO_VERSION,
            );
            assert!(m.contains("compression type"), "{m}");
        }
        // Incompressible takes the uncompressed nonce (#356).
        let incompressible = Crc {
            compression_type: compression::INCOMPRESSIBLE,
            ..hello_crc()
        };
        assert_eq!(
            extent_nonce(&incompressible, 0, HELLO_VERSION).unwrap(),
            extent_nonce(&hello_crc(), 0, HELLO_VERSION).unwrap()
        );
        for (hi, lo) in [(1, HELLO_VERSION), (0, 1 << 32)] {
            let m = unsupported(hello_crc(), hi, lo);
            assert!(m.contains("version"), "{m}");
        }
        for t in [0, 5] {
            let m = unsupported(
                Crc {
                    csum_type: t,
                    ..hello_crc()
                },
                0,
                HELLO_VERSION,
            );
            assert!(m.contains(&format!("checksum type {t}")), "{m}");
        }
    }
}
