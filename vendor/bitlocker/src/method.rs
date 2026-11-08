//! BitLocker encryption-method classification.
//!
//! The 16-bit encryption method (FVE metadata header offset 36) selects the
//! sector transform along three orthogonal axes — key size, cipher mode, and
//! whether the Elephant Diffuser is applied. This module decodes the raw value
//! into those axes so the unlock dispatch branches on the *axes*, not on
//! per-method literals: a new cipher drops in as one `validated_kind` arm plus a
//! builder and an oracle test.
//!
//! The 0x800x enumeration is a documented, discrete domain (libbde
//! `libbde_definitions.h` / dfvfs): bit 0 is the key size (0 ⇒ 128, 1 ⇒ 256),
//! and the pair index selects mode + diffuser.
//!
//! | Method | Key | Mode | Diffuser | Validated (oracle) |
//! |---|---|---|---|---|
//! | `0x8000` | 128 | CBC | yes | ✅ dfvfs `bdetogo.raw` |
//! | `0x8001` | 256 | CBC | yes | independent Python vectors |
//! | `0x8002` | 128 | CBC | no  | ✅ picoCTF `bitlocker-1.dd` |
//! | `0x8003` | 256 | CBC | no  | — |
//! | `0x8004` | 128 | XTS | no  | — |
//! | `0x8005` | 256 | XTS | no  | — |

/// The AES cipher mode a BitLocker volume uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherMode {
    /// AES-CBC with a per-sector ECB-derived IV (methods 0x8000–0x8003).
    Cbc,
    /// AES-XTS (methods 0x8004/0x8005).
    Xts,
}

/// A BitLocker encryption method decoded into its three axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncryptionMethod {
    /// The raw 16-bit method value (kept for diagnostics).
    pub raw: u16,
    /// Key size in bits (128 or 256).
    pub key_bits: u16,
    /// The cipher mode.
    pub mode: CipherMode,
    /// Whether the Elephant Diffuser is applied after the CBC stage.
    pub diffuser: bool,
}

/// A sector-cipher kind this build has an oracle for and can actually decrypt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectorCipherKind {
    /// AES-128-CBC + Elephant Diffuser (method 0x8000).
    Cbc128Diffuser,
    /// AES-256-CBC + Elephant Diffuser (0x8001).
    Cbc256Diffuser,
    /// AES-128-CBC, no diffuser (method 0x8002).
    Cbc128,
    /// AES-256-CBC, no diffuser (method 0x8003).
    Cbc256,
    /// XTS-AES-128 (method 0x8004).
    Xts128,
    /// XTS-AES-256 (method 0x8005).
    Xts256,
}

impl EncryptionMethod {
    /// Decode a raw encryption-method value, or `None` if it is not one of the
    /// six defined `0x8000`–`0x8005` BitLocker ciphers.
    #[must_use]
    pub fn decode(raw: u16) -> Option<EncryptionMethod> {
        // Only 0x8000..=0x8005 are defined ciphers; 0x8006/0x8007 and everything
        // outside the range are unrecognized.
        let index = raw.checked_sub(0x8000)?;
        if index > 5 {
            return None;
        }
        let key_bits = if index & 1 == 1 { 256 } else { 128 };
        // Pair index selects mode + diffuser: 0 ⇒ CBC+diffuser, 1 ⇒ CBC,
        // 2 ⇒ XTS. `index <= 5` ⇒ `index >> 1 <= 2`, so the wildcard is a
        // panic-free guard that no in-range value can reach.
        let (mode, diffuser) = match index >> 1 {
            0 => (CipherMode::Cbc, true),
            1 => (CipherMode::Cbc, false),
            _ => (CipherMode::Xts, false),
        };
        Some(EncryptionMethod {
            raw,
            key_bits,
            mode,
            diffuser,
        })
    }

    /// The validated sector-cipher kind for this method, or `None` if the method
    /// is not supported by the tested transform implementation.
    #[must_use]
    pub fn validated_kind(self) -> Option<SectorCipherKind> {
        match (self.mode, self.key_bits, self.diffuser) {
            (CipherMode::Cbc, 256, true) => Some(SectorCipherKind::Cbc256Diffuser),
            (CipherMode::Cbc, 128, true) => Some(SectorCipherKind::Cbc128Diffuser),
            (CipherMode::Cbc, 128, false) => Some(SectorCipherKind::Cbc128),
            (CipherMode::Cbc, 256, false) => Some(SectorCipherKind::Cbc256),
            (CipherMode::Xts, 128, false) => Some(SectorCipherKind::Xts128),
            (CipherMode::Xts, 256, false) => Some(SectorCipherKind::Xts256),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_all_six_axes() {
        let cases = [
            (0x8000u16, 128, CipherMode::Cbc, true),
            (0x8001, 256, CipherMode::Cbc, true),
            (0x8002, 128, CipherMode::Cbc, false),
            (0x8003, 256, CipherMode::Cbc, false),
            (0x8004, 128, CipherMode::Xts, false),
            (0x8005, 256, CipherMode::Xts, false),
        ];
        for (raw, key_bits, mode, diffuser) in cases {
            let m = EncryptionMethod::decode(raw).unwrap();
            assert_eq!(m.raw, raw);
            assert_eq!(m.key_bits, key_bits);
            assert_eq!(m.mode, mode);
            assert_eq!(m.diffuser, diffuser);
        }
    }

    #[test]
    fn unrecognized_methods_decode_none() {
        for raw in [0x0000u16, 0x7fff, 0x8006, 0x8007, 0x8010, 0x1234, 0xffff] {
            assert!(EncryptionMethod::decode(raw).is_none(), "{raw:#06x}");
        }
    }

    #[test]
    fn validated_kinds_match_oracle_backed_methods() {
        assert_eq!(
            EncryptionMethod::decode(0x8000).unwrap().validated_kind(),
            Some(SectorCipherKind::Cbc128Diffuser)
        );
        assert_eq!(
            EncryptionMethod::decode(0x8002).unwrap().validated_kind(),
            Some(SectorCipherKind::Cbc128)
        );
        assert_eq!(
            EncryptionMethod::decode(0x8003).unwrap().validated_kind(),
            Some(SectorCipherKind::Cbc256)
        );
        assert_eq!(
            EncryptionMethod::decode(0x8004).unwrap().validated_kind(),
            Some(SectorCipherKind::Xts128)
        );
        assert_eq!(
            EncryptionMethod::decode(0x8005).unwrap().validated_kind(),
            Some(SectorCipherKind::Xts256)
        );
        // CBC-256 + Elephant Diffuser has independent Python vectors.
        assert_eq!(
            EncryptionMethod::decode(0x8001).unwrap().validated_kind(),
            Some(SectorCipherKind::Cbc256Diffuser),
            "0x8001 has independent cipher vectors"
        );
    }
}
