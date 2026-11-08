//! `SQLCipher` at-rest decryption → a plaintext `SQLite` byte stream the reader
//! ([`crate::Database::open`]) consumes unchanged.
//!
//! # What `SQLCipher` does (and how we undo it)
//!
//! A `SQLCipher` database is an ordinary page-structured `SQLite` file whose every
//! page is encrypted with **AES-256-CBC** and authenticated with a per-page
//! **HMAC**. The first 16 bytes of the file are a random **salt** (in place of
//! the `SQLite format 3\0` magic). Key material is derived with **`PBKDF2`**:
//!
//! - encryption key: `PBKDF2(passphrase, salt, kdf_iter, 32)` — or a raw 32-byte
//!   key used directly (`PRAGMA key = "x'<64 hex>'"`);
//! - HMAC key: `PBKDF2(encryption_key, salt ^ 0x3a, 2, 32)`.
//!
//! Each page's tail holds `[ IV(16) | HMAC | padding ]` occupying `reserve`
//! bytes. The HMAC authenticates `ciphertext || IV || page_no_le32`. Page 1's
//! first 16 bytes (the salt) are not encrypted; on decrypt we prepend the
//! standard magic to reconstruct a valid plaintext page 1. The plaintext header
//! carries `SQLCipher`'s own reserved-space byte, so the reader computes the
//! correct usable size with no further help.
//!
//! # Version detection
//!
//! The two shipped profiles are the `SQLCipher` v4 and v3 defaults; they differ in
//! `PBKDF2`/HMAC digest (SHA-512 vs SHA-1), iteration count, default page size, and
//! reserve. Because nothing in the header is readable before decryption, the
//! version is detected by **HMAC verification on page 1**: the first profile whose
//! page-1 tag matches the derived key is the correct one. A wrong key/parameters
//! matches no profile and fails loud ([`DecryptError::KeyOrParametersMismatch`]) —
//! never a silent wrong-output.
//!
//! # Crypto provenance
//!
//! Every primitive is an audited `RustCrypto` crate (`pbkdf2`, `hmac`, `sha1`,
//! `sha2`, `aes`, `cbc`). Nothing here is hand-rolled.

use aes::Aes256;
use cipher::block_padding::NoPadding;
use cipher::{BlockDecryptMut, KeyIvInit};
use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::Sha512;

/// Per-file random salt length, and the length of page 1's plaintext magic.
const SALT_LEN: usize = 16;
/// AES-CBC initialization-vector length (one block).
const IV_LEN: usize = 16;
/// AES-256 key length.
const KEY_LEN: usize = 32;
/// XOR mask applied to the salt to derive the HMAC-key salt (`SQLCipher`
/// `HMAC_SALT_MASK`).
const HMAC_SALT_MASK: u8 = 0x3a;
/// `PBKDF2` iterations for the HMAC-key derivation (`SQLCipher` `FAST_PBKDF2`).
const HMAC_KDF_ITER: u32 = 2;
/// The 16-byte header every plaintext `SQLite` file begins with.
const SQLITE_MAGIC: &[u8; SALT_LEN] = b"SQLite format 3\x00";

type Aes256CbcDec = cbc::Decryptor<Aes256>;

/// The key supplied by the caller.
///
/// Secure-by-design: the two shapes are distinct types, so a raw key can never be
/// mistaken for a passphrase (which would silently `PBKDF2`-stretch 32 random bytes
/// and fail to decrypt).
#[derive(Clone)]
pub enum SqlCipherKey {
    /// A user passphrase (`PRAGMA key = 'passphrase'`); the encryption key is
    /// `PBKDF2`-derived from it and the database's per-file salt.
    Passphrase(Vec<u8>),
    /// A raw 32-byte key (`PRAGMA key = "x'<64 hex>'"`), used directly as the
    /// AES-256 key. The salt for HMAC-key derivation still comes from the file.
    RawKey([u8; KEY_LEN]),
}

