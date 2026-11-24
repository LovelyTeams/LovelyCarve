//! Why a database could not be read.

/// A failure to read a KDBX database.
///
/// The variants separate what a caller acts on: a file that is not a KDBX
/// database or uses something this reader does not implement, a damaged
/// file, a key that does not open it, and a configured limit.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file does not start with a KeePass 2.x signature.
    #[error("not a KeePass database")]
    NotKeePass,
    /// A KeePass 1.x (KDB) database, a different format.
    #[error("KeePass 1.x (KDB) database")]
    KeePass1,
    /// A KDBX file version this reader does not read.
    #[error("unsupported KDBX version {major}.{minor}")]
    UnsupportedVersion { major: u16, minor: u16 },
    /// A cipher, key derivation, compression or other algorithm this reader
    /// does not implement.
    #[error("unsupported {0}")]
    Unsupported(String),
    /// The file ends inside a structure.
    #[error("truncated {0}")]
    Truncated(&'static str),
    /// A structure fails validation.
    #[error("malformed {0}")]
    Malformed(String),
    /// The SHA-256 stored after a KDBX 4 header does not match the header.
    #[error("header checksum mismatch")]
    HeaderChecksum,
    /// The password, keyfile or transformed key does not open the database.
    #[error("invalid credentials")]
    InvalidCredentials,
    /// A payload block fails its SHA-256 (KDBX 3) or HMAC-SHA-256 (KDBX 4).
    #[error("payload block {index} fails its checksum")]
    BlockChecksum { index: u64 },
    /// The keyfile is a KeePass XML keyfile that cannot be used.
    #[error("keyfile {0}")]
    Keyfile(String),
    /// The decrypted XML is not well formed.
    #[error("XML payload: {0}")]
    Xml(String),
    /// A configured limit was reached.
    #[error("limit reached: {0}")]
    Limit(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
