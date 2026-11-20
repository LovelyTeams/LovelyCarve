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

/// Detect the matching `SQLCipher` profile and derived keys for `ciphertext`,
/// honoring `params` when supplied or trying [`DETECTED_VERSIONS`] in order.
fn detect_profile(
    ciphertext: &[u8],
    key: &SqlCipherKey,
    params: Option<&CipherParams>,
) -> Result<(Profile, [u8; KEY_LEN], [u8; KEY_LEN]), DecryptError> {
    if ciphertext.len() < SALT_LEN {
        return Err(DecryptError::TooSmall);
    }
    let salt = ciphertext.get(..SALT_LEN).ok_or(DecryptError::TooSmall)?;
    let candidates: Vec<CipherParams> = if let Some(p) = params {
        vec![*p]
    } else {
        DETECTED_VERSIONS
            .iter()
            .filter_map(|version| CipherParams::compatibility(*version))
            .collect()
    };
    for cand in candidates {
        let profile = cand.profile();
        if profile.page_size == 0
            || ciphertext.len() < profile.page_size
            || !ciphertext.len().is_multiple_of(profile.page_size)
        {
            continue;
        }
        let (enc_key, hmac_key) = derive_keys(&profile, key, salt);
        let Some(page1) = ciphertext.get(..profile.page_size) else {
            continue;
        };
        if page_one_ok(&profile, &enc_key, &hmac_key, page1) {
            return Ok((profile, enc_key, hmac_key));
        }
    }
    Err(DecryptError::KeyOrParametersMismatch)
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

/// Decrypt a `SQLCipher` `-wal` sidecar into a plaintext `SQLite` WAL stream,
/// preserving plaintext header/frame metadata and recomputing frame checksums
/// over the decrypted pages when the original encrypted checksum chain verified.
pub fn decrypt_wal(
    db_ciphertext: &[u8],
    wal_ciphertext: &[u8],
    key: &SqlCipherKey,
    params: Option<&CipherParams>,
) -> Result<Vec<u8>, DecryptError> {
    const WAL_HEADER_SIZE: usize = 32;
    const WAL_FRAME_HEADER_SIZE: usize = 24;

    if wal_ciphertext.len() < WAL_HEADER_SIZE {
        return Ok(wal_ciphertext.to_vec());
    }
    let hdr = &wal_ciphertext[..WAL_HEADER_SIZE];
    let magic = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
    let Some(endian) = crate::WalChecksumEndian::from_magic(magic) else {
        return Ok(wal_ciphertext.to_vec());
    };

    let (profile, enc_key, hmac_key) = detect_profile(db_ciphertext, key, params)?;
    let wal_page_size = u32::from_be_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]) as usize;
    if wal_page_size != profile.page_size || wal_page_size == 0 {
        return Err(DecryptError::KeyOrParametersMismatch);
    }

    let (mut orig_s0, mut orig_s1) = crate::wal_checksum(endian, 0, 0, &hdr[0..24]);
    let stored_h0 = u32::from_be_bytes([hdr[24], hdr[25], hdr[26], hdr[27]]);
    let stored_h1 = u32::from_be_bytes([hdr[28], hdr[29], hdr[30], hdr[31]]);
    let mut orig_chain_valid = orig_s0 == stored_h0 && orig_s1 == stored_h1;
    let (mut plain_s0, mut plain_s1) = (orig_s0, orig_s1);

    let hdr_salt1 = &hdr[16..20];
    let hdr_salt2 = &hdr[20..24];
    let stride = WAL_FRAME_HEADER_SIZE + profile.page_size;

    let mut out = Vec::with_capacity(wal_ciphertext.len());
    out.extend_from_slice(hdr);

    for frame in wal_ciphertext[WAL_HEADER_SIZE..].chunks_exact(stride) {
        let f_hdr = &frame[..WAL_FRAME_HEADER_SIZE];
        let enc_page = &frame[WAL_FRAME_HEADER_SIZE..];
        let pgno = u32::from_be_bytes([f_hdr[0], f_hdr[1], f_hdr[2], f_hdr[3]]);
        if pgno == 0 {
            break;
        }
        if orig_chain_valid {
            if &f_hdr[8..12] != hdr_salt1 || &f_hdr[12..16] != hdr_salt2 {
                orig_chain_valid = false;
            } else {
                let (ns0, ns1) = crate::wal_checksum(endian, orig_s0, orig_s1, &f_hdr[0..8]);
                let (ns0, ns1) = crate::wal_checksum(endian, ns0, ns1, enc_page);
                let sf0 = u32::from_be_bytes([f_hdr[16], f_hdr[17], f_hdr[18], f_hdr[19]]);
                let sf1 = u32::from_be_bytes([f_hdr[20], f_hdr[21], f_hdr[22], f_hdr[23]]);
                if sf0 == ns0 && sf1 == ns1 {
                    orig_s0 = ns0;
                    orig_s1 = ns1;
                } else {
                    orig_chain_valid = false;
                }
            }
        }
        let Some(plain_page) = decrypt_page(&profile, &enc_key, &hmac_key, enc_page, pgno) else {
            break;
        };
        let mut out_fhdr = f_hdr.to_vec();
        if orig_chain_valid {
            let (ns0, ns1) = crate::wal_checksum(endian, plain_s0, plain_s1, &out_fhdr[0..8]);
            let (ns0, ns1) = crate::wal_checksum(endian, ns0, ns1, &plain_page);
            plain_s0 = ns0;
            plain_s1 = ns1;
            out_fhdr[16..20].copy_from_slice(&plain_s0.to_be_bytes());
            out_fhdr[20..24].copy_from_slice(&plain_s1.to_be_bytes());
        } else {
            // Preserve invalid checksum semantics across salt resets or corrupt frames.
            let broken = plain_s0.wrapping_add(1);
            out_fhdr[16..20].copy_from_slice(&broken.to_be_bytes());
        }
        out.extend_from_slice(&out_fhdr);
        out.extend_from_slice(&plain_page);
    }

    Ok(out)
}

