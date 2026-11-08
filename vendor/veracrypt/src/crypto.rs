//! VeraCrypt key derivation (PBKDF2 over five PRFs) and AES/Serpent/Twofish XTS
//! decryption. Every primitive is an audited RustCrypto crate.

use aes::cipher::KeyInit;
use aes::Aes256;
use serpent::Serpent;
use twofish::Twofish;
use xts_mode::Xts128;

use crate::error::{Result, VeraError};

/// A VeraCrypt header PRF (the PBKDF2 hash), with its non-system iteration count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prf {
    /// SHA-512 (500000 iterations).
    Sha512,
    /// SHA-256 (500000 iterations).
    Sha256,
    /// Whirlpool (500000 iterations).
    Whirlpool,
    /// Streebog-512 (500000 iterations).
    Streebog,
    /// RIPEMD-160 (655331 iterations, TrueCrypt-compatible).
    Ripemd160,
}

impl Prf {
    /// All PRFs, in VeraCrypt's try order.
    #[must_use]
    pub fn all() -> [Prf; 5] {
        [
            Prf::Sha512,
            Prf::Sha256,
            Prf::Whirlpool,
            Prf::Streebog,
            Prf::Ripemd160,
        ]
    }

    /// Human name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Prf::Sha512 => "sha512",
            Prf::Sha256 => "sha256",
            Prf::Whirlpool => "whirlpool",
            Prf::Streebog => "streebog",
            Prf::Ripemd160 => "ripemd160",
        }
    }

    /// Non-system, no-PIM iteration count.
    #[must_use]
    pub fn iterations(self) -> u32 {
        match self {
            Prf::Ripemd160 => 655_331,
            _ => 500_000,
        }
    }

    /// Iterations for an explicit PIM (personal iterations multiplier). PIM 0 uses
    /// the default. For SHA-family/Whirlpool/Streebog: `15000 + PIM*1000`;
    /// RIPEMD-160: `PIM*2048` — matching VeraCrypt's non-system formula.
    #[must_use]
    pub fn iterations_pim(self, pim: u32) -> u32 {
        if pim == 0 {
            return self.iterations();
        }
        match self {
            Prf::Ripemd160 => pim.saturating_mul(2048),
            _ => 15_000u32.saturating_add(pim.saturating_mul(1000)),
        }
    }

    /// Derive `out_len` bytes with PBKDF2-HMAC using this PRF.
    pub fn derive(self, password: &[u8], salt: &[u8], iterations: u32, out_len: usize) -> Vec<u8> {
        let mut out = vec![0u8; out_len];
        let it = iterations.max(1);
        match self {
            Prf::Sha512 => pbkdf2::pbkdf2_hmac::<sha2::Sha512>(password, salt, it, &mut out),
            Prf::Sha256 => pbkdf2::pbkdf2_hmac::<sha2::Sha256>(password, salt, it, &mut out),
            Prf::Whirlpool => {
                pbkdf2::pbkdf2_hmac::<whirlpool::Whirlpool>(password, salt, it, &mut out);
            }
            Prf::Streebog => {
                pbkdf2::pbkdf2_hmac::<streebog::Streebog512>(password, salt, it, &mut out);
            }
            Prf::Ripemd160 => {
                pbkdf2::pbkdf2_hmac::<ripemd::Ripemd160>(password, salt, it, &mut out);
            }
        }
        out
    }
}

/// A VeraCrypt data cipher (single-cipher; cipher cascades are a future
/// extension). All are 256-bit keys in XTS mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cipher {
    /// AES-256 (XTS).
    Aes,
    /// Serpent-256 (XTS).
    Serpent,
    /// Twofish-256 (XTS).
    Twofish,
}

impl Cipher {
    /// All single ciphers, in VeraCrypt's try order.
    #[must_use]
    pub fn all() -> [Cipher; 3] {
        [Cipher::Aes, Cipher::Serpent, Cipher::Twofish]
    }

