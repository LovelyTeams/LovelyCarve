//! Error type for the BitLocker reader/decryptor.
//!
//! The reader never panics on malformed input: out-of-range reads yield safe
//! defaults through the bounds-checked helpers. Genuine failures — a non-BitLocker
//! image, an unsupported cipher, a wrong password (CCM tag mismatch), or missing
//! key material — surface as loud, specific errors that carry the offending value,
//! never a silent empty result.

/// An error decoding or unlocking a BitLocker (BDE) volume.
#[derive(Debug, thiserror::Error)]
pub enum BdeError {
    /// Invalid format geometry, bounds or an inconsistent key container.
    #[error("invalid BitLocker data: {0}")]
    InvalidData(&'static str),
    /// An I/O error reading the underlying volume.
    #[error("i/o error reading BitLocker volume: {0}")]
    Io(#[from] std::io::Error),

    /// The volume header carries neither a BitLocker `-FVE-FS-` signature nor a
    /// BitLocker To Go `MSWIN4.1` signature. The offending bytes at offset 3 are
    /// included so the caller can identify what was actually there.
    #[error("not a BitLocker volume: signature at offset 3 is {signature:02x?} (expected \"-FVE-FS-\" or \"MSWIN4.1\")")]
    NotBitLocker {
        /// The 8 bytes found at offset 3 of the volume header.
        signature: [u8; 8],
    },

    /// No FVE metadata block carried a valid `-FVE-FS-` block-header signature at
    /// any of the three candidate offsets read from the volume header.
    #[error("no valid FVE metadata block found at candidate offsets {offsets:?}")]
    NoValidMetadata {
        /// The three candidate byte offsets tried.
        offsets: [u64; 3],
    },

    /// The encryption-method value is not one of the six defined BitLocker
    /// ciphers (`0x8000`–`0x8005`). The raw value is included.
    #[error("unsupported encryption method 0x{method:04x} (not a recognized BitLocker cipher; supported methods: 0x8000, 0x8001, 0x8002, 0x8003, 0x8004, 0x8005)")]
    UnsupportedEncryptionMethod {
        /// The raw 16-bit encryption-method value from the metadata header.
        method: u16,
    },

    /// The encryption method is a recognized BitLocker cipher but this build has
    /// no oracle-validated decrypt for it yet (AES-256-CBC `0x8003`, or AES-XTS
    /// `0x8004`/`0x8005`, or AES-256-CBC + diffuser `0x8001`). It is refused
    /// rather than decrypted by construction. The raw value is named.
    #[error("unsupported encryption method (recognized but unvalidated) 0x{method:04x} (no validated transform for this method)")]
    UnvalidatedEncryptionMethod {
        /// The raw 16-bit encryption-method value from the metadata header.
        method: u16,
    },

    /// The metadata carries no VMK for the protector the caller tried to unlock
    /// with (password `0x2000` or recovery password `0x0800`). The protector
    /// types that *were* present are listed to guide the examiner toward the
    /// right unlock path.
    #[error("no {protector} protector present; protectors found: {found:?}")]
    NoUnlockProtector {
        /// The protector the caller attempted (e.g. "password", "recovery password").
        protector: &'static str,
        /// The key-protection types that were present.
        found: Vec<u16>,
    },

    /// The supplied recovery password is not a valid 48-digit BitLocker recovery
    /// key (wrong group count, a non-digit, a failed divisible-by-11 checksum, or
    /// an out-of-range group). The specific reason is named.
    #[error("invalid recovery password: {reason}")]
    InvalidRecoveryPassword {
        /// Why the recovery password was rejected.
        reason: &'static str,
    },

    /// The password-protected VMK is missing its stretch key or AES-CCM key
    /// entry, so the VMK cannot be derived. Names which part is absent.
    #[error("password protector is malformed: missing {what}")]
    MissingKeyMaterial {
        /// Which required sub-entry was absent (e.g. "stretch key", "AES-CCM key").
        what: &'static str,
    },

    /// The AES-CCM authentication tag did not verify — for a password unlock this
    /// means the supplied password is wrong; for FVEK it means the VMK is wrong.
    #[error(
        "AES-CCM authentication failed unwrapping the {what} (wrong password or corrupt metadata)"
    )]
    AuthenticationFailed {
        /// Which key was being unwrapped ("volume master key" or "FVEK").
        what: &'static str,
    },

    /// A decrypted key container was shorter than the fixed layout requires, or a
    /// key field ran past the end of the plaintext.
    #[error("decrypted {what} key container is malformed (got {got} bytes, need at least {need})")]
    MalformedKeyContainer {
        /// Which container ("volume master key" or "FVEK").
        what: &'static str,
        /// Bytes actually present.
        got: usize,
        /// Minimum bytes the layout requires.
        need: usize,
    },
}

/// Convenience alias for reader results.
pub type Result<T> = std::result::Result<T, BdeError>;
