//! BitLocker cryptography: password-key derivation, AES-CCM key unwrap, and
//! AES-CBC sector decryption (with or without the Elephant Diffuser stage).
//!
//! Every primitive comes from an audited RustCrypto crate — `aes`, `cbc`, `ccm`,
//! `sha2`. The one exception, the **Elephant Diffuser** (no ecosystem crate
//! exists), lives in our own [`elephant_diffuser`] crate — extracted from here
//! and validated **in situ** by the Tier-1 `pybde` oracle (a self-authored
//! round-trip proves nothing — see `docs/validation.md`).

use aes::cipher::block_padding::NoPadding;
use aes::cipher::{
    BlockCipher, BlockDecrypt, BlockDecryptMut, BlockEncrypt, BlockSizeUser, KeyInit, KeyIvInit,
};
use aes::{Aes128, Aes256};
use ccm::aead::generic_array::GenericArray;
use ccm::aead::AeadInPlace;
use ccm::consts::{U12, U16};
use ccm::{Ccm, KeyInit as CcmKeyInit};
use sha2::{Digest, Sha256};
use xts_mode::{get_tweak_default, Xts128 as XtsCipher};

/// Number of key-stretch iterations (`0x100000`), per the BDE format.
const STRETCH_ITERATIONS: u64 = 0x0010_0000;

/// AES-256 CCM with a 16-byte tag and a 12-byte nonce — the mode BitLocker uses
/// to wrap the VMK and FVEK (both are 256-bit keys). Type params are the tag
/// size (`U16`) then the nonce size (`U12`).
type BdeCcm = Ccm<aes::Aes256, U16, U12>;

/// Compute the BitLocker password hash: `SHA-256(SHA-256(UTF-16LE(password)))`,
/// with no byte-order mark and no NUL terminator.
#[must_use]
pub fn password_hash(password: &str) -> [u8; 32] {
    let utf16: Vec<u8> = password.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let first = Sha256::digest(&utf16);
    let second = Sha256::digest(first);
    second.into()
}

/// Run the BDE key-stretch loop `iterations` times and return the final 32-byte
/// key. The hashed structure is `last[32] | initial[32] | salt[16] | count(u64
/// LE)`; each round hashes it into `last` and increments `count`.
#[must_use]
pub fn stretch_key_n(password_hash: &[u8; 32], salt: &[u8; 16], iterations: u64) -> [u8; 32] {
    let mut buf = [0u8; 88];
    // buf[0..32] = last (starts zero); buf[32..64] = initial; buf[64..80] = salt.
    buf[32..64].copy_from_slice(password_hash);
    buf[64..80].copy_from_slice(salt);
    for count in 0..iterations {
        buf[80..88].copy_from_slice(&count.to_le_bytes());
        let digest = Sha256::digest(buf);
        buf[0..32].copy_from_slice(&digest);
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&buf[0..32]);
    out
}

/// The full BitLocker password stretch (`0x100000` iterations).
#[must_use]
pub fn stretch_key(password_hash: &[u8; 32], salt: &[u8; 16]) -> [u8; 32] {
    stretch_key_n(password_hash, salt, STRETCH_ITERATIONS)
}

/// Derive the 32-byte key-stretch input from a 48-digit BitLocker **recovery
/// password** (`libbde_recovery.c`). The password is eight groups of six digits;
/// each group must be divisible by 11 (its checksum) and, divided by 11, fit in
/// 16 bits. Those eight 16-bit words, little-endian, form a 16-byte binary key,
/// and its `SHA-256` is the hash fed to [`stretch_key`] — the recovery analogue
/// of [`password_hash`].
///
/// # Errors
/// Returns a static reason string when the format is malformed (not eight
/// six-digit groups, a non-digit, a failed `% 11` checksum, or an out-of-range
/// group) so the caller can fail loud rather than derive a bogus key.
pub fn recovery_key_hash(recovery: &str) -> Result<[u8; 32], &'static str> {
    let mut key = [0u8; 16];
    let mut groups = 0usize;
    for (i, group) in recovery.split('-').enumerate() {
        if i >= 8 {
            return Err("recovery password must be exactly 8 groups");
        }
        if group.len() != 6 || !group.bytes().all(|b| b.is_ascii_digit()) {
            return Err("each recovery group must be 6 digits");
        }
        // 6 ASCII digits fit in u32 (max 999_999).
        let value: u32 = group.parse().map_err(|_| "invalid recovery digits")?;
        if value % 11 != 0 {
            return Err("recovery group failed the divisible-by-11 checksum");
        }
        let word = value / 11;
        if word > u32::from(u16::MAX) {
            return Err("recovery group out of range (value / 11 exceeds 16 bits)");
        }
        key[i * 2..i * 2 + 2].copy_from_slice(&(word as u16).to_le_bytes());
        groups = i + 1;
    }
    if groups != 8 {
        return Err("recovery password must be exactly 8 groups");
    }
    Ok(Sha256::digest(key).into())
}

