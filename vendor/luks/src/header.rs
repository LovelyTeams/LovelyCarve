//! LUKS1 partition header (`phdr`) and keyslot parsing.
//!
//! The LUKS1 on-disk header is 592 bytes, all integers big-endian, per the
//! *LUKS On-Disk Format Specification* (version 1.2.3). Layout:
//!
//! ```text
//!   0  magic[6] = "LUKS\xba\xbe"      104  payload-offset  u32 (sectors)
//!   6  version  u16                    108  key-bytes       u32
//!   8  cipher-name[32]                 112  mk-digest[20]
//!  40  cipher-mode[32]                 132  mk-digest-salt[32]
//!  72  hash-spec[32]                   164  mk-digest-iter  u32
//!                                       168  uuid[40]
//!  208  8 x keyslot (48 bytes each)
//! ```

use crate::bytes::{be_u16, be_u32, bytes_n, cstr};
use crate::error::{LuksError, Result};

/// The LUKS1 magic signature.
pub const LUKS_MAGIC: [u8; 6] = [b'L', b'U', b'K', b'S', 0xba, 0xbe];
/// Total size of the LUKS1 header in bytes.
pub const LUKS1_PHDR_LEN: usize = 592;
/// Number of keyslots in a LUKS1 header.
pub const LUKS_NUM_KEYS: usize = 8;
/// Size of each keyslot record in bytes.
pub const KEYSLOT_LEN: usize = 48;
/// Size of the master-key digest (always 20 bytes in LUKS1).
pub const MK_DIGEST_LEN: usize = 20;
/// Size of a salt field.
pub const SALT_LEN: usize = 32;
/// Keyslot `active` marker: enabled.
pub const KEY_ENABLED: u32 = 0x00AC_71F3;
/// Keyslot `active` marker: disabled.
pub const KEY_DISABLED: u32 = 0x0000_DEAD;
/// LUKS sector size (bytes).
pub const SECTOR: u64 = 512;

/// One LUKS1 keyslot descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keyslot {
    /// `active` marker (`KEY_ENABLED` / `KEY_DISABLED`).
    pub active: u32,
    /// PBKDF2 iteration count for this slot.
    pub iterations: u32,
    /// PBKDF2 salt (32 bytes).
    pub salt: [u8; SALT_LEN],
    /// Byte offset of the anti-forensic key material = `key_material_offset * 512`.
    pub key_material_offset: u32,
    /// Number of anti-forensic stripes (typically 4000).
    pub stripes: u32,
}

impl Keyslot {
    /// Whether this keyslot is active (holds usable key material).
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active == KEY_ENABLED
    }

    /// Whether this keyslot is explicitly disabled (marker `KEY_DISABLED`). A slot
    /// that is neither enabled nor disabled carries a corrupt/unknown marker.
    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.active == KEY_DISABLED
    }
}

/// A parsed LUKS1 partition header.
#[derive(Debug, Clone)]
pub struct Luks1Header {
    /// Header version (always 1 here).
    pub version: u16,
    /// Cipher name, e.g. `aes`.
    pub cipher_name: String,
    /// Cipher mode, e.g. `xts-plain64`.
    pub cipher_mode: String,
    /// Hash spec for PBKDF2 and AF, e.g. `sha256`.
    pub hash_spec: String,
    /// Payload start, in 512-byte sectors.
    pub payload_offset: u32,
    /// Master-key length in bytes (e.g. 64 for AES-256-XTS).
    pub key_bytes: u32,
    /// Master-key digest (PBKDF2 of the master key).
    pub mk_digest: [u8; MK_DIGEST_LEN],
    /// Salt for the master-key digest.
    pub mk_digest_salt: [u8; SALT_LEN],
    /// Iteration count for the master-key digest.
    pub mk_digest_iter: u32,
    /// Volume UUID.
    pub uuid: String,
    /// The eight keyslots.
    pub keyslots: [Keyslot; LUKS_NUM_KEYS],
}

impl Luks1Header {
    /// Parse a LUKS1 header from the first `LUKS1_PHDR_LEN` bytes of `data`.
    ///
    /// # Errors
    /// [`LuksError::NotLuks`] if the magic is absent, [`LuksError::UnsupportedVersion`]
    /// if the version is not 1, [`LuksError::MalformedHeader`] if the buffer is short.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let magic = bytes_n::<6>(data, 0);
        if magic != LUKS_MAGIC {
            return Err(LuksError::NotLuks { found: magic });
        }
        if data.len() < LUKS1_PHDR_LEN {
            return Err(LuksError::MalformedHeader {
                what: "phdr",
                need: LUKS1_PHDR_LEN,
                got: data.len(),
            });
        }
        let version = be_u16(data, 6);
        if version != 1 {
            return Err(LuksError::UnsupportedVersion { version });
        }

        let keyslots = std::array::from_fn(|i| {
            let base = 208 + i * KEYSLOT_LEN;
            Keyslot {
                active: be_u32(data, base),
                iterations: be_u32(data, base + 4),
                salt: bytes_n::<SALT_LEN>(data, base + 8),
                key_material_offset: be_u32(data, base + 40),
                stripes: be_u32(data, base + 44),
            }
        });