    /// Human name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Cipher::Aes => "aes",
            Cipher::Serpent => "serpent",
            Cipher::Twofish => "twofish",
        }
    }

    /// XTS key length in bytes (two 256-bit subkeys = 64).
    #[must_use]
    pub fn key_len(self) -> usize {
        64
    }
}

/// The cipher chains VeraCrypt supports, in cryptsetup `tcrypt` **array order**
/// (which fixes the per-cipher key offsets): the three singles, then the cascades.
#[must_use]
pub fn cipher_chains() -> Vec<Vec<Cipher>> {
    use Cipher::{Aes, Serpent, Twofish};
    vec![
        vec![Aes],
        vec![Serpent],
        vec![Twofish],
        vec![Twofish, Aes],          // "twofish-aes"
        vec![Serpent, Twofish, Aes], // "serpent-twofish-aes"
        vec![Aes, Serpent],          // "aes-serpent"
        vec![Aes, Twofish, Serpent], // "aes-twofish-serpent"
        vec![Serpent, Twofish],      // "serpent-twofish"
    ]
}

/// The maximum cascade key length in bytes (three 256-bit XTS ciphers).
pub const MAX_CHAIN_KEY_LEN: usize = 3 * 64;

/// Decrypt `buffer` in place through a cascade of `ciphers` given in cryptsetup
/// array order, exactly as `TCRYPT_decrypt_hdr`: cipher at array index `j` of an
/// `n`-cipher cascade uses XTS key `key[32j..32j+32]` (primary) ++
/// `key[32(n+j)..32(n+j)+32]` (secondary), and the ciphers are applied in **reverse**
/// array order (`j = n-1 .. 0`). A single-cipher chain reduces to [`xts_decrypt`].
///
/// # Errors
/// [`VeraError::Crypto`] if `ciphers` is empty or `key` is shorter than `64*n`.
pub fn xts_decrypt_chain(
    ciphers: &[Cipher],
    key: &[u8],
    buffer: &mut [u8],
    unit_size: usize,
    base_unit: u128,
) -> Result<()> {
    let n = ciphers.len();
    if n == 0 || key.len() < 64 * n {
        return Err(VeraError::Crypto {
            what: "cascade key too short",
        });
    }
    let mut subkey = [0u8; 64];
    for j in (0..n).rev() {
        subkey[..32].copy_from_slice(&key[32 * j..32 * j + 32]);
        subkey[32..].copy_from_slice(&key[32 * (n + j)..32 * (n + j) + 32]);
        xts_decrypt(ciphers[j], &subkey, buffer, unit_size, base_unit)?;
    }
    Ok(())
}

/// Decrypt `buffer` in place as XTS with `cipher`, split into `unit_size`-byte
/// data units; data unit `u` uses tweak `base_unit + u` (little-endian).
///
/// # Errors
/// [`VeraError::Crypto`] if `key` is not 64 bytes.
pub fn xts_decrypt(
    cipher: Cipher,
    key: &[u8],
    buffer: &mut [u8],
    unit_size: usize,
    base_unit: u128,
) -> Result<()> {
    if key.len() != 64 {
        return Err(VeraError::Crypto {
            what: "xts key must be 64 bytes",
        });
    }
    let (k1, k2) = key.split_at(32);
    match cipher {
        Cipher::Aes => decrypt_units(
            &Xts128::new(Aes256::new(k1.into()), Aes256::new(k2.into())),
            buffer,
            unit_size,
            base_unit,
        ),
        Cipher::Serpent => {
            // The typed Serpent::new() is fixed to 16-byte keys, but new_from_slice
            // accepts 16..=32 and uses the full 256-bit key schedule (VeraCrypt's).
            // k1/k2 are exactly 32 bytes here (key.len()==64 checked above), and
            // new_from_slice accepts 16..=32, so the InvalidLength arm is unreachable.
            let c1 = Serpent::new_from_slice(k1).map_err(|_| VeraError::Crypto {
                what: "serpent 256-bit key",
            })?; // cov:unreachable: k1 is 32 bytes
            let c2 = Serpent::new_from_slice(k2).map_err(|_| VeraError::Crypto {
                what: "serpent 256-bit key",
            })?; // cov:unreachable: k2 is 32 bytes
            decrypt_units(&Xts128::new(c1, c2), buffer, unit_size, base_unit);
        }
        Cipher::Twofish => decrypt_units(
            &Xts128::new(Twofish::new(k1.into()), Twofish::new(k2.into())),
            buffer,
            unit_size,
            base_unit,
        ),
    }
    Ok(())
}

