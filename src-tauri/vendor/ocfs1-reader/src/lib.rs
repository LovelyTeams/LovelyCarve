//! Read-only OCFS release 1 with the OracleCFS 1.2 disk format.
#![forbid(unsafe_code)]

mod extents;
mod format;
mod object;
mod reader;
#[cfg(test)]
mod tests;

pub use format::{Header, SECTOR_BYTES};
pub use reader::Ocfs1;

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
        .ok_or(FoldError::Corrupt("ocfs1: truncated field"))
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
