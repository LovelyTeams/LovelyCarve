//! Immutable OCFS2 interpretation. See ORIGIN.md for provenance and limits.
#![forbid(unsafe_code)]
mod allocation;
mod attributes;
mod directory;
mod extents;
mod header;
mod index;
mod inode;
mod journal;
mod reader;
mod refcounts;
pub use attributes::Attribute;
pub use header::Header;
use lamfold::{BlockSource, FoldError, Result};
pub use reader::Ocfs2;

fn require(condition: bool, message: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(FoldError::Corrupt(message))
    }
}
fn field<const N: usize>(raw: &[u8], offset: usize) -> Result<[u8; N]> {
    raw.get(
        offset
            ..offset
                .checked_add(N)
                .ok_or(FoldError::Corrupt("ocfs2: field overflow"))?,
    )
    .ok_or(FoldError::Corrupt("ocfs2: truncated field"))?
    .try_into()
    .map_err(|_| FoldError::Corrupt("ocfs2: field length"))
}
fn le16(raw: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(field(raw, offset)?))
}
fn le32(raw: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(field(raw, offset)?))
}
fn le64(raw: &[u8], offset: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(field(raw, offset)?))
}
fn exact(source: &mut impl BlockSource, offset: u64, raw: &mut [u8]) -> Result<()> {
    require(
        offset
            .checked_add(raw.len() as u64)
            .is_some_and(|end| end <= source.len()),
        "ocfs2: source range/truncation",
    )?;
    source.read_at(offset, raw)
}
#[cfg(test)]
mod large_tests;
#[cfg(test)]
mod native_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod unit_tests;
