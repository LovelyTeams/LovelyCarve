//! Read-only classic HFS (Mac OS Standard), over an exact positioned byte source.
//! Metadata layouts and MacRoman decoding adapted from fstool; collation from
//! rusty-backup. See ORIGIN.md for pinned sources and licenses. No repair/writer.
#![forbid(unsafe_code)]
mod btree;
mod catalog;
mod fork;
mod metadata;
mod names;
mod reader;
#[cfg(test)]
mod tests;
use lamfold::{BlockSource, FoldError, Result};
pub use metadata::Header;
pub use reader::{ForkKind, Hfs};

fn require(condition: bool, reason: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(FoldError::Corrupt(reason))
    }
}
fn be16(bytes: &[u8], offset: usize) -> Result<u16> {
    let bytes = bytes
        .get(offset..offset + 2)
        .ok_or(FoldError::Corrupt("hfs: short u16"))?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}
fn be32(bytes: &[u8], offset: usize) -> Result<u32> {
    let bytes = bytes
        .get(offset..offset + 4)
        .ok_or(FoldError::Corrupt("hfs: short u32"))?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}
fn exact(source: &mut impl BlockSource, offset: u64, bytes: &mut [u8]) -> Result<()> {
    require(
        offset
            .checked_add(bytes.len() as u64)
            .is_some_and(|end| end <= source.len()),
        "hfs: truncated source/range outside volume",
    )?;
    source.read_at(offset, bytes)
}
