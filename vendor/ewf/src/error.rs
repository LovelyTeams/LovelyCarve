use std::io;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum EwfError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("invalid EWF signature: found {found:02x?}, expected EVF (physical) or LVF (logical)")]
    InvalidSignature {
        /// The eight bytes actually read, so the caller can see what this is.
        found: [u8; 8],
    },

    /// The container is a valid EWF image, but a PHYSICAL one, and the
    /// operation needs a logical evidence file.
    ///
    /// Distinct from [`Self::InvalidSignature`] on purpose. A physical E01 is
    /// not damaged and its signature is perfectly valid — it simply has no
    /// `ltree` section, because a disk image holds sectors rather than a file
    /// tree. Reporting that as a bad signature sends an examiner hunting for
    /// corruption in a sound image.
    #[error(
        "no file-entry tree: this is a PHYSICAL evidence file (EVF/.E01), which images a disk \
         and has no `ltree` section. `ls` enumerates a LOGICAL evidence file (LVF/.L01). \
         Searched {segments} segment(s)."
    )]
    NotLogicalEvidence {
        /// How many segment files were searched for an `ltree`.
        segments: usize,
    },

    /// A logical entry's field could not be decoded.
    ///
    /// Carries the field name and the offending value verbatim, because
    /// "malformed" without the bytes is not a diagnosis.
    #[error("malformed logical entry field `{field}`: {value:?}")]
    MalformedLogicalField {
        /// Which field failed to decode (e.g. `be`, `du`).
        field: &'static str,
        /// The value as stored, verbatim.
        value: String,
    },

    #[error("buffer too short: expected {expected}, got {got}")]
    BufferTooShort { expected: usize, got: usize },

    #[error("invalid chunk size: {0}")]
    InvalidChunkSize(u32),

    #[error("missing volume section")]
    MissingVolume,

    #[error("decompression error: {0}")]
    Decompression(String),

    #[error("segment gap: expected segment {expected}, got {got}")]
    SegmentGap { expected: u32, got: u32 },

    #[error("no segment files found matching: {0}")]
    NoSegments(String),

    #[error("parse error: {0}")]
    Parse(String),

    #[error("encrypted EWF2 images are not supported")]
    EncryptedNotSupported,
}

pub type Result<T> = std::result::Result<T, EwfError>;
