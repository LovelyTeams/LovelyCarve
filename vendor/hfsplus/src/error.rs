use thiserror::Error;

#[derive(Error, Debug)]
pub enum HfsPlusError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid HFS+ signature: 0x{0:04X} (expected 0x482B or 0x4858)")]
    InvalidSignature(u16),

    #[error("invalid B-tree: {0}")]
    InvalidBTree(String),

    #[error("file not found: {0}")]
    FileNotFound(String),

    #[error("not a directory: {0}")]
    NotADirectory(String),

    #[error("corrupted data: {0}")]
    CorruptedData(String),

    #[error("unsupported version: {0}")]
    UnsupportedVersion(u16),

    #[error("decmpfs: {0}")]
    Compression(#[from] cmpfs::CmpfsError),
}

pub type Result<T> = std::result::Result<T, HfsPlusError>;
