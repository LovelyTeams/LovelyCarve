//! The backup keybag's cryptography, and nothing else.
//!
//! Every primitive here comes from `RustCrypto` — audited, and never hand-rolled.
//! A "simplified" stand-in that returned plausible-but-wrong bytes would
//! **fabricate evidence**, so there is no fallback path in this module: a
//! failure is always an error, never a best-effort result.
//!
//! # The chain
//!
//! ```text
//! password
//!   └─(PBKDF2-HMAC-SHA256, DPSL, DPIC)──┐   ← "double protection", iOS 10.2+
//!                                       ▼
//!   └─(PBKDF2-HMAC-SHA1, SALT, ITER)──> backup key (32 bytes)
//!         └─(AES key unwrap, RFC 3394)──> class key, per protection class
//!               └─(AES key unwrap)──────> file key, from Files.file EncryptionKey
//!                     └─(AES-256-CBC, zero IV)──> plaintext
//! ```
//!
//! The two PBKDF2 rounds use **different digests**. Getting that backwards
//! produces a wrong key that is indistinguishable from a wrong password, which
//! is why this crate's RFC test vectors pin each round separately.

use aes::cipher::{BlockDecryptMut, KeyIvInit};
use hmac::Hmac;
use sha1::Sha1;
use sha2::Sha256;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::Error;
use crate::keybag::KeyBag;

/// Width of every key in the backup chain: AES-256.
pub const KEY_LEN: usize = 32;

/// RFC 3394 wraps an `n`-byte key to `n + 8` bytes, so a class or file key is
/// stored as 40.
pub const WRAPPED_KEY_LEN: usize = KEY_LEN + 8;

/// A protection class whose key is wrapped under the password-derived backup
/// key. `Files.file` records the class alongside the wrapped key.
///
/// `WRAP` is a bitmask; bit 1 (`0x2`) means "wrapped with the passcode-derived
/// key", which is the case every backup class key uses.
pub const WRAP_PASSCODE: u32 = 0x2;

/// The 32-byte key derived from the backup password. Zeroized on drop so it
/// does not outlive the read that needed it.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct BackupKey([u8; KEY_LEN]);

impl BackupKey {
    /// Borrow the raw bytes. Deliberately not `Deref`/`AsRef`: a key should be
    /// awkward to print, log or serialize by accident.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

/// A key never renders its bytes, so a stray `{:?}` cannot leak it into a log
/// or an error report.
impl core::fmt::Debug for BackupKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("BackupKey(<redacted>)")
    }
}

/// The unwrapped per-class keys, keyed by protection class.
#[derive(Clone, Default, Zeroize, ZeroizeOnDrop)]
pub struct ClassKeys {
    /// `(protection_class, key)` pairs. A `Vec` rather than a map: there are at
    /// most a dozen classes, and this keeps the type trivially zeroizable.
    entries: Vec<(u32, [u8; KEY_LEN])>,
}

impl ClassKeys {
    /// The key for `protection_class`, or `None` when the keybag carried none
    /// that the password could unwrap.
    #[must_use]
    pub fn get(&self, protection_class: u32) -> Option<&[u8; KEY_LEN]> {
        self.entries
            .iter()
            .find(|(class, _)| *class == protection_class)
            .map(|(_, key)| key)
    }

    /// How many class keys were recovered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no class key was recovered at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The protection classes present, ascending.
    #[must_use]
    pub fn classes(&self) -> Vec<u32> {
        let mut classes: Vec<u32> = self.entries.iter().map(|(class, _)| *class).collect();
        classes.sort_unstable();
        classes
    }
}

impl core::fmt::Debug for ClassKeys {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ClassKeys")
            .field("classes", &self.classes())
            .finish_non_exhaustive()
    }
}

/// PBKDF2-HMAC-SHA1 into `out`, filling its full length.
///
/// The keybag's `SALT`/`ITER` round. Exposed so an examiner can reproduce a
/// derivation independently of this crate.
pub fn pbkdf2_hmac_sha1(password: &[u8], salt: &[u8], rounds: u32, out: &mut [u8]) {
    // pbkdf2 returns Err only for an invalid output length, which the fixed
    // callers below never produce; a caller-supplied `out` of length 0 is
    // likewise harmless. Ignoring the result keeps this panic-free.
    let _ = pbkdf2::pbkdf2::<Hmac<Sha1>>(password, salt, rounds, out);
}

