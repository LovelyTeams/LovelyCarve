//! Error type for VeraCrypt parsing and unlocking.

use std::io;

/// Result alias for `veracrypt-core`.
pub type Result<T> = std::result::Result<T, VeraError>;

/// A VeraCrypt parse or unlock failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum VeraError {
    /// No PRF/cipher combination decrypted the header to a valid `VERA` signature
    /// with matching CRCs — a wrong password, an unsupported cipher/PRF, or not a
    /// VeraCrypt volume.
    #[error("authentication failed: no PRF/cipher decrypted a valid VeraCrypt header (wrong password or unsupported cipher)")]
    AuthenticationFailed,

    /// The container is too small to hold a VeraCrypt header.
    #[error("too small for a VeraCrypt header: {got} bytes (need at least 512)")]
    TooSmall {
        /// Bytes available.
        got: usize,
    },

    /// A cryptographic precondition failed.
    #[error("crypto error: {what}")]
    Crypto {
        /// What failed.
        what: &'static str,
    },

    /// An I/O error reading the container.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}
