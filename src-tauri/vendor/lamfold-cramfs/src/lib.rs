//! # lamfold-cramfs — the cramfs frontend of the lamfold read-only media stack.
//!
//! Clean-room reader for cramfs (the Compressed ROM filesystem): the small,
//! per-page-zlib read-only format long used for embedded firmware and rescue
//! images. Like squashfs it compresses, but with a far terser structure — a
//! 12-byte bitfield inode and a flat per-page block index — which is exactly why
//! it stays useful where squashfs is too heavy.
//!
//! Reads over a lamfold [`lamfold::BlockSource`] and implements
//! [`lamfold::FoldFrontend`]; every compressed page is inflated through the
//! shared substrate codec (`Codec::Zlib`), so this frontend carries only the
//! on-disk structure.
//!
//! Reads FSID v1 and v2 volumes, including big-endian images, boot padding
//! and extended pointers to raw or compressed pages.
//!
//! Derived only from the public format (kernel
//! `Documentation/filesystems/cramfs.rst` + `cramfs_fs.h`); the GPL-2
//! `fs/cramfs` driver is fenced off. The upstream `cramfs` crate is unbuildable,
//! so there was nothing permissive to reuse.

#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod cramfs;

pub use cramfs::{superblock_offset, Cramfs};