/// AES-CCM-unwrap a key. `value_data` is an AES-CCM-encrypted-key entry value:
/// `nonce(12) | MAC(16) | ciphertext`. `key` is the 256-bit unwrapping key
/// (stretched key for the VMK, VMK for the FVEK). Returns the plaintext container
/// on success, or `None` when the authentication tag does not verify (wrong key).
#[must_use]
pub fn aes_ccm_unwrap(key: &[u8; 32], value_data: &[u8]) -> Option<Vec<u8>> {
    let nonce = value_data.get(0..12)?;
    let tag = value_data.get(12..28)?;
    let mut buffer = value_data.get(28..)?.to_vec();
    let cipher = <BdeCcm as CcmKeyInit>::new(GenericArray::from_slice(key));
    cipher
        .decrypt_in_place_detached(
            GenericArray::from_slice(nonce),
            &[],
            &mut buffer,
            GenericArray::from_slice(tag),
        )
        .ok()?;
    Some(buffer)
}

/// Wrap `plaintext` into the BitLocker on-disk AES-CCM key layout
/// (`nonce(12) | MAC(16) | ciphertext`) — the inverse of [`aes_ccm_unwrap`], used
/// only to build synthetic volumes in tests.
#[cfg(test)]
#[must_use]
pub fn aes_ccm_wrap(key: &[u8; 32], nonce: &[u8; 12], plaintext: &[u8]) -> Vec<u8> {
    let cipher = <BdeCcm as CcmKeyInit>::new(GenericArray::from_slice(key));
    let mut buffer = plaintext.to_vec();
    let tag = cipher
        .encrypt_in_place_detached(GenericArray::from_slice(nonce), &[], &mut buffer)
        .unwrap();
    let mut out = Vec::with_capacity(12 + 16 + buffer.len());
    out.extend_from_slice(nonce);
    out.extend_from_slice(&tag);
    out.extend_from_slice(&buffer);
    out
}

/// The FVEK-keyed sector transform, selected by encryption method. Each variant
/// owns exactly the key material its cipher needs; the enum keeps the dispatch
/// panic-free and extends one variant per validated method.
//
// The variants differ in size because each holds pre-expanded AES key schedules
// (Cbc128 carries two — `fvek_ecb` and the diffuser tweak). The cipher is built
// once per unlocked volume and lives for its lifetime, while `decrypt_sector`
// runs per sector; boxing a variant would add heap indirection to that hot path
// for no benefit, so the size skew is deliberate.
#[allow(clippy::large_enum_variant)]
enum SectorTransform {
    None,
    /// AES-128-CBC (methods 0x8000/0x8002). `tweak` `Some` ⇒ the Elephant
    /// Diffuser is applied (0x8000); `None` ⇒ the CBC plaintext is final (0x8002).
    Cbc128 {
        fvek: [u8; 16],
        fvek_ecb: Aes128,
        tweak: Option<Aes128>,
    },
    /// AES-256-CBC, no diffuser (method 0x8003).
    Cbc256 {
        fvek: [u8; 32],
        fvek_ecb: Aes256,
        tweak: Option<Aes256>,
    },
    /// XTS-AES-128, tweak = sector number (method 0x8004).
    Xts128 {
        xts: XtsCipher<Aes128>,
    },
    /// XTS-AES-256, tweak = sector number (method 0x8005).
    Xts256 {
        xts: XtsCipher<Aes256>,
    },
}

/// The volume sector cipher: a validated [`SectorTransform`] plus the read-path
/// entry points that apply it at a given volume byte offset.
pub struct SectorCipher {
    transform: SectorTransform,
}

