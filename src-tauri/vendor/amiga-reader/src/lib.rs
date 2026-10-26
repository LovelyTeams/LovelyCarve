//! Immutable OFS/FFS over the existing pure Rust amiga-ffs parser. BlockSource
//! is implemented without BlockSink, so the byte source cannot be written.
#![forbid(unsafe_code)]
mod allocation;
mod geometry;
mod ranges;
mod reader;
#[cfg(test)]
mod tests;
pub use geometry::{Geometry, Hints};
use lamfold::{BlockSource, FoldError, Result};
pub use reader::Amiga;

fn require(condition: bool, reason: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(FoldError::Corrupt(reason))
    }
}
fn convert<T>(result: std::result::Result<T, amiga_ffs::Error<FoldError>>) -> Result<T> {
    result.map_err(|error| match error {
        amiga_ffs::Error::Io(error) => error,
        error => FoldError::Source(format!("amiga: {error}")),
    })
}
fn exact(source: &mut impl BlockSource, offset: u64, bytes: &mut [u8]) -> Result<()> {
    require(
        offset
            .checked_add(bytes.len() as u64)
            .is_some_and(|end| end <= source.len()),
        "amiga: truncated source/range",
    )?;
    source.read_at(offset, bytes)
}
/// A read budget bounds adversarial metadata chains. Each public adapter
/// operation resets it; ordinary chunked reads do not consume a lifetime cap.
struct Source<S> {
    source: S,
    geometry: Geometry,
    remaining: usize,
}
impl<S> Source<S> {
    fn reset(&mut self) {
        self.remaining = 131072;
    }
}
impl<S: BlockSource> amiga_ffs::BlockSource for Source<S> {
    type Error = FoldError;
    fn block_size(&self) -> usize {
        self.geometry.block_size as usize
    }
    fn block_count(&self) -> Option<u64> {
        Some(self.geometry.blocks)
    }
    fn read_block(&mut self, lba: u64, bytes: &mut [u8]) -> Result<()> {
        require(
            bytes.len() == self.block_size() && lba < self.geometry.blocks,
            "amiga: block read geometry",
        )?;
        if self.remaining == 0 {
            return Err(FoldError::Unsupported(
                "amiga: metadata read budget exceeded",
            ));
        }
        self.remaining -= 1;
        exact(
            &mut self.source,
            lba * u64::from(self.geometry.block_size),
            bytes,
        )
    }
}
