//! evidence domain modules.

pub mod crypto;
pub mod image;
pub mod partitions;
pub mod physical_disk;
pub mod raid;
pub mod readers;
pub mod vss_image;
#[cfg(test)]
pub(crate) mod vss_image_tests;