impl SectorCipher {
    pub fn unencrypted() -> Self {
        Self {
            transform: SectorTransform::None,
        }
    }
    /// Build a method-0x8000 cipher (AES-128-CBC + Elephant Diffuser) from the
    /// 16-byte FVEK and TWEAK keys.
    #[must_use]
    pub fn new(fvek: [u8; 16], tweak: [u8; 16]) -> SectorCipher {
        SectorCipher {
            transform: SectorTransform::Cbc128 {
                fvek,
                fvek_ecb: Aes128::new(GenericArray::from_slice(&fvek)),
                tweak: Some(Aes128::new(GenericArray::from_slice(&tweak))),
            },
        }
    }

    /// Build a method-0x8002 cipher (AES-128-CBC, no diffuser) from the 16-byte
    /// FVEK. A no-diffuser volume has no TWEAK key.
    #[must_use]
    pub fn new_cbc(fvek: [u8; 16]) -> SectorCipher {
        SectorCipher {
            transform: SectorTransform::Cbc128 {
                fvek,
                fvek_ecb: Aes128::new(GenericArray::from_slice(&fvek)),
                tweak: None,
            },
        }
    }

    /// Build a method-0x8003 cipher (AES-256-CBC, no diffuser) from the 32-byte
    /// FVEK. A no-diffuser volume has no TWEAK key.
    #[must_use]
    pub fn new_cbc256(fvek: [u8; 32]) -> SectorCipher {
        SectorCipher {
            transform: SectorTransform::Cbc256 {
                fvek,
                fvek_ecb: Aes256::new(GenericArray::from_slice(&fvek)),
                tweak: None,
            },
        }
    }

    /// Build AES-256-CBC with Elephant Diffuser (0x8001).
    #[must_use]
    pub fn new_cbc256_diffuser(fvek: [u8; 32], tweak: [u8; 32]) -> Self {
        Self {
            transform: SectorTransform::Cbc256 {
                fvek,
                fvek_ecb: Aes256::new(GenericArray::from_slice(&fvek)),
                tweak: Some(Aes256::new(GenericArray::from_slice(&tweak))),
            },
        }
    }

    /// Build a method-0x8004 cipher (XTS-AES-128) from the 32-byte FVEK: the
    /// first 16 bytes key the data cipher, the next 16 the tweak cipher.
    #[must_use]
    pub fn new_xts128(fvek: [u8; 32]) -> SectorCipher {
        let data = Aes128::new(GenericArray::from_slice(&fvek[0..16]));
        let tweak = Aes128::new(GenericArray::from_slice(&fvek[16..32]));
        SectorCipher {
            transform: SectorTransform::Xts128 {
                xts: XtsCipher::new(data, tweak),
            },
        }
    }

    /// Build a method-0x8005 cipher (XTS-AES-256) from the 64-byte FVEK: the
    /// first 32 bytes key the data cipher, the next 32 the tweak cipher.
    #[must_use]
    pub fn new_xts256(fvek: [u8; 64]) -> SectorCipher {
        let data = Aes256::new(GenericArray::from_slice(&fvek[0..32]));
        let tweak = Aes256::new(GenericArray::from_slice(&fvek[32..64]));
        SectorCipher {
            transform: SectorTransform::Xts256 {
                xts: XtsCipher::new(data, tweak),
            },
        }
    }

    /// AES-ECB-encrypt one block with `cipher` (used for the CBC IV and the
    /// diffuser sector key). Works for any AES key size (16-byte block).
    fn ecb<C>(cipher: &C, input: &[u8; 16]) -> [u8; 16]
    where
        C: BlockEncrypt + BlockSizeUser<BlockSize = U16>,
    {
        let mut block = GenericArray::clone_from_slice(input);
        cipher.encrypt_block(&mut block);
        block.into()
    }

    /// `LE128(byte_offset)`: the 8-byte little-endian offset, zero-padded to 16.
    fn le128(byte_offset: u64) -> [u8; 16] {
        let mut iv = [0u8; 16];
        iv[0..8].copy_from_slice(&byte_offset.to_le_bytes());
        iv
    }

    /// The 32-byte diffuser per-sector key: `ECB(TWEAK, LE128(off))` ‖
    /// `ECB(TWEAK, LE128(off) with byte[15]=0x80)`.
    fn sector_key<C: BlockEncrypt + BlockSizeUser<BlockSize = U16>>(
        tweak: &C,
        byte_offset: u64,
    ) -> [u8; 32] {
        let mut iv = Self::le128(byte_offset);
        let lower = Self::ecb(tweak, &iv);
        iv[15] = 0x80;
        let upper = Self::ecb(tweak, &iv);
        let mut key = [0u8; 32];
        key[0..16].copy_from_slice(&lower);
        key[16..32].copy_from_slice(&upper);
        key
    }

