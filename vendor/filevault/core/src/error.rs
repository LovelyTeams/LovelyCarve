//! Error type for the FileVault / CoreStorage decryptor.

use thiserror::Error;

/// Errors surfaced while parsing or decrypting a CoreStorage / FileVault volume.
///
/// Every failure is loud and named (Fail-loud): a bootstrap failure — a missing
/// CoreStorage signature, an unreadable metadata block, a failed key unwrap — is
/// an explicit error, never an empty/`Ok` degrade. A per-artifact miss inside a
/// validated volume is the only place a silent skip is legitimate.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FileVaultError {
    /// An I/O error reading the underlying image.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// The physical volume header does not carry the CoreStorage `"CS"`
    /// signature at offset 88 — this is not a CoreStorage volume. Carries the
    /// bytes that WERE found (Show-the-unrecognized-value).
    #[error("not a CoreStorage volume: signature at offset 88 was {found:#06x}, expected 0x5343 (\"CS\")")]
    NotCoreStorage {
        /// The 2 signature bytes actually found, little-endian.
        found: u16,
    },

    /// The encryption method is not AES-XTS-128 (the only supported method).
    #[error("unsupported encryption method {found}: only 2 (AES-XTS-128) is supported")]
    UnsupportedEncryptionMethod {
        /// The encryption-method code found in the header at offset 172.
        found: u32,
    },

    /// A required metadata structure could not be located in the (decrypted)
    /// metadata — the encryption context, a wrapped-key struct, or the family
    /// UUID. Names which one so the investigator knows what was missing.
    #[error("required metadata structure not found: {what}")]
    MetadataStructureMissing {
        /// Which structure was missing (e.g. "encryption context plist").
        what: &'static str,
    },

    /// A base64 blob in the encryption context did not decode.
    #[error("base64 decode failed for {what}")]
    Base64 {
        /// Which blob failed to decode.
        what: &'static str,
    },

    /// An RFC 3394 AES key-unwrap failed (wrong password, or corrupt wrapped
    /// key): the integrity check value did not match `0xA6A6A6A6A6A6A6A6`.
    /// The password-derived unwrap failing is the "wrong password" signal.
    #[error("key unwrap failed for {what}: wrong password or corrupt wrapped key")]
    KeyUnwrap {
        /// Which key failed to unwrap (KEK or VMK).
        what: &'static str,
    },

    /// A length or count field taken from the image was out of range.
    #[error("value out of range: {what}")]
    OutOfRange {
        /// What was out of range.
        what: &'static str,
    },
}
