//! Immutable ReiserFS 3.5/3.6 reader. Layouts adapted from rusty-backup;
//! see ORIGIN.md for pinned upstream and primary format references.
//! No journal replay, source writes, native driver or C dependency.
#![forbid(unsafe_code)]
mod header;
mod objects;
mod reader;
mod tree;
pub use header::{Format, Header};
use lamfold::{BlockSource, FoldError, Result};
pub use reader::ReiserFs;
#[cfg(test)]
mod tests;

fn require(condition: bool, reason: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(FoldError::Corrupt(reason))
    }
}
fn le16(bytes: &[u8], offset: usize) -> Result<u16> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or(FoldError::Corrupt("reiserfs: short u16"))?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}
fn le32(bytes: &[u8], offset: usize) -> Result<u32> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or(FoldError::Corrupt("reiserfs: short u32"))?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}
fn le64(bytes: &[u8], offset: usize) -> Result<u64> {
    let value = bytes
        .get(offset..offset + 8)
        .ok_or(FoldError::Corrupt("reiserfs: short u64"))?;
    Ok(u64::from_le_bytes([
        value[0], value[1], value[2], value[3], value[4], value[5], value[6], value[7],
    ]))
}
fn exact(source: &mut impl BlockSource, offset: u64, bytes: &mut [u8]) -> Result<()> {
    require(
        offset
            .checked_add(bytes.len() as u64)
            .is_some_and(|end| end <= source.len()),
        "reiserfs: source truncation/range",
    )?;
    source.read_at(offset, bytes)
}
