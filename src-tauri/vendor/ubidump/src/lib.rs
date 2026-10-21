//! Bounded, read-only UBI volume reconstruction. See LOVELYCARVE.md for scope.
mod blocks;
pub mod error;
pub mod ubifs;
pub mod utils;
mod volume;

pub use blocks::UbiImage;
pub use volume::{ReadAt, UbiReader, UbiVolume};

#[cfg(test)]
mod tests;