    /// AES-CBC-decrypt `buf` in place with the FVEK and IV (no padding: the
    /// sector is a 16-byte multiple). Returns the plaintext length. Generic over
    /// the AES key size so 128- and 256-bit CBC share one panic-free body.
    fn cbc_decrypt_inplace<C>(fvek: &[u8], iv: &[u8; 16], buf: &mut [u8]) -> usize
    where
        C: BlockCipher + BlockDecrypt + KeyInit,
    {
        let dec =
            cbc::Decryptor::<C>::new(GenericArray::from_slice(fvek), GenericArray::from_slice(iv));
        let len = buf.len() - (buf.len() % 16);
        // Explicit `match` so the unreachable Err arm is a named, panic-free
        // defence carrying the coverage-gate marker.
        match dec.decrypt_padded_mut::<NoPadding>(&mut buf[..len]) {
            Ok(plain) => plain.len(),
            // `len` is a 16-byte multiple, so NoPadding CBC decryption cannot
            // fail; the arm keeps the reader panic-free if that ever changes.
            Err(_) => len, // cov:unreachable: NoPadding CBC over a 16-byte-multiple slice cannot fail
        }
    }

    /// XTS-decrypt one sector in place. BitLocker keys the XTS data unit off the
    /// sector number (`byte_offset / buf.len() as u64`). Generic over the AES key size so
    /// XTS-128 and XTS-256 share one body. The 16-byte guard keeps the panic-free
    /// contract if a caller ever passes a sub-block slice (the read path never
    /// does); XTS needs at least one full block.
    fn xts_decrypt_sector<C>(xts: &XtsCipher<C>, buf: &mut [u8], byte_offset: u64)
    where
        C: BlockEncrypt + BlockDecrypt + BlockCipher,
    {
        if buf.len() >= 16 {
            xts.decrypt_sector(
                buf,
                get_tweak_default(u128::from(byte_offset / buf.len() as u64)),
            );
        }
    }

    /// XTS-encrypt one sector in place — inverse of [`Self::xts_decrypt_sector`],
    /// used only by the round-trip self-consistency tests.
    #[cfg(test)]
    fn xts_encrypt_sector<C>(xts: &XtsCipher<C>, buf: &mut [u8], byte_offset: u64)
    where
        C: BlockEncrypt + BlockDecrypt + BlockCipher,
    {
        xts.encrypt_sector(
            buf,
            get_tweak_default(u128::from(byte_offset / buf.len() as u64)),
        );
    }

    /// Decrypt one sector of `cipher` at volume byte offset `byte_offset`.
    /// The sector length must be a non-zero multiple of 16.
    #[must_use]
    pub fn decrypt_sector(&self, cipher: &[u8], byte_offset: u64) -> Vec<u8> {
        let mut buf = cipher.to_vec();
        let iv = Self::le128(byte_offset);
        match &self.transform {
            SectorTransform::None => {}
            SectorTransform::Cbc128 {
                fvek,
                fvek_ecb,
                tweak,
            } => {
                let cbc_iv = Self::ecb(fvek_ecb, &iv);
                let plain_len = Self::cbc_decrypt_inplace::<Aes128>(fvek, &cbc_iv, &mut buf);
                // Method 0x8000 only: apply the Elephant Diffuser after CBC.
                if let Some(tweak) = tweak {
                    let sector_key = Self::sector_key(tweak, byte_offset);
                    elephant_diffuser::decrypt(&mut buf[..plain_len], &sector_key);
                }
            }
            SectorTransform::Cbc256 {
                fvek,
                fvek_ecb,
                tweak,
            } => {
                let cbc_iv = Self::ecb(fvek_ecb, &iv);
                let plain_len = Self::cbc_decrypt_inplace::<Aes256>(fvek, &cbc_iv, &mut buf);
                if let Some(tweak) = tweak {
                    elephant_diffuser::decrypt(
                        &mut buf[..plain_len],
                        &Self::sector_key(tweak, byte_offset),
                    );
                }
            }
            SectorTransform::Xts128 { xts } => Self::xts_decrypt_sector(xts, &mut buf, byte_offset),
            SectorTransform::Xts256 { xts } => Self::xts_decrypt_sector(xts, &mut buf, byte_offset),
        }
        buf
    }

