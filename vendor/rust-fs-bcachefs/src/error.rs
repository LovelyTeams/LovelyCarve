//! The one error type every parser here returns.

use std::fmt;

/// Why an image could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The underlying device failed to read.
    Io(String),
    /// A magic number did not match; the bytes are not what was expected.
    BadMagic { what: &'static str },
    /// A stored checksum did not match the bytes it covers.
    BadChecksum {
        what: &'static str,
        stored: u64,
        computed: u64,
    },
    /// A structure is shorter than its own header says, or a field is out of range.
    Corrupt(String),
    /// A valid structure using something this reader does not implement.
    Unsupported(String),
    /// The path or key does not exist.
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(m) => write!(f, "I/O error: {m}"),
            Error::BadMagic { what } => write!(f, "{what}: bad magic"),
            Error::BadChecksum {
                what,
                stored,
                computed,
            } => {
                write!(
                    f,
                    "{what}: checksum mismatch (stored {stored:#x}, computed {computed:#x})"
                )
            }
            Error::Corrupt(m) => write!(f, "corrupt: {m}"),
            Error::Unsupported(m) => write!(f, "unsupported: {m}"),
            Error::NotFound(m) => write!(f, "not found: {m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<fs_core::Error> for Error {
    fn from(e: fs_core::Error) -> Self {
        Error::Io(e.to_string())
    }
}
