//! # lamfold-erofs — the EROFS frontend of the lamfold read-only media stack.
//!
//! Clean-room EROFS (Enhanced Read-Only File System) reader. EROFS is the
//! forward standard for immutable images — the metadata format **composefs**
//! builds on for content-addressed OS and OCI images — so this frontend is where
//! the lamfold stack meets the shepherd: read the metadata here, anchor trust in
//! [`lamfold`]'s native fs-verity Merkle (`the lamfold design spec` §7).
//!
//! Reads over a lamfold [`lamfold::BlockSource`] and implements
//! [`lamfold::FoldFrontend`]; with a [`lamfold::MerkleVerifier`] in the
//! [`lamfold::SubstrateCtx`] the read path verifies every data block against the
//! trusted leaf layer and refuses tampered media.
//!
//! Scope: compact and extended inodes, plain and inline files, directories,
//! symlinks, FULL / COMPACT compressed indexes (LZ4, MicroLZMA, DEFLATE, Zstd),
//! and four-byte chunk maps. Unsupported advanced layouts fail explicitly.
//! LovelyCarve's bounded integration, local changes and verification limits are
//! described in `LOVELYCARVE.md`; the host does not enable the verifier feature.
//!
//! Derived only from the public EROFS on-disk format — the kernel header
//! `fs/erofs/erofs_fs.h` is itself **SPDX MIT**, so it is referenced directly;
//! the GPL-2 `fs/erofs` *driver* is fenced off (never read or copied).

#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod erofs;

pub use erofs::Erofs;

#[cfg(test)]
extern crate self as lamfold_erofs;
#[cfg(test)]
#[path = "../tests/erofs_base.rs"]
mod tests_base;
#[cfg(test)]
#[path = "../tests/erofs_chunks.rs"]
mod tests_chunks;
#[cfg(test)]
#[path = "../tests/erofs_compact.rs"]
mod tests_compact;
#[cfg(test)]
#[path = "../tests/erofs_compressed.rs"]
mod tests_compressed;
#[cfg(test)]
#[path = "../tests/erofs_mkfs19.rs"]
mod tests_mkfs19;
#[cfg(test)]
#[path = "../tests/erofs_path.rs"]
mod tests_path;