/// The `SQLCipher` parameter set that decrypted a database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlCipherVersion {
    /// `SQLCipher` 4 defaults: `PBKDF2`/HMAC-SHA512, 256 000 iterations, 4096-byte
    /// pages, 80-byte reserve.
    V4,
    /// `SQLCipher` 3 defaults (or `cipher_compatibility = 3`): `PBKDF2`/HMAC-SHA1,
    /// 64 000 iterations, 1024-byte pages, 48-byte reserve.
    V3,
    /// `SQLCipher` 2 defaults (`cipher_compatibility = 2`): `PBKDF2`/HMAC-SHA1,
    /// 4 000 iterations, 1024-byte pages, 48-byte reserve.
    V2,
    /// `SQLCipher` 1 defaults (`cipher_compatibility = 1`): `PBKDF2`-SHA1, 4 000
    /// iterations, 1024-byte pages, no page HMAC (16-byte IV reserve).
    V1,
    /// Explicit [`CipherParams`] that match none of the version defaults.
    Custom,
}

/// The digest `SQLCipher` uses for `PBKDF2` key derivation and the page HMAC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherDigest {
    /// SHA-1 (`SQLCipher` 1–3).
    Sha1,
    /// SHA-512 (`SQLCipher` 4).
    Sha512,
}

/// Explicit `SQLCipher` parameters, for databases that do not use the defaults
/// [`decrypt`] detects — a `cipher_page_size` or `kdf_iter` override, or
/// `cipher_compatibility = 1/2` databases such as `WeChat`'s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CipherParams {
    /// Page size in bytes.
    pub page_size: usize,
    /// `PBKDF2` iterations deriving the key from a passphrase (unused for a raw key).
    pub kdf_iter: u32,
    /// `PBKDF2` / HMAC digest.
    pub digest: CipherDigest,
    /// Whether every page carries an HMAC (all versions but 1).
    pub hmac: bool,
}

impl CipherParams {
    /// The defaults of `PRAGMA cipher_compatibility = version` (1–4).
    #[must_use]
    pub fn compatibility(version: u8) -> Option<Self> {
        let (page_size, kdf_iter, digest, hmac) = match version {
            1 => (1024, 4_000, CipherDigest::Sha1, false),
            2 => (1024, 4_000, CipherDigest::Sha1, true),
            3 => (1024, 64_000, CipherDigest::Sha1, true),
            4 => (4096, 256_000, CipherDigest::Sha512, true),
            _ => return None,
        };
        Some(Self {
            page_size,
            kdf_iter,
            digest,
            hmac,
        })
    }

    fn version(&self) -> SqlCipherVersion {
        let versions = [
            (4, SqlCipherVersion::V4),
            (3, SqlCipherVersion::V3),
            (2, SqlCipherVersion::V2),
            (1, SqlCipherVersion::V1),
        ];
        versions
            .into_iter()
            .find(|(number, _)| Self::compatibility(*number).as_ref() == Some(self))
            .map_or(SqlCipherVersion::Custom, |(_, version)| version)
    }

    fn profile(&self) -> Profile {
        let hmac_len = match (self.hmac, self.digest) {
            (false, _) => 0,
            (true, CipherDigest::Sha1) => 20,
            (true, CipherDigest::Sha512) => 64,
        };
        Profile {
            version: self.version(),
            page_size: self.page_size,
            kdf_iter: self.kdf_iter,
            prf: self.digest,
            // `IV || HMAC`, rounded up to whole AES blocks.
            reserve: (IV_LEN + hmac_len).div_ceil(IV_LEN) * IV_LEN,
            hmac_len,
        }
    }
}

/// Why decryption could not proceed. Every variant is a loud, recoverable
/// failure — decryption never panics and never emits plausible-but-wrong bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecryptError {
    /// The input is smaller than the 16-byte salt — not a `SQLCipher` file.
    TooSmall,
    /// No shipped profile's page-1 HMAC verified: the key is wrong, or the
    /// database uses non-default cipher parameters this decryptor does not model.
    KeyOrParametersMismatch,
    /// A page past page 1 failed HMAC authentication after page 1 verified —
    /// consistent with tampering or corruption of an otherwise-valid database.
    /// Carries the 1-based page number (show-the-offending-value).
    PageAuthFailed(u32),
    /// The file holds more pages than a 32-bit page number can address.
    TooLarge,
}

