//! Error types for DMG operations

use thiserror::Error;

/// Result type alias for DPP operations
pub type Result<T> = std::result::Result<T, DppError>;

/// Errors that can occur during DMG operations
#[derive(Error, Debug)]
pub enum DppError {
    /// Invalid DMG magic bytes (expected "koly")
    #[error("invalid DMG magic: expected 'koly' trailer")]
    InvalidMagic,

    /// I/O error
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Invalid koly header structure
    #[error("invalid koly header: {0}")]
    InvalidKolyHeader(String),

    /// Invalid plist format
    #[error("invalid plist: {0}")]
    InvalidPlist(String),

    /// Invalid block map (mish) data
    #[error("invalid block map: {0}")]
    InvalidBlockMap(String),

    /// Decompression error
    #[error("decompression error: {0}")]
    Decompression(String),

    /// Compression error
    #[error("compression error: {0}")]
    Compression(String),

    /// Unsupported compression type
    #[error("unsupported compression type: {0:#x}")]
    UnsupportedCompression(u32),

    /// File not found in DMG
    #[error("file not found: {0}")]
    FileNotFound(String),

    /// Invalid path (e.g., path traversal attempt)
    #[error("invalid path: {0}")]
    InvalidPath(String),

    /// Base64 decoding error
    #[error("base64 decode error: {0}")]
    Base64Error(String),

    /// XML parsing error
    #[error("XML parsing error: {0}")]
    XmlError(String),

    /// Unsupported feature
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// Checksum mismatch
    #[error("checksum mismatch: expected {expected:#x}, got {actual:#x}")]
    ChecksumMismatch { expected: u32, actual: u32 },
}
