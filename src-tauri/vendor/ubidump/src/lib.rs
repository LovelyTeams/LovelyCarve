//! Bounded, read-only UBI volume reconstruction. See LOVELYCARVE.md for scope.
mod blocks;
pub mod error;
pub mod filesystem;
pub mod ubifs;
pub mod utils;
mod volume;

pub use blocks::UbiImage;
pub use filesystem::{DirectoryEntry, FileType, Inode, UbifsImage};
pub use volume::{ReadAt, UbiReader, UbiVolume};

#[cfg(test)]
mod tests;