        Ok(Luks1Header {
            version,
            cipher_name: cstr(data, 8, 32),
            cipher_mode: cstr(data, 40, 32),
            hash_spec: cstr(data, 72, 32),
            payload_offset: be_u32(data, 104),
            key_bytes: be_u32(data, 108),
            mk_digest: bytes_n::<MK_DIGEST_LEN>(data, 112),
            mk_digest_salt: bytes_n::<SALT_LEN>(data, 132),
            mk_digest_iter: be_u32(data, 164),
            uuid: cstr(data, 168, 40),
            keyslots,
        })
    }

    /// The active keyslots, in slot order.
    pub fn active_keyslots(&self) -> impl Iterator<Item = &Keyslot> {
        self.keyslots.iter().filter(|k| k.is_active())
    }

    /// Payload byte offset (`payload_offset * 512`).
    #[must_use]
    pub fn payload_byte_offset(&self) -> u64 {
        u64::from(self.payload_offset) * SECTOR
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic LUKS1 header with the given fields (one active keyslot 0).
    fn build_header() -> Vec<u8> {
        let mut h = vec![0u8; LUKS1_PHDR_LEN];
        h[0..6].copy_from_slice(&LUKS_MAGIC);
        h[6..8].copy_from_slice(&1u16.to_be_bytes());
        h[8..11].copy_from_slice(b"aes");
        h[40..51].copy_from_slice(b"xts-plain64");
        h[72..78].copy_from_slice(b"sha256");
        h[104..108].copy_from_slice(&4096u32.to_be_bytes()); // payload offset
        h[108..112].copy_from_slice(&64u32.to_be_bytes()); // key bytes (AES-256-XTS)
        h[112..132].copy_from_slice(&[0xAB; 20]); // mk-digest
        h[132..164].copy_from_slice(&[0xCD; 32]); // mk-digest salt
        h[164..168].copy_from_slice(&1000u32.to_be_bytes()); // mk-digest iter
        h[168..204].copy_from_slice(b"b22690e1-a392-4ecc-83b1-c1cf21200116");
        // keyslot 0: active, iterations, salt, key-material offset (sector 8), stripes 4000
        let b = 208;
        h[b..b + 4].copy_from_slice(&KEY_ENABLED.to_be_bytes());
        h[b + 4..b + 8].copy_from_slice(&50000u32.to_be_bytes());
        h[b + 8..b + 40].copy_from_slice(&[0x11; 32]);
        h[b + 40..b + 44].copy_from_slice(&8u32.to_be_bytes());
        h[b + 44..b + 48].copy_from_slice(&4000u32.to_be_bytes());
        // keyslot 1: disabled
        let b1 = 208 + KEYSLOT_LEN;
        h[b1..b1 + 4].copy_from_slice(&KEY_DISABLED.to_be_bytes());
        h
    }

    #[test]
    fn parses_luks1_header_fields() {
        let h = Luks1Header::parse(&build_header()).unwrap();
        assert_eq!(h.version, 1);
        assert_eq!(h.cipher_name, "aes");
        assert_eq!(h.cipher_mode, "xts-plain64");
        assert_eq!(h.hash_spec, "sha256");
        assert_eq!(h.payload_offset, 4096);
        assert_eq!(h.payload_byte_offset(), 4096 * 512);
        assert_eq!(h.key_bytes, 64);
        assert_eq!(h.mk_digest, [0xAB; 20]);
        assert_eq!(h.mk_digest_salt, [0xCD; 32]);
        assert_eq!(h.mk_digest_iter, 1000);
        assert_eq!(h.uuid, "b22690e1-a392-4ecc-83b1-c1cf21200116");
    }

    #[test]
    fn parses_keyslots_and_active_filter() {
        let h = Luks1Header::parse(&build_header()).unwrap();
        assert!(h.keyslots[0].is_active());
        assert_eq!(h.keyslots[0].iterations, 50000);
        assert_eq!(h.keyslots[0].salt, [0x11; 32]);
        assert_eq!(h.keyslots[0].key_material_offset, 8);
        assert_eq!(h.keyslots[0].stripes, 4000);
        assert!(!h.keyslots[1].is_active());
        assert!(h.keyslots[1].is_disabled());
        assert_eq!(h.active_keyslots().count(), 1);
    }

    #[test]
    fn rejects_non_luks() {
        let err = Luks1Header::parse(&[0u8; LUKS1_PHDR_LEN]).unwrap_err();
        assert!(matches!(err, LuksError::NotLuks { .. }));
    }

    #[test]
    fn rejects_bad_version() {
        let mut h = build_header();
        h[6..8].copy_from_slice(&9u16.to_be_bytes());
        assert!(matches!(
            Luks1Header::parse(&h).unwrap_err(),
            LuksError::UnsupportedVersion { version: 9 }
        ));
    }

    #[test]
    fn rejects_truncated() {
        let mut h = build_header();
        h.truncate(100);
        assert!(matches!(
            Luks1Header::parse(&h).unwrap_err(),
            LuksError::MalformedHeader { .. }
        ));
    }
}