/// PBKDF2-HMAC-SHA256 into `out`, filling its full length.
///
/// The *double protection* `DPSL`/`DPIC` pre-round, present on backups written
/// by iOS 10.2 and later.
pub fn pbkdf2_hmac_sha256(password: &[u8], salt: &[u8], rounds: u32, out: &mut [u8]) {
    let _ = pbkdf2::pbkdf2::<Hmac<Sha256>>(password, salt, rounds, out);
}

/// AES key unwrap (RFC 3394) of `wrapped` under `kek`.
///
/// The KEK width selects AES-128/192/256 — dispatching on the key's own length
/// rather than assuming 256 keeps this correct for the escrow and system
/// keybags too, not only the backup case.
///
/// # Errors
/// [`Error::UnsupportedKeyWidth`] for a KEK that is not 16, 24 or 32 bytes,
/// and [`Error::KeyUnwrapFailed`] when the RFC 3394 integrity check does not
/// hold — which is the signal that the password (hence the KEK) is wrong.
pub fn aes_key_unwrap(kek: &[u8], wrapped: &[u8]) -> Result<Vec<u8>, Error> {
    // RFC 3394 operates on 64-bit semiblocks and needs at least three of them.
    if !wrapped.len().is_multiple_of(8) || wrapped.len() < 24 {
        return Err(Error::BadWrappedKeyLength(wrapped.len()));
    }
    let mut out = vec![0u8; wrapped.len() - 8];

    match kek.len() {
        16 => {
            let kek = aes_kw::KekAes128::from(
                <[u8; 16]>::try_from(kek).map_err(|_| Error::UnsupportedKeyWidth(kek.len()))?,
            );
            kek.unwrap(wrapped, &mut out)
        }
        24 => {
            let kek = aes_kw::KekAes192::from(
                <[u8; 24]>::try_from(kek).map_err(|_| Error::UnsupportedKeyWidth(kek.len()))?,
            );
            kek.unwrap(wrapped, &mut out)
        }
        32 => {
            let kek = aes_kw::KekAes256::from(
                <[u8; 32]>::try_from(kek).map_err(|_| Error::UnsupportedKeyWidth(kek.len()))?,
            );
            kek.unwrap(wrapped, &mut out)
        }
        other => return Err(Error::UnsupportedKeyWidth(other)),
    }
    .map_err(|_| Error::KeyUnwrapFailed)?;

    Ok(out)
}