/// A decrypted database: the reconstructed plaintext bytes plus the profile that
/// decrypted them.
pub struct Decrypted {
    /// A valid, standalone plaintext `SQLite` file — feed straight to
    /// [`crate::Database::open`].
    pub plaintext: Vec<u8>,
    /// The `SQLCipher` profile that authenticated the pages.
    pub version: SqlCipherVersion,
    /// Logical page size in bytes.
    pub page_size: u32,
}

/// The `PBKDF2`/HMAC digest a profile uses.
type Prf = CipherDigest;

/// A fully-specified `SQLCipher` cipher configuration.
struct Profile {
    version: SqlCipherVersion,
    page_size: usize,
    kdf_iter: u32,
    prf: Prf,
    /// Bytes reserved at the end of each page for `IV || HMAC || padding`.
    reserve: usize,
    /// HMAC tag length (SHA-1 → 20, SHA-512 → 64, 0 without a page HMAC).
    hmac_len: usize,
}

/// The default profiles [`decrypt`] tries, newest first.
const DETECTED_VERSIONS: [u8; 4] = [4, 3, 2, 1];

/// `PBKDF2` into `out`, selecting the PRF digest. Infallible; `out` is any length.
fn pbkdf2(prf: Prf, password: &[u8], salt: &[u8], rounds: u32, out: &mut [u8]) {
    match prf {
        Prf::Sha1 => pbkdf2::pbkdf2_hmac::<Sha1>(password, salt, rounds, out),
        Prf::Sha512 => pbkdf2::pbkdf2_hmac::<Sha512>(password, salt, rounds, out),
    }
}

/// Constant-time HMAC check of `data_a || data_b` against `tag`. Returns `false`
/// (never panics) on any key/length issue.
fn hmac_ok(prf: Prf, key: &[u8], data_a: &[u8], data_b: &[u8], tag: &[u8]) -> bool {
    match prf {
        Prf::Sha1 => {
            let Ok(mut mac) = Hmac::<Sha1>::new_from_slice(key) else {
                return false; // cov:unreachable: HMAC accepts any key length
            };
            mac.update(data_a);
            mac.update(data_b);
            mac.verify_slice(tag).is_ok()
        }
        Prf::Sha512 => {
            let Ok(mut mac) = Hmac::<Sha512>::new_from_slice(key) else {
                return false; // cov:unreachable: HMAC accepts any key length
            };
            mac.update(data_a);
            mac.update(data_b);
            mac.verify_slice(tag).is_ok()
        }
    }
}

/// The encryption key and HMAC key for one profile + supplied key + file salt.
fn derive_keys(
    profile: &Profile,
    key: &SqlCipherKey,
    salt: &[u8],
) -> ([u8; KEY_LEN], [u8; KEY_LEN]) {
    let mut enc = [0u8; KEY_LEN];
    match key {
        SqlCipherKey::Passphrase(pw) => pbkdf2(profile.prf, pw, salt, profile.kdf_iter, &mut enc),
        SqlCipherKey::RawKey(k) => enc.copy_from_slice(k),
    }
    let mut hmac_salt = [0u8; SALT_LEN];
    for (dst, &s) in hmac_salt.iter_mut().zip(salt.iter()) {
        *dst = s ^ HMAC_SALT_MASK;
    }
    let mut hmac_key = [0u8; KEY_LEN];
    pbkdf2(profile.prf, &enc, &hmac_salt, HMAC_KDF_ITER, &mut hmac_key);
    (enc, hmac_key)
}

/// Byte spans within one on-disk page for a given profile and page number.
/// `None` when the page is too short for its own reserve (crafted / truncated).
struct PageLayout {
    /// Where the encrypted region starts (16 on page 1 to skip the salt, else 0).
    start: usize,
    /// Where the IV starts (`page_size - reserve`).
    iv_start: usize,
}

