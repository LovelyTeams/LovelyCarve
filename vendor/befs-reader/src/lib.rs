//! Immutable BeOS/Haiku BFS. Read layouts and algorithms adapted from
//! rusty-backup's bfs.rs; see ORIGIN.md for provenance and reference semantics.
#![forbid(unsafe_code)]
mod allocation;
mod header;
mod inode;
mod reader;
mod streams;
mod tree;
pub use header::{Endian, Header, Run};
use lamfold::{BlockSource, FoldError, Result};
pub use reader::{Attribute, BeFs, RecordedTimes, TimestampEncoding};
#[cfg(test)]
mod large_tests;
#[cfg(test)]
mod native_tests;
#[cfg(test)]
mod tests;

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
        "befs: source range/truncation",
    )?;
    source.read_at(offset, bytes)
}
