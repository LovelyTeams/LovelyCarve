//! Read-only Linux JFS. Metadata layouts adapted from rusty-backup and bounded
//! tree validation from partmgr; see ORIGIN.md for pinned sources and licenses.
//! The byte source must provide exact positioned reads. No writer or replay.
#![forbid(unsafe_code)]
mod allocation;
mod dtree;
mod inodes;
mod metadata;
mod reader;
mod xtree;
pub use reader::Jfs;
#[cfg(test)]
mod tests;
use lamfold::{BlockSource, FoldError, Result};
pub use metadata::{Header, HeaderCopy};

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
        .ok_or(FoldError::Corrupt("jfs: short u16"))?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}
fn le32(bytes: &[u8], offset: usize) -> Result<u32> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or(FoldError::Corrupt("jfs: short u32"))?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}
fn le64(bytes: &[u8], offset: usize) -> Result<u64> {
    let value = bytes
        .get(offset..offset + 8)
        .ok_or(FoldError::Corrupt("jfs: short u64"))?;
    Ok(u64::from_le_bytes([
        value[0], value[1], value[2], value[3], value[4], value[5], value[6], value[7],
    ]))
}
fn exact(source: &mut impl BlockSource, offset: u64, bytes: &mut [u8]) -> Result<()> {
    require(
        offset
            .checked_add(bytes.len() as u64)
            .is_some_and(|end| end <= source.len()),
        "jfs: source range/truncation",
    )?;
    source.read_at(offset, bytes)
}