impl PageLayout {
    fn for_page(profile: &Profile, pgno: u32) -> Option<Self> {
        let iv_start = profile.page_size.checked_sub(profile.reserve)?;
        let start = if pgno == 1 { SALT_LEN } else { 0 };
        // Room for at least the ciphertext, the IV, and the HMAC tag.
        if iv_start < start || iv_start.checked_add(IV_LEN + profile.hmac_len)? > profile.page_size
        {
            return None;
        }
        Some(Self { start, iv_start })
    }
}

/// Verify one page's HMAC without decrypting it (used for version detection).
fn page_hmac_ok(profile: &Profile, hmac_key: &[u8], page: &[u8], pgno: u32) -> bool {
    let Some(layout) = PageLayout::for_page(profile, pgno) else {
        return false;
    };
    let (Some(auth_region), Some(tag)) = (
        page.get(layout.start..layout.iv_start + IV_LEN),
        page.get(layout.iv_start + IV_LEN..layout.iv_start + IV_LEN + profile.hmac_len),
    ) else {
        return false; // cov:unreachable: PageLayout bounds already guarantee these
    };
    hmac_ok(profile.prf, hmac_key, auth_region, &pgno.to_le_bytes(), tag)
}

/// Authenticate and decrypt one page, returning the reconstructed plaintext page.
/// `None` on any authentication or bounds failure (panic-free).
fn decrypt_page(
    profile: &Profile,
    enc_key: &[u8; KEY_LEN],
    hmac_key: &[u8],
    page: &[u8],
    pgno: u32,
) -> Option<Vec<u8>> {
    let layout = PageLayout::for_page(profile, pgno)?;
    let iv = page.get(layout.iv_start..layout.iv_start + IV_LEN)?;
    let ciphertext = page.get(layout.start..layout.iv_start)?;
    let auth_region = page.get(layout.start..layout.iv_start + IV_LEN)?;
    let tag = page.get(layout.iv_start + IV_LEN..layout.iv_start + IV_LEN + profile.hmac_len)?;
    let tail = page.get(layout.iv_start..profile.page_size)?;

    // A `SQLCipher` 1 page has no HMAC; its key is checked on page 1 instead
    // (see `page_one_ok`).
    if profile.hmac_len > 0
        && !hmac_ok(profile.prf, hmac_key, auth_region, &pgno.to_le_bytes(), tag)
    {
        return None;
    }
    if ciphertext.len() % IV_LEN != 0 {
        return None; // cov:unreachable: a valid SQLCipher page is block-aligned
    }

    let dec = Aes256CbcDec::new_from_slices(enc_key, iv).ok()?;
    let mut buf = ciphertext.to_vec();
    let plain = dec.decrypt_padded_mut::<NoPadding>(&mut buf).ok()?;

    let mut out = Vec::with_capacity(profile.page_size);
    if pgno == 1 {
        out.extend_from_slice(SQLITE_MAGIC);
    }
    out.extend_from_slice(plain);
    out.extend_from_slice(tail);
    Some(out)
}

/// Decrypt every page under an already-selected profile.
fn decrypt_all(
    profile: &Profile,
    enc_key: &[u8; KEY_LEN],
    hmac_key: &[u8],
    ciphertext: &[u8],
) -> Result<Decrypted, DecryptError> {
    let page_count = ciphertext.len() / profile.page_size;
    let mut out = Vec::with_capacity(page_count * profile.page_size);
    for i in 0..page_count {
        let pgno = u32::try_from(i + 1).map_err(|_| DecryptError::TooLarge)?;
        let start = i * profile.page_size;
        let end = start + profile.page_size;
        let page = ciphertext
            .get(start..end)
            .ok_or(DecryptError::PageAuthFailed(pgno))?;
        let plain = decrypt_page(profile, enc_key, hmac_key, page, pgno)
            .ok_or(DecryptError::PageAuthFailed(pgno))?;
        out.extend_from_slice(&plain);
    }
    Ok(Decrypted {
        plaintext: out,
        version: profile.version,
        page_size: u32::try_from(profile.page_size).unwrap_or(u32::MAX),
    })
}

