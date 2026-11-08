//! Read-only UDF 1.02–2.01 physical partitions, adapted from lamfold-udf.
//! Descriptor checksums, CRCs, partition references and logical lengths are
//! validated before use. Unsupported partition maps fail explicitly.
#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;
mod descriptors;
mod inode;
#[cfg(test)]
mod tests;
mod udf;
pub use descriptors::{anchor_locations, valid_anchor, BLOCK_SIZES};
pub use udf::Udf;