/// AES-256-CBC decryption of `ciphertext` under `key` and `iv`.
///
/// No padding is removed: an iOS backup records each file's true length in its
/// manifest, so the caller truncates to that. Stripping PKCS#7 here would guess
/// at a length the evidence already states, and would corrupt a file whose
/// final plaintext byte happens to look like padding.
///
/// # Errors
/// [`Error::BadCiphertextLength`] when `ciphertext` is not a whole number of
/// 16-byte blocks, and [`Error::UnsupportedKeyWidth`] for a non-AES key width.
pub fn decrypt_aes_cbc(key: &[u8], iv: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, Error> {
    if !ciphertext.len().is_multiple_of(16) {
        return Err(Error::BadCiphertextLength(ciphertext.len()));
    }
    if iv.len() != 16 {
        return Err(Error::BadIvLength(iv.len()));
    }
    let mut out = ciphertext.to_vec();

    macro_rules! decrypt_with {
        ($cipher:ty) => {{
            let mut cipher = <$cipher>::new_from_slices(key, iv)
                .map_err(|_| Error::UnsupportedKeyWidth(key.len()))?;
            // Decrypt block by block: `decrypt_blocks_mut` on a chunked slice
            // keeps this allocation-stable and needs no padding trait.
            for block in out.chunks_exact_mut(16) {
                cipher.decrypt_block_mut(block.into());
            }
        }};
    }

    match key.len() {
        16 => decrypt_with!(cbc::Decryptor<aes::Aes128>),
        24 => decrypt_with!(cbc::Decryptor<aes::Aes192>),
        32 => decrypt_with!(cbc::Decryptor<aes::Aes256>),
        other => return Err(Error::UnsupportedKeyWidth(other)),
    }

    Ok(out)
}

/// AES-CBC decryption under the all-zero IV an iOS backup uses for file blobs.
///
/// # Errors
/// As [`decrypt_aes_cbc`].
pub fn decrypt_aes_cbc_zero_iv(key: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, Error> {
    decrypt_aes_cbc(key, &[0u8; 16], ciphertext)
}

/// Derive the backup key from `password` and the keybag's KDF parameters.
///
/// When the keybag carries `DPSL`/`DPIC` the password is first run through
/// PBKDF2-HMAC-**SHA256** (the "double protection" round Apple added in iOS
/// 10.2, which is why an old decryptor fails on a modern backup), and the
/// result becomes the password for the PBKDF2-HMAC-**SHA1** round over
/// `SALT`/`ITER`. Without those fields only the SHA-1 round runs.
///
/// This cannot fail: a wrong password derives a *valid* key that simply will
/// not unwrap anything. Detection belongs to [`unwrap_class_keys`], where the
/// RFC 3394 integrity check lives.
#[must_use]
pub fn derive_backup_key(password: &[u8], keybag: &KeyBag) -> BackupKey {
    let mut derived = [0u8; KEY_LEN];

    match (
        keybag.double_protection_salt.as_deref(),
        keybag.double_protection_iterations,
    ) {
        (Some(dp_salt), Some(dp_rounds)) => {
            let mut first = [0u8; KEY_LEN];
            pbkdf2_hmac_sha256(password, dp_salt, dp_rounds, &mut first);
            pbkdf2_hmac_sha1(&first, &keybag.salt, keybag.iterations, &mut derived);
            first.zeroize();
        }
        _ => pbkdf2_hmac_sha1(password, &keybag.salt, keybag.iterations, &mut derived),
    }

    BackupKey(derived)
}

/// Unwrap every class key the backup key can open.
///
/// # Errors
/// [`Error::WrongPassword`] when no class key unwraps. That is the honest
/// reading: the RFC 3394 integrity check failing on every class means the KEK
/// is wrong, and the KEK is a pure function of the password. Returning an empty
/// set instead would be refusal counted as zero — a caller could not tell a
/// wrong password from a keybag with no passcode-wrapped classes.
pub fn unwrap_class_keys(keybag: &KeyBag, backup_key: &BackupKey) -> Result<ClassKeys, Error> {
    let mut entries = Vec::new();
    let mut candidates = 0usize;

    for class in &keybag.classes {
        // Only passcode-wrapped classes are derivable from the password; a
        // device-bound class (wrapped by the hardware key) is not present in a
        // backup keybag and must not be counted as a failure.
        if class.wrap & WRAP_PASSCODE == 0 {
            continue;
        }
        candidates += 1;

        let Ok(key) = aes_key_unwrap(backup_key.as_bytes(), &class.wrapped_key) else {
            continue;
        };
        let Ok(key) = <[u8; KEY_LEN]>::try_from(key.as_slice()) else {
            continue;
        };
        entries.push((class.protection_class, key));
    }

    if entries.is_empty() && candidates > 0 {
        return Err(Error::WrongPassword);
    }
    Ok(ClassKeys { entries })
}

/// Unwrap a per-file key recorded in `Files.file`'s `EncryptionKey`.
///
/// The stored blob is a 4-byte little-endian protection class followed by the
/// 40-byte wrapped key. The leading class is dropped when present, so both the
/// 44-byte and bare 40-byte encodings are handled.
///
/// # Errors
/// [`Error::NoKeyForClass`] when the keybag yielded no key for
/// `protection_class`, and [`Error::KeyUnwrapFailed`] when the integrity check
/// fails.
pub fn unwrap_file_key(
    class_keys: &ClassKeys,
    protection_class: u32,
    encryption_key: &[u8],
) -> Result<[u8; KEY_LEN], Error> {
    let wrapped = if encryption_key.len() == WRAPPED_KEY_LEN + 4 {
        &encryption_key[4..]
    } else {
        encryption_key
    };

    let class_key = class_keys
        .get(protection_class)
        .ok_or(Error::NoKeyForClass(protection_class))?;

    let key = aes_key_unwrap(class_key, wrapped)?;
    <[u8; KEY_LEN]>::try_from(key.as_slice()).map_err(|_| Error::KeyUnwrapFailed)
}