/// Whether a decrypted page 1 carries a coherent `SQLite` header: its declared
/// page size, file-format versions 1–2 and the fixed 64/32/32 payload
/// fractions. Eight structured bytes, so a wrong key passes with negligible
/// probability — the key check for `SQLCipher` 1, which has no page HMAC.
fn plausible_page_one(plain: &[u8], page_size: usize) -> bool {
    let Some(&[high, low]) = plain.get(16..18) else {
        return false;
    };
    let declared = match u16::from_be_bytes([high, low]) {
        1 => 65_536,
        size => usize::from(size),
    };
    declared == page_size
        && matches!(plain.get(18), Some(1 | 2))
        && matches!(plain.get(19), Some(1 | 2))
        && plain.get(21..24) == Some(&[64, 32, 32][..])
}

/// Whether page 1 authenticates (when the profile has a page HMAC) and decrypts
/// to a coherent `SQLite` header under these keys.
fn page_one_ok(profile: &Profile, enc_key: &[u8; KEY_LEN], hmac_key: &[u8], page1: &[u8]) -> bool {
    if profile.hmac_len > 0 && !page_hmac_ok(profile, hmac_key, page1, 1) {
        return false;
    }
    decrypt_page(profile, enc_key, hmac_key, page1, 1)
        .is_some_and(|plain| plausible_page_one(&plain, profile.page_size))
}

/// Decrypt under one profile, or `None` when its geometry or page-1 check does
/// not fit the database.
fn try_profile(
    profile: &Profile,
    ciphertext: &[u8],
    key: &SqlCipherKey,
) -> Option<Result<Decrypted, DecryptError>> {
    if profile.page_size == 0
        || ciphertext.len() < profile.page_size
        || !ciphertext.len().is_multiple_of(profile.page_size)
    {
        return None;
    }
    let salt = ciphertext.get(..SALT_LEN)?;
    let (enc_key, hmac_key) = derive_keys(profile, key, salt);
    let page1 = ciphertext.get(..profile.page_size)?;
    page_one_ok(profile, &enc_key, &hmac_key, page1)
        .then(|| decrypt_all(profile, &enc_key, &hmac_key, ciphertext))
}

/// Decrypt a `SQLCipher` database into a plaintext `SQLite` byte stream, detecting
/// the cipher version by trying the `cipher_compatibility` 4, 3, 2 and 1
/// defaults in turn: the first profile whose page 1 authenticates (HMAC, or for
/// version 1 a coherent decrypted header) wins.
///
/// Returns [`DecryptError::KeyOrParametersMismatch`] if the key is wrong or the
/// database uses non-default cipher parameters (use [`decrypt_with`]) — a loud
/// failure, never a silent wrong plaintext.
pub fn decrypt(ciphertext: &[u8], key: &SqlCipherKey) -> Result<Decrypted, DecryptError> {
    if ciphertext.len() < SALT_LEN {
        return Err(DecryptError::TooSmall);
    }
    DETECTED_VERSIONS
        .iter()
        .filter_map(|version| CipherParams::compatibility(*version))
        .find_map(|params| try_profile(&params.profile(), ciphertext, key))
        .unwrap_or(Err(DecryptError::KeyOrParametersMismatch))
}

/// Decrypt a `SQLCipher` database with explicit cipher parameters.
///
/// # Errors
/// As [`decrypt`]: [`DecryptError::KeyOrParametersMismatch`] when page 1 does
/// not authenticate under `params` and `key`.
pub fn decrypt_with(
    ciphertext: &[u8],
    key: &SqlCipherKey,
    params: &CipherParams,
) -> Result<Decrypted, DecryptError> {
    if ciphertext.len() < SALT_LEN {
        return Err(DecryptError::TooSmall);
    }
    try_profile(&params.profile(), ciphertext, key)
        .unwrap_or(Err(DecryptError::KeyOrParametersMismatch))
}
