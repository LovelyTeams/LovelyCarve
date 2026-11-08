use thiserror::Error;

#[derive(Error, Debug)]
pub enum CmpfsError {
    #[error("not a decmpfs attribute: magic {0:#010x} (expected 0x636d7066)")]
    InvalidMagic(u32),

    #[error("decmpfs attribute truncated: need {need} bytes, have {have}")]
    Truncated { need: usize, have: usize },

    #[error("resource fork truncated: need {need} bytes, have {have}")]
    TruncatedResourceFork { need: usize, have: usize },

    #[error("compression type {0} stores its payload in the resource fork, which was not supplied")]
    MissingResourceFork(u32),

    #[error("file is dataless (compression type {0:#010x}); its contents are not on this volume")]
    Dataless(u32),

    #[error("unsupported compression type: {0}")]
    Unsupported(u32),

    #[error("corrupted data: {0}")]
    CorruptedData(String),

    #[error("decompression failed: {0}")]
    Decompression(String),
}

pub type Result<T> = std::result::Result<T, CmpfsError>;
