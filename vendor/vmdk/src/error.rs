use thiserror::Error;

pub type Result<T> = std::result::Result<T, VmdkError>;

/// Errors returned while opening or parsing a VMDK image.
#[derive(Debug, Error)]
pub enum VmdkError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a VMware VMDK file: bad magic number")]
    BadMagic,
    #[error("unsupported VMDK version: {0}")]
    UnsupportedVersion(u32),
    #[error("compressed VMDKs are not supported")]
    CompressedNotSupported,
    #[error("VMDK file too small")]
    FileTooSmall,
    /// An arithmetic computation on a geometry field overflowed `u64`.
    #[error("geometry field `{field}` overflowed")]
    GeometryOverflow { field: &'static str },
    /// A geometry field held a value outside its valid range.
    #[error("geometry field `{field}` = {value} is invalid: {reason}")]
    FieldOutOfRange {
        /// The header/descriptor field that was out of range.
        field: &'static str,
        /// The offending value as read.
        value: u64,
        /// Why it is invalid (the expected range).
        reason: &'static str,
    },
    /// The text descriptor was structurally malformed.
    #[error("malformed descriptor: {0}")]
    MalformedDescriptor(&'static str),
    #[error("unsupported VMDK disk type: {0}")]
    UnsupportedDiskType(String),
}
