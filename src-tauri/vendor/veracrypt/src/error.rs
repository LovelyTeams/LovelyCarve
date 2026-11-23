//! Errors of the VeraCrypt / TrueCrypt reader.

use std::io;

/// Result alias for `veracrypt-core`.
pub type Result<T> = std::result::Result<T, VeraError>;

/// Why a volume did not open or a read failed.
#[derive(Debug, thiserror::Error)]
pub enum VeraError {
    /// No header the search read decrypted to a valid VeraCrypt or TrueCrypt
    /// header: a wrong password, PIM, keyfile set or header choice, or no
    /// volume at all.
    #[error("no VeraCrypt or TrueCrypt header opened with these credentials")]
    AuthenticationFailed,

    /// The source cannot hold the header the search reads.
    #[error("the source ({len} bytes) is too small for a {what} header")]
    TooSmall {
        /// The header that does not fit.
        what: &'static str,
        /// Bytes available.
        len: u64,
    },

    /// The credentials opened a header whose volume this reader does not
    /// decrypt (a newer header version, an unfinished in-place encryption …).
    #[error("unsupported VeraCrypt volume: {0}")]
    Unsupported(String),

    /// The header opened, but its geometry does not fit the source.
    #[error("corrupt VeraCrypt volume: {0}")]
    Corrupt(String),

    /// The request itself is not valid (a PIM out of range, a keyfile that
    /// cannot be used …).
    #[error("invalid VeraCrypt request: {0}")]
    InvalidRequest(String),

    /// The caller cancelled the search.
    #[error("VeraCrypt operation cancelled")]
    Cancelled,

    /// Reading the source failed.
    #[error("VeraCrypt source read failed: {0}")]
    Io(#[from] io::Error),
}