/// Decrypt a `SQLCipher` rollback `-journal` into a plaintext rollback journal.
pub fn decrypt_rollback_journal(
    db_ciphertext: &[u8],
    journal_ciphertext: &[u8],
    key: &SqlCipherKey,
    params: Option<&CipherParams>,
) -> Result<Vec<u8>, DecryptError> {
    let (profile, enc_key, hmac_key) = detect_profile(db_ciphertext, key, params)?;
    let ps = profile.page_size;
    let stride = 4usize.saturating_add(ps).saturating_add(4);
    if journal_ciphertext.len() < 28 + stride {
        return Ok(journal_ciphertext.to_vec());
    }

    let (sector_start, nonce_opt) = if journal_ciphertext.starts_with(&crate::JOURNAL_MAGIC) {
        let sector = u32::from_be_bytes([
            journal_ciphertext[20],
            journal_ciphertext[21],
            journal_ciphertext[22],
            journal_ciphertext[23],
        ]) as usize;
        let nonce = u32::from_be_bytes([
            journal_ciphertext[12],
            journal_ciphertext[13],
            journal_ciphertext[14],
            journal_ciphertext[15],
        ]);
        (sector.max(28), Some(nonce))
    } else {
        // Tier B: PERSIST zeroed header; locate sector boundary where record 0 decrypts cleanly.
        let mut detected = None;
        for cand in [512usize, 4096, ps] {
            if let Some(rec) = cand
                .checked_add(stride)
                .and_then(|end| journal_ciphertext.get(cand..end))
            {
                let pgno = u32::from_be_bytes([rec[0], rec[1], rec[2], rec[3]]);
                let enc_page = &rec[4..4 + ps];
                if pgno >= 1
                    && decrypt_page(&profile, &enc_key, &hmac_key, enc_page, pgno).is_some_and(
                        |plain| {
                            if pgno == 1 {
                                plausible_page_one(&plain, ps)
                            } else {
                                profile.hmac_len > 0
                                    || matches!(plain.first(), Some(0x02 | 0x05 | 0x0a | 0x0d))
                            }
                        },
                    )
                {
                    detected = Some(cand);
                    break;
                }
            }
        }
        (detected.unwrap_or(ps), None)
    };

    if sector_start >= journal_ciphertext.len() {
        return Ok(journal_ciphertext.to_vec());
    }

    let mut out = Vec::with_capacity(journal_ciphertext.len());
    out.extend_from_slice(&journal_ciphertext[..sector_start]);

    for rec in journal_ciphertext[sector_start..].chunks_exact(stride) {
        let pgno = u32::from_be_bytes([rec[0], rec[1], rec[2], rec[3]]);
        if pgno == 0 {
            out.extend_from_slice(rec);
            break;
        }
        let enc_page = &rec[4..4 + ps];
        let Some(plain_page) = decrypt_page(&profile, &enc_key, &hmac_key, enc_page, pgno) else {
            break;
        };
        let cksum = if let Some(n) = nonce_opt {
            crate::journal_cksum(n, &plain_page).to_be_bytes()
        } else {
            [rec[4 + ps], rec[5 + ps], rec[6 + ps], rec[7 + ps]]
        };
        out.extend_from_slice(&pgno.to_be_bytes());
        out.extend_from_slice(&plain_page);
        out.extend_from_slice(&cksum);
    }

    Ok(out)
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

#[cfg(test)]
mod tests {
    use cipher::BlockEncryptMut;

    use super::*;

    type Aes256CbcEnc = cbc::Encryptor<Aes256>;

    fn build_plain_db_pages(page_size: usize, reserve: usize) -> (Vec<u8>, Vec<u8>) {
        let mut page1 = vec![0u8; page_size];
        page1[..SALT_LEN].copy_from_slice(SQLITE_MAGIC);
        page1[16..18].copy_from_slice(&(page_size as u16).to_be_bytes());
        page1[18] = 1;
        page1[19] = 1;
        page1[20] = reserve as u8;
        page1[21..24].copy_from_slice(&[64, 32, 32]);
        page1[24..28].copy_from_slice(&1u32.to_be_bytes());
        page1[28..32].copy_from_slice(&2u32.to_be_bytes());
        page1[100] = 0x0d; // valid leaf table page header type
        let usable = page_size - reserve;
        page1[105..107].copy_from_slice(&(usable as u16).to_be_bytes());

        let mut page2 = vec![0u8; page_size];
        page2[0] = 0x0d;
        page2[5..7].copy_from_slice(&(usable as u16).to_be_bytes());
        page2[16..24].copy_from_slice(b"FORENSIC");

        (page1, page2)
    }

    fn encrypt_test_page(
        profile: &Profile,
        enc_key: &[u8; KEY_LEN],
        hmac_key: &[u8],
        salt: &[u8; SALT_LEN],
        plain: &[u8],
        pgno: u32,
        iv: [u8; IV_LEN],
    ) -> Vec<u8> {
        let layout = PageLayout::for_page(profile, pgno).expect("layout");
        let mut out = vec![0u8; profile.page_size];
        if pgno == 1 {
            out[..SALT_LEN].copy_from_slice(salt);
        }
        let mut ct = plain[layout.start..layout.iv_start].to_vec();
        let ct_len = ct.len();
        let enc = Aes256CbcEnc::new_from_slices(enc_key, &iv).expect("aes-256-cbc");
        let encrypted = enc
            .encrypt_padded_mut::<NoPadding>(&mut ct, ct_len)
            .expect("aligned page block");
        out[layout.start..layout.iv_start].copy_from_slice(encrypted);
        out[layout.iv_start..layout.iv_start + IV_LEN].copy_from_slice(&iv);

        let auth_region = &out[layout.start..layout.iv_start + IV_LEN];
        let mut mac = <Hmac<Sha512> as Mac>::new_from_slice(hmac_key).expect("hmac key");
        mac.update(auth_region);
        mac.update(&pgno.to_le_bytes());
        let tag = mac.finalize().into_bytes();
        out[layout.iv_start + IV_LEN..layout.iv_start + IV_LEN + profile.hmac_len]
            .copy_from_slice(&tag);
        out
    }

    fn build_v4_test_fixture() -> (
        Profile,
        SqlCipherKey,
        [u8; KEY_LEN],
        [u8; KEY_LEN],
        [u8; SALT_LEN],
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
    ) {
        let params = CipherParams::compatibility(4).expect("v4 profile");
        let profile = params.profile();
        let raw = [0x42u8; KEY_LEN];
        let key = SqlCipherKey::RawKey(raw);
        let salt = [0x17u8; SALT_LEN];
        let (enc_key, hmac_key) = derive_keys(&profile, &key, &salt);
        let (plain1, plain2) = build_plain_db_pages(profile.page_size, profile.reserve);
        let enc1 = encrypt_test_page(&profile, &enc_key, &hmac_key, &salt, &plain1, 1, [0x01; 16]);
        let enc2 = encrypt_test_page(&profile, &enc_key, &hmac_key, &salt, &plain2, 2, [0x02; 16]);
        let mut db_ct = Vec::with_capacity(profile.page_size * 2);
        db_ct.extend_from_slice(&enc1);
        db_ct.extend_from_slice(&enc2);
        (profile, key, enc_key, hmac_key, salt, plain1, plain2, db_ct)
    }

    fn wal_frame(
        pgno: u32,
        commit_pages: u32,
        salt1: &[u8],
        salt2: &[u8],
        endian: crate::WalChecksumEndian,
        prev_s: (u32, u32),
        page: &[u8],
    ) -> (Vec<u8>, (u32, u32)) {
        let mut fhdr = vec![0u8; 24];
        fhdr[0..4].copy_from_slice(&pgno.to_be_bytes());
        fhdr[4..8].copy_from_slice(&commit_pages.to_be_bytes());
        fhdr[8..12].copy_from_slice(salt1);
        fhdr[12..16].copy_from_slice(salt2);
        let (s0, s1) = crate::wal_checksum(endian, prev_s.0, prev_s.1, &fhdr[0..8]);
        let (s0, s1) = crate::wal_checksum(endian, s0, s1, page);
        fhdr[16..20].copy_from_slice(&s0.to_be_bytes());
        fhdr[20..24].copy_from_slice(&s1.to_be_bytes());
        let mut out = fhdr;
        out.extend_from_slice(page);
        (out, (s0, s1))
    }

    #[test]
    fn decrypt_wal_recomputes_valid_chain_and_marks_broken_frames() {
        let (profile, key, enc_key, hmac_key, salt, _plain1, mut plain2_updated, db_ct) =
            build_v4_test_fixture();
        plain2_updated[16..24].copy_from_slice(b"WALFRAME");
        let enc_wal_page = encrypt_test_page(
            &profile,
            &enc_key,
            &hmac_key,
            &salt,
            &plain2_updated,
            2,
            [0x09; 16],
        );

        let endian = crate::WalChecksumEndian::Big;
        let salt1 = [0xAA, 0xBB, 0xCC, 0xDD];
        let salt2 = [0x11, 0x22, 0x33, 0x44];
        let mut hdr = vec![0u8; 32];
        hdr[0..4].copy_from_slice(&0x377f_0683u32.to_be_bytes());
        hdr[4..8].copy_from_slice(&3_007_000u32.to_be_bytes());
        hdr[8..12].copy_from_slice(&(profile.page_size as u32).to_be_bytes());
        hdr[16..20].copy_from_slice(&salt1);
        hdr[20..24].copy_from_slice(&salt2);
        let (h0, h1) = crate::wal_checksum(endian, 0, 0, &hdr[0..24]);
        hdr[24..28].copy_from_slice(&h0.to_be_bytes());
        hdr[28..32].copy_from_slice(&h1.to_be_bytes());

        let (frame1, (f1_s0, f1_s1)) =
            wal_frame(2, 2, &salt1, &salt2, endian, (h0, h1), &enc_wal_page);
        let mut wal_ct = hdr.clone();
        wal_ct.extend_from_slice(&frame1);

        let wal_plain = decrypt_wal(&db_ct, &wal_ct, &key, None).expect("decrypt valid wal");
        assert_eq!(wal_plain.len(), 32 + 24 + profile.page_size);
        let dec_fhdr = &wal_plain[32..56];
        let dec_page = &wal_plain[56..];
        assert_eq!(dec_page[16..24], *b"WALFRAME");
        // Verify decrypted page retains authentic encrypted tail bytes while body matches plaintext.
        assert_eq!(
            dec_page[..profile.page_size - profile.reserve],
            plain2_updated[..profile.page_size - profile.reserve]
        );
        let (want_s0, want_s1) = crate::wal_checksum(endian, h0, h1, &dec_fhdr[0..8]);
        let (want_s0, want_s1) = crate::wal_checksum(endian, want_s0, want_s1, dec_page);
        assert_eq!(&dec_fhdr[16..20], &want_s0.to_be_bytes());
        assert_eq!(&dec_fhdr[20..24], &want_s1.to_be_bytes());

        // Second WAL where frame 1 has a mismatched salt -> checksum must stay broken.
        let (mut bad_salt_frame, _) = wal_frame(
            2,
            2,
            &[0x99; 4],
            &salt2,
            endian,
            (f1_s0, f1_s1),
            &enc_wal_page,
        );
        bad_salt_frame[8..12].copy_from_slice(&[0x99; 4]);
        let mut wal_bad = hdr;
        wal_bad.extend_from_slice(&bad_salt_frame);
        let wal_bad_plain = decrypt_wal(&db_ct, &wal_bad, &key, None).expect("decrypt broken wal");
        let bad_fhdr = &wal_bad_plain[32..56];
        let bad_page = &wal_bad_plain[56..];
        let (valid_if_recomputed, _) = crate::wal_checksum(endian, h0, h1, &bad_fhdr[0..8]);
        let (valid_if_recomputed, _) =
            crate::wal_checksum(endian, valid_if_recomputed, h1, bad_page);
        assert_ne!(
            u32::from_be_bytes([bad_fhdr[16], bad_fhdr[17], bad_fhdr[18], bad_fhdr[19]]),
            valid_if_recomputed
        );
    }

    #[test]
    fn decrypt_wal_handles_non_wal_and_rejects_mismatched_page_size() {
        let (_profile, key, _enc_key, _hmac_key, _salt, _p1, _p2, db_ct) = build_v4_test_fixture();
        assert_eq!(
            decrypt_wal(&db_ct, b"short", &key, None).expect("short"),
            b"short"
        );
        let non_wal = vec![0x11u8; 64];
        assert_eq!(
            decrypt_wal(&db_ct, &non_wal, &key, None).expect("non-wal"),
            non_wal
        );

        let mut mismatch_hdr = vec![0u8; 32];
        mismatch_hdr[0..4].copy_from_slice(&0x377f_0683u32.to_be_bytes());
        mismatch_hdr[8..12].copy_from_slice(&1024u32.to_be_bytes());
        assert_eq!(
            decrypt_wal(&db_ct, &mismatch_hdr, &key, None),
            Err(DecryptError::KeyOrParametersMismatch)
        );
    }

    #[test]
    fn decrypt_rollback_journal_handles_standard_and_persist_headers() {
        let (profile, key, enc_key, hmac_key, salt, plain1, plain2, db_ct) =
            build_v4_test_fixture();
        let ps = profile.page_size;
        let enc2 = encrypt_test_page(&profile, &enc_key, &hmac_key, &salt, &plain2, 2, [0x05; 16]);
        let nonce = 0x1357_2468u32;

        // Standard rollback journal with JOURNAL_MAGIC and sector_size = 512.
        let mut std_journal = vec![0u8; 512];
        std_journal[..8].copy_from_slice(&crate::JOURNAL_MAGIC);
        std_journal[8..12].copy_from_slice(&1u32.to_be_bytes());
        std_journal[12..16].copy_from_slice(&nonce.to_be_bytes());
        std_journal[20..24].copy_from_slice(&512u32.to_be_bytes());
        std_journal[24..28].copy_from_slice(&(ps as u32).to_be_bytes());
        std_journal.extend_from_slice(&2u32.to_be_bytes());
        std_journal.extend_from_slice(&enc2);
        std_journal.extend_from_slice(&crate::journal_cksum(nonce, &enc2).to_be_bytes());
        std_journal.extend(std::iter::repeat_n(0u8, 4 + ps + 4));

        let out_std =
            decrypt_rollback_journal(&db_ct, &std_journal, &key, None).expect("std journal");
        let rec_page = &out_std[516..516 + ps];
        assert_eq!(
            rec_page[..ps - profile.reserve],
            plain2[..ps - profile.reserve]
        );
        let recomputed = crate::journal_cksum(nonce, rec_page).to_be_bytes();
        assert_eq!(&out_std[516 + ps..520 + ps], &recomputed);

        // PERSIST zeroed header with page 1 stored at 512-byte sector boundary.
        let enc1 = encrypt_test_page(&profile, &enc_key, &hmac_key, &salt, &plain1, 1, [0x06; 16]);
        let mut persist_journal = vec![0u8; 512];
        persist_journal.extend_from_slice(&1u32.to_be_bytes());
        persist_journal.extend_from_slice(&enc1);
        let preserved_cksum = [0xDE, 0xAD, 0xBE, 0xEF];
        persist_journal.extend_from_slice(&preserved_cksum);

        let out_persist = decrypt_rollback_journal(&db_ct, &persist_journal, &key, None)
            .expect("persist journal");
        let dec1 = &out_persist[516..516 + ps];
        assert_eq!(dec1[..SALT_LEN], SQLITE_MAGIC[..]);
        assert_eq!(dec1[16..24], plain1[16..24]);
        assert_eq!(&out_persist[516 + ps..520 + ps], &preserved_cksum);
    }
}
