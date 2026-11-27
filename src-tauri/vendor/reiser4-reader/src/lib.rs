//! Immutable reader for the Reiser4 format40 on-disk layout.
#![forbid(unsafe_code)]

mod cluster;
mod format;
mod object;
mod reader;
#[cfg(test)]
mod tests;
mod tree;

pub use format::{Header, MASTER_OFFSET};
pub use reader::Reiser4;

use lamfold::{FoldError, Result};

fn require(condition: bool, message: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(FoldError::Corrupt(message))
    }
}

fn field<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N]> {
    bytes
        .get(offset..offset.saturating_add(N))
        .and_then(|field| field.try_into().ok())
        .ok_or(FoldError::Corrupt("reiser4: truncated field"))
}

fn le16(bytes: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(field(bytes, offset)?))
}

fn le32(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(field(bytes, offset)?))
}

fn le64(bytes: &[u8], offset: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(field(bytes, offset)?))
}
