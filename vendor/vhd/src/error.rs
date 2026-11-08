use thiserror::Error;

#[derive(Debug, Error)]
pub enum VhdError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a VHD file (bad cookie)")]
    BadCookie,
    #[error("unsupported VHD version: {0:#010x}")]
    UnsupportedVersion(u32),
    #[error("differencing disks are not supported")]
    DifferencingNotSupported,
    #[error("unknown disk type: {0}")]
    UnknownDiskType(u32),
    #[error("file too small to be a valid VHD")]
    FileTooSmall,
    #[error("footer checksum mismatch (expected {expected:#010x}, got {actual:#010x})")]
    ChecksumMismatch { expected: u32, actual: u32 },
    #[error("BAT offset out of bounds")]
    BatOutOfBounds,
    #[error("block data offset out of bounds")]
    BlockOutOfBounds,
    #[error("block_size must be a power of two of at least 512 bytes")]
    InvalidBlockSize,
}

pub type Result<T> = std::result::Result<T, VhdError>;