fn decrypt_units<C>(xts: &Xts128<C>, buffer: &mut [u8], unit_size: usize, base: u128)
where
    C: aes::cipher::BlockCipher + aes::cipher::BlockEncrypt + aes::cipher::BlockDecrypt,
{
    for (u, chunk) in buffer.chunks_mut(unit_size).enumerate() {
        if chunk.len() < 16 {
            continue; // cov:unreachable: reads are unit-aligned (>= 16)
        }
        let tweak = (base + u as u128).to_le_bytes();
        xts.decrypt_sector(chunk, tweak);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prf_iterations_and_names() {
        assert_eq!(Prf::Sha512.iterations(), 500_000);
        assert_eq!(Prf::Ripemd160.iterations(), 655_331);
        assert_eq!(Prf::Sha512.iterations_pim(0), 500_000);
        assert_eq!(Prf::Sha512.iterations_pim(10), 25_000);
        assert_eq!(Prf::Ripemd160.iterations_pim(10), 20_480);
        assert_eq!(Prf::all().len(), 5);
        assert_eq!(
            Cipher::all().map(Cipher::name),
            ["aes", "serpent", "twofish"]
        );
    }

    #[test]
    fn every_prf_has_a_name_and_derives() {
        // Names for the four non-SHA512 PRFs (SHA-512 is covered by the oracle).
        assert_eq!(Prf::Sha256.name(), "sha256");
        assert_eq!(Prf::Whirlpool.name(), "whirlpool");
        assert_eq!(Prf::Streebog.name(), "streebog");
        assert_eq!(Prf::Ripemd160.name(), "ripemd160");
        // Each PRF derives the requested number of bytes (1 iteration for speed).
        for prf in Prf::all() {
            let k = prf.derive(b"password", b"salt", 1, 64);
            assert_eq!(k.len(), 64, "prf {} derive length", prf.name());
        }
    }

    #[test]
    fn cipher_key_len_is_two_256bit_subkeys() {
        assert_eq!(Cipher::Aes.key_len(), 64);
        assert_eq!(Cipher::Twofish.key_len(), 64);
    }

    #[test]
    fn pbkdf2_sha512_matches_python() {
        // PBKDF2-HMAC-SHA512("password","salt",1,32) — cross-checked vs Python.
        let k = Prf::Sha512.derive(b"password", b"salt", 1, 32);
        assert_eq!(
            hex(&k),
            "867f70cf1ade02cff3752599a3a53dc4af34c7a669815ae5d513554e1c8cf252"
        );
    }

    #[test]
    fn xts_roundtrips_for_each_cipher() {
        for cipher in Cipher::all() {
            let key = [0x24u8; 64];
            let mut buf = vec![0u8; 512];
            for (i, b) in buf.iter_mut().enumerate() {
                *b = (i as u8) ^ 0x3c;
            }
            let plain = buf.clone();
            // encrypt via the same primitive at unit base 256, then decrypt
            let (k1, k2) = key.split_at(32);
            match cipher {
                Cipher::Aes => encrypt_one(
                    &Xts128::new(Aes256::new(k1.into()), Aes256::new(k2.into())),
                    &mut buf,
                    256,
                ),
                Cipher::Serpent => encrypt_one(
                    &Xts128::new(
                        Serpent::new_from_slice(k1).unwrap(),
                        Serpent::new_from_slice(k2).unwrap(),
                    ),
                    &mut buf,
                    256,
                ),
                Cipher::Twofish => encrypt_one(
                    &Xts128::new(Twofish::new(k1.into()), Twofish::new(k2.into())),
                    &mut buf,
                    256,
                ),
            }
            xts_decrypt(cipher, &key, &mut buf, 512, 256).unwrap();
            assert_eq!(buf, plain, "cipher {}", cipher.name());
        }
    }

    #[test]
    fn xts_rejects_bad_key_len() {
        let mut b = [0u8; 512];
        assert!(matches!(
            xts_decrypt(Cipher::Aes, &[0u8; 48], &mut b, 512, 0),
            Err(VeraError::Crypto { .. })
        ));
    }

    #[test]
    fn cipher_chains_are_the_eight_veracrypt_chains() {
        let chains = cipher_chains();
        assert_eq!(chains.len(), 8);
        // Three singles, then the five multi-cipher cascades in cryptsetup order.
        assert_eq!(chains[0], vec![Cipher::Aes]);
        assert_eq!(chains[3], vec![Cipher::Twofish, Cipher::Aes]);
        assert_eq!(
            chains[6],
            vec![Cipher::Aes, Cipher::Twofish, Cipher::Serpent]
        );
        assert!(chains.iter().all(|c| c.len() <= 3));
    }

    #[test]
    fn xts_decrypt_chain_rejects_empty_and_short_key() {
        let mut b = [0u8; 512];
        // Empty chain.
        assert!(matches!(
            xts_decrypt_chain(&[], &[0u8; 64], &mut b, 512, 0),
            Err(VeraError::Crypto { .. })
        ));
        // Two ciphers need 128 bytes; 100 is short.
        assert!(matches!(
            xts_decrypt_chain(&[Cipher::Aes, Cipher::Twofish], &[0u8; 100], &mut b, 512, 0),
            Err(VeraError::Crypto { .. })
        ));
    }

    #[test]
    fn xts_decrypt_chain_roundtrips_a_three_cipher_cascade() {
        // Prove the cryptsetup key layout + reverse-apply order: encrypt forward
        // (e[0]..e[n-1]), decrypt via the chain, recover the plaintext. A
        // three-cipher chain exercises all three cipher arms in one round-trip.
        let ciphers = [Cipher::Aes, Cipher::Twofish, Cipher::Serpent];
        let n = ciphers.len();
        let mut key = [0u8; 192];
        for (i, b) in key.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(7) ^ 0x5a;
        }
        let mut buf = vec![0u8; 512];
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (i as u8) ^ 0x91;
        }
        let plain = buf.clone();
        // Encrypt forward: cipher j uses subkey key[32j..]||key[32(n+j)..].
        for j in 0..n {
            let mut k1 = [0u8; 32];
            let mut k2 = [0u8; 32];
            k1.copy_from_slice(&key[32 * j..32 * j + 32]);
            k2.copy_from_slice(&key[32 * (n + j)..32 * (n + j) + 32]);
            match ciphers[j] {
                Cipher::Aes => encrypt_one(
                    &Xts128::new(Aes256::new((&k1).into()), Aes256::new((&k2).into())),
                    &mut buf,
                    256,
                ),
                Cipher::Twofish => encrypt_one(
                    &Xts128::new(Twofish::new((&k1).into()), Twofish::new((&k2).into())),
                    &mut buf,
                    256,
                ),
                Cipher::Serpent => encrypt_one(
                    &Xts128::new(
                        Serpent::new_from_slice(&k1).unwrap(),
                        Serpent::new_from_slice(&k2).unwrap(),
                    ),
                    &mut buf,
                    256,
                ),
            }
        }
        assert_ne!(buf, plain, "cascade must actually encrypt");
        xts_decrypt_chain(&ciphers, &key, &mut buf, 512, 256).unwrap();
        assert_eq!(buf, plain, "two-cipher cascade round-trip");
    }

    fn encrypt_one<C>(xts: &Xts128<C>, buf: &mut [u8], unit: u128)
    where
        C: aes::cipher::BlockCipher + aes::cipher::BlockEncrypt + aes::cipher::BlockDecrypt,
    {
        xts.encrypt_sector(buf, unit.to_le_bytes());
    }

    fn hex(b: &[u8]) -> String {
        use std::fmt::Write;
        b.iter().fold(String::new(), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
    }
}