    /// Encrypt one sector — the inverse of [`Self::decrypt_sector`], used only by
    /// the round-trip self-consistency tests.
    #[cfg(test)]
    #[must_use]
    pub fn encrypt_sector(&self, plain: &[u8], byte_offset: u64) -> Vec<u8> {
        use aes::cipher::BlockEncryptMut;
        let mut buf = plain.to_vec();
        let iv = Self::le128(byte_offset);
        let len = buf.len() - (buf.len() % 16);
        match &self.transform {
            SectorTransform::None => {}
            SectorTransform::Cbc128 {
                fvek,
                fvek_ecb,
                tweak,
            } => {
                if let Some(tweak) = tweak {
                    let sector_key = Self::sector_key(tweak, byte_offset);
                    elephant_diffuser::encrypt(&mut buf, &sector_key);
                }
                let cbc_iv = Self::ecb(fvek_ecb, &iv);
                cbc::Encryptor::<Aes128>::new(
                    GenericArray::from_slice(fvek),
                    GenericArray::from_slice(&cbc_iv),
                )
                .encrypt_padded_mut::<NoPadding>(&mut buf[..len], len)
                .unwrap();
            }
            SectorTransform::Cbc256 {
                fvek,
                fvek_ecb,
                tweak,
            } => {
                if let Some(tweak) = tweak {
                    elephant_diffuser::encrypt(
                        &mut buf[..len],
                        &Self::sector_key(tweak, byte_offset),
                    );
                }
                let cbc_iv = Self::ecb(fvek_ecb, &iv);
                cbc::Encryptor::<Aes256>::new(
                    GenericArray::from_slice(fvek),
                    GenericArray::from_slice(&cbc_iv),
                )
                .encrypt_padded_mut::<NoPadding>(&mut buf[..len], len)
                .unwrap();
            }
            SectorTransform::Xts128 { xts } => Self::xts_encrypt_sector(xts, &mut buf, byte_offset),
            SectorTransform::Xts256 { xts } => Self::xts_encrypt_sector(xts, &mut buf, byte_offset),
        }
        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        use std::fmt::Write;
        bytes.iter().fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
    }

    #[test]
    fn password_hash_matches_independent_python() {
        // hashlib.sha256(hashlib.sha256("bde-TEST".encode("utf-16-le")).digest())
        assert_eq!(
            hex(&password_hash("bde-TEST")),
            "f5acb5bd3c4e31c5c988128fcfc50717da18ca4f7dbaa8bf21e7525bd431ee3f"
        );
    }

    #[test]
    fn stretch_two_iterations_matches_independent_python() {
        let salt: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        let ph = password_hash("bde-TEST");
        assert_eq!(
            hex(&stretch_key_n(&ph, &salt, 2)),
            "0660651b876f7d3777e292fb96671e57d48a4b34d7e17c3e01f20d6ba6af42e4"
        );
    }

    #[test]
    fn recovery_key_hash_matches_independent_python() {
        // Independent Python: 8x u16-LE of (group / 11), then SHA-256 of the
        // 16-byte key. See docs/validation.md.
        assert_eq!(
            hex(
                &recovery_key_hash("111111-111111-111111-111111-111111-111111-111111-111111")
                    .unwrap()
            ),
            "17f2c896b4e802b3668dfe7f8b22ab00b7adbba67097643f3c02767abc72648e"
        );
        // A real minted recovery password (m8003).
        assert_eq!(
            hex(
                &recovery_key_hash("068002-479633-277629-623568-540826-435039-327756-375705")
                    .unwrap()
            ),
            "6bb2448ffc833b574bcc0e7c3d9f8e8afd7410692289e7405cbcc42a7fee3de3"
        );
    }

    #[test]
    fn recovery_key_hash_rejects_malformed() {
        // Too few groups.
        assert!(recovery_key_hash("111111").is_err());
        // Too many groups (9) — the in-loop `i >= 8` guard.
        assert!(recovery_key_hash(
            "111111-111111-111111-111111-111111-111111-111111-111111-111111"
        )
        .is_err());
        // Non-digit character.
        assert!(
            recovery_key_hash("111111-111111-111111-111111-111111-111111-111111-11111x").is_err()
        );
        // Group not exactly six digits.
        assert!(
            recovery_key_hash("11111-111111-111111-111111-111111-111111-111111-111111").is_err()
        );
        // Group not divisible by 11 (bad checksum).
        assert!(
            recovery_key_hash("111112-111111-111111-111111-111111-111111-111111-111111").is_err()
        );
        // Group / 11 exceeds 0xffff (out of range): 999999 / 11 = 90909.
        assert!(
            recovery_key_hash("999999-111111-111111-111111-111111-111111-111111-111111").is_err()
        );
    }

