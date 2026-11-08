use thiserror::Error;

#[derive(Debug, Error)]
pub enum VhdxError {
    #[error("not a VHDX file (bad magic)")]
    BadMagic,
    #[error("no valid VHDX header found")]
    NoValidHeader,
    #[error("region table not found or invalid")]
    InvalidRegionTable,
    #[error("BAT region not found in region table")]
    BatRegionMissing,
    #[error("metadata region not found in region table")]
    MetadataRegionMissing,
    #[error("required metadata item missing: {0}")]
    MetadataMissing(&'static str),
    #[error("metadata value is outside valid range: {0}")]
    InvalidMetadata(&'static str),
    #[error("container is too small to be a valid VHDX (minimum {0} bytes required)")]
    ContainerTooSmall(u64),
    #[error("region or BAT file offset is outside the container bounds")]
    OffsetOutOfBounds,
    #[error("BAT entry file offset calculation overflows u64")]
    AddressOverflow,
    #[error("sector out of range (sector {sector}, virtual disk size {size})")]
    SectorOutOfRange { sector: u64, size: u64 },
    #[error("BAT entry not present for payload block {0}")]
    BlockNotPresent(u64),
    #[error("invalid payload BAT state {state} for block {block}")]
    InvalidPayloadBlockState { block: u64, state: u8 },
    #[error("sector bitmap BAT entry is not present for partial payload block {0}")]
    SectorBitmapNotPresent(u64),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("VHDX has a parent locator (differencing disk not supported)")]
    DifferencingNotSupported,
}

pub type Result<T> = std::result::Result<T, VhdxError>;
