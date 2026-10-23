//! The error type for the lamfold substrate and its frontends.

use core::fmt;

pub type Result<T> = core::result::Result<T, FoldError>;

/// Every fallible lamfold operation returns this. Variants carry a stable
/// `&'static str` tokens, except host Source errors which retain owned context.
/// The type remains `no_std`-friendly, and callers can log a stable token.
#[derive(Debug)]
pub enum FoldError {
    /// Path component or node not found.
    NotFound,
    /// Expected a directory, found something else.
    NotDirectory,
    /// Expected a file, found a directory.
    IsDirectory,
    /// A path was malformed (not absolute, non-UTF-8 component, …).
    InvalidPath(&'static str),
    /// On-disk structure failed validation (bad magic, checksum, geometry).
    Corrupt(&'static str),
    /// A format/feature this build does not support (e.g. a codec whose cargo
    /// feature is disabled, or a format revision not yet implemented).
    Unsupported(&'static str),
    /// A metadata-reported size exceeded the read cap (see `read_cap`). Rejected
    /// *before* allocation so a hostile size field cannot drive an OOM.
    FileTooLarge { size: u64, max: u64 },
    /// Decompression failed or would exceed the per-block output cap (a
    /// decompression-bomb guard).
    Decompress(&'static str),
    /// Integrity verification (the shepherd) rejected a block/file.
    VerifyFailed(&'static str),
    /// The underlying block source returned an I/O error.
    Io(&'static str),
    /// Host adapter error, retaining the backing evidence reader's context.
    Source(alloc::string::String),
}

impl FoldError {
    /// Short stable token for logging / trust-event `status` fields.
    pub fn as_token(&self) -> &'static str {
        match self {
            FoldError::NotFound => "not_found",
            FoldError::NotDirectory => "not_directory",
            FoldError::IsDirectory => "is_directory",
            FoldError::InvalidPath(_) => "invalid_path",
            FoldError::Corrupt(_) => "corrupt",
            FoldError::Unsupported(_) => "unsupported",
            FoldError::FileTooLarge { .. } => "file_too_large",
            FoldError::Decompress(_) => "decompress",
            FoldError::VerifyFailed(_) => "verify_failed",
            FoldError::Io(_) | FoldError::Source(_) => "io",
        }
    }
}

impl fmt::Display for FoldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FoldError::NotFound => f.write_str("not found"),
            FoldError::NotDirectory => f.write_str("not a directory"),
            FoldError::IsDirectory => f.write_str("is a directory"),
            FoldError::InvalidPath(w) => write!(f, "invalid path: {w}"),
            FoldError::Corrupt(w) => write!(f, "corrupt: {w}"),
            FoldError::Unsupported(w) => write!(f, "unsupported: {w}"),
            FoldError::FileTooLarge { size, max } => {
                write!(f, "file too large: {size} bytes exceeds cap {max}")
            }
            FoldError::Decompress(w) => write!(f, "decompress: {w}"),
            FoldError::VerifyFailed(w) => write!(f, "verify failed: {w}"),
            FoldError::Io(w) => write!(f, "io: {w}"),
            FoldError::Source(w) => write!(f, "io: {w}"),
        }
    }
}

impl core::error::Error for FoldError {}