    fn sample_sector() -> Vec<u8> {
        (0..512u32)
            .map(|i| (i.wrapping_mul(31) ^ 0xA5) as u8)
            .collect()
    }

    #[test]
    fn ccm_unwrap_roundtrip_ms_layout() {
        // Encrypt with the ccm crate, assemble BitLocker's nonce|MAC|ciphertext,
        // and confirm aes_ccm_unwrap recovers it (wiring self-consistency; the
        // MS-compat proof is the Tier-1 oracle).
        let key = [7u8; 32];
        let nonce = [3u8; 12];
        let plaintext = b"volume master key container payload!!".to_vec();
        let cipher = <BdeCcm as CcmKeyInit>::new(GenericArray::from_slice(&key));
        let mut buf = plaintext.clone();
        let tag = cipher
            .encrypt_in_place_detached(GenericArray::from_slice(&nonce), &[], &mut buf)
            .unwrap();
        let mut value_data = Vec::new();
        value_data.extend_from_slice(&nonce);
        value_data.extend_from_slice(&tag); // MAC first, then ciphertext
        value_data.extend_from_slice(&buf);

        assert_eq!(aes_ccm_unwrap(&key, &value_data).unwrap(), plaintext);
        // Wrong key -> authentication fails -> None.
        assert!(aes_ccm_unwrap(&[8u8; 32], &value_data).is_none());
        // Truncated value -> None, no panic.
        assert!(aes_ccm_unwrap(&key, &value_data[..20]).is_none());
    }

    #[test]
    fn sector_cipher_roundtrip() {
        let cipher = SectorCipher::new([0x11; 16], [0x22; 16]);
        let plain = sample_sector();
        let off = 0x0211_0800u64;
        let ct = cipher.encrypt_sector(&plain, off);
        assert_ne!(ct, plain);
        assert_eq!(cipher.decrypt_sector(&ct, off), plain);
    }

    #[test]
    fn sector_cipher_cbc_roundtrip() {
        // Method 0x8002: AES-128-CBC only, no diffuser, no tweak (Tier-3
        // self-consistency; oracle_bitlocker1.rs is the Tier-1 proof).
        let cipher = SectorCipher::new_cbc([0x13; 16]);
        let plain = sample_sector();
        let off = 0x0211_0800u64;
        let ct = cipher.encrypt_sector(&plain, off);
        assert_ne!(ct, plain);
        assert_eq!(cipher.decrypt_sector(&ct, off), plain);
    }

    #[test]
    fn sector_cipher_cbc256_roundtrip() {
        // Method 0x8003: AES-256-CBC only, no diffuser, no tweak (Tier-3
        // self-consistency; oracle_m8003.rs is the Tier-2 proof).
        let cipher = SectorCipher::new_cbc256([0x24; 32]);
        let plain = sample_sector();
        let off = 0x0211_0800u64;
        let ct = cipher.encrypt_sector(&plain, off);
        assert_ne!(ct, plain);
        assert_eq!(cipher.decrypt_sector(&ct, off), plain);
    }

    #[test]
    fn sector_cipher_xts128_roundtrip() {
        // Method 0x8004: XTS-AES-128 (Tier-3 self-consistency; the Tier-1 proof
        // is oracle_vault.rs / oracle_m8004.rs).
        let cipher = SectorCipher::new_xts128([0x35; 32]);
        let plain = sample_sector();
        let off = 0x0211_0800u64;
        let ct = cipher.encrypt_sector(&plain, off);
        assert_ne!(ct, plain);
        assert_eq!(cipher.decrypt_sector(&ct, off), plain);
    }

    #[test]
    fn sector_cipher_xts256_roundtrip() {
        // Method 0x8005: XTS-AES-256 (Tier-3 self-consistency; the Tier-2 proof
        // is oracle_m8005.rs).
        let cipher = SectorCipher::new_xts256([0x46; 64]);
        let plain = sample_sector();
        let off = 0x0211_0800u64;
        let ct = cipher.encrypt_sector(&plain, off);
        assert_ne!(ct, plain);
        assert_eq!(cipher.decrypt_sector(&ct, off), plain);
    }
}
