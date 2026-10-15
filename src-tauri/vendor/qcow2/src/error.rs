use std::error::Error as StdError;
use std::fmt::{self, Display, Formatter};
use std::io;
use std::sync::PoisonError;

/// The error type for Qcow2 operations.
#[derive(Debug)]
pub enum Error {
    /// An operation was cancelled by its caller.
    Cancelled,
    /// Legacy-encrypted logical bytes require an explicit key.
    KeyRequired,
    /// A declared backing source has not been attached.
    MissingBacking,
    /// A backing file could not be opened; retains source context.
    BackingFile {
        /// Resolved parent filename.
        path: std::path::PathBuf,
        /// Underlying failure.
        error: io::Error,
    },
    /// The underlying source of a Qcow2 reported an I/O error.
    Io(io::Error),

    /// A synchronization primitive reported being poisoned.
    /// See [std::sync::PoisonError](https://doc.rust-lang.org/std/sync/struct.PoisonError.html).
    Poison(String),

    /// The file being opened is not a qcow2 file.
    FileType,

    /// The file being opened has an unsupported version.
    Version(u32),

    /// A feature unsupported by this library was detected.
    UnsupportedFeature(String),

    /// An error was detected in a qcow2 file. The file may be corrupt.
    FileFormat(String),

    /// An internal error was detected, there must be a bug in this library.
    Internal(String),
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::Io(err)
    }
}

impl<T> From<PoisonError<T>> for Error {
    fn from(err: PoisonError<T>) -> Error {
        Error::Poison(format!("{}", err))
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match *self {
            Error::BackingFile { ref error, .. } => Some(error),
            Error::Io(ref err) => Some(err),
            _ => None,
        }
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match *self {
            Error::Cancelled => f.write_str("QCOW read cancelled"),
            Error::KeyRequired => {
                f.write_str("QCOW AES-128-CBC requires a password or 16-byte key")
            }
            Error::MissingBacking => {
                f.write_str("QCOW backing file is required for unallocated data")
            }
            Error::BackingFile {
                ref path,
                ref error,
            } => write!(f, "QCOW backing file {}: {error}", path.display()),
            Error::Io(ref err) => err.fmt(f),
            Error::FileType => f.write_str("Not a qcow2 file"),
            Error::Version(found) => write!(f, "Unsupported version {}", found),
            Error::UnsupportedFeature(ref feat) => write!(f, "Unsupported feature: {}", feat),
            Error::FileFormat(ref err) => write!(f, "Malformed qcow2 file: {}", err),
            Error::Internal(ref err) => write!(f, "Internal error: {}", err),
            Error::Poison(ref s) => f.write_str(s),
        }
    }
}

impl From<Error> for io::Error {
    fn from(err: Error) -> io::Error {
        match err {
            Error::Io(error) => error,
            Error::Cancelled => io::Error::new(io::ErrorKind::Interrupted, err),
            error => io::Error::other(error),
        }
    }
}
