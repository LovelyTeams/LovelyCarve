//! Pure Rust, read-only Minix V1/V2/V3 filesystem reader. Superblock and inode
//! layouts are adapted from danifunker/rusty-backup (AGPL-3.0); see ORIGIN.md.
//! The source contract requires exact positioned reads. No writer, mounting or
//! repair API is included. Sparse pointers are the only synthesized zero data.
#![forbid(unsafe_code)]
mod metadata;
mod reader;
#[cfg(test)]
mod tests;
pub use metadata::{Endian, Superblock, Version};
pub use reader::Minix;

use lamfold::{BlockSource, FoldError, Result};
fn require(condition: bool, reason: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(FoldError::Corrupt(reason))
    }
}
fn exact(source: &mut impl BlockSource, offset: u64, bytes: &mut [u8]) -> Result<()> {
    require(
        offset
            .checked_add(bytes.len() as u64)
            .is_some_and(|end| end <= source.len()),
        "minix: source range/truncation",
    )?;
    source.read_at(offset, bytes)
}
