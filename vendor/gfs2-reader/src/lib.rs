//! Immutable GFS2 metadata and data interpretation. See ORIGIN.md for sources.
#![forbid(unsafe_code)]
mod allocation;
mod attributes;
mod directory;
mod header;
mod inode;
mod journal;
mod reader;
mod stream;
pub use attributes::Attribute;
pub use header::{Header, Inum};
use lamfold::{BlockSource, FoldError, Result};
pub use reader::Gfs2;

fn require(condition: bool, message: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(FoldError::Corrupt(message))
    }
}
fn bytes<const N: usize>(raw: &[u8], at: usize) -> Result<[u8; N]> {
    raw.get(
        at..at
            .checked_add(N)
            .ok_or(FoldError::Corrupt("gfs2: field overflow"))?,
    )
    .ok_or(FoldError::Corrupt("gfs2: truncated field"))?
    .try_into()
    .map_err(|_| FoldError::Corrupt("gfs2: field length"))
}
fn u16be(raw: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_be_bytes(bytes(raw, at)?))
}
fn u32be(raw: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(bytes(raw, at)?))
}
fn u64be(raw: &[u8], at: usize) -> Result<u64> {
    Ok(u64::from_be_bytes(bytes(raw, at)?))
}
fn exact(source: &mut impl BlockSource, at: u64, raw: &mut [u8]) -> Result<()> {
    require(
        at.checked_add(raw.len() as u64)
            .is_some_and(|end| end <= source.len()),
        "gfs2: source range/truncation",
    )?;
    source.read_at(at, raw)
}
fn meta(raw: &[u8], kind: u32, format: u32) -> Result<()> {
    require(
        u32be(raw, 0)? == 0x0116_1970 && u32be(raw, 4)? == kind && u32be(raw, 16)? == format,
        "gfs2: metadata identity/format",
    )
}
fn disk_hash(raw: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in raw {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}
#[cfg(test)]
mod large_tests;
#[cfg(test)]
mod native_tests;
#[cfg(test)]
mod tests;
