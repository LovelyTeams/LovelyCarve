//! # lamfold-romfs — the romfs frontend of the lamfold read-only media stack.
//!
//! Clean-room reader for Linux romfs (the `-rom1fs-` format): the minimal,
//! uncompressed read-only filesystem used in tiny embedded and early-boot
//! images. It is the floor of the flock — no compression, no block index, just
//! a big-endian chain of 16-byte-aligned file headers.
//!
//! Reads over a lamfold [`lamfold::BlockSource`] and implements
//! [`lamfold::FoldFrontend`]; composes through `dispatch_fs_over_source` like
//! every other frontend.
//!
//! Scope: the full romfs read path — header, the directory next-chain, regular
//! files, and symlinks (`read_link`). romfs has no compression, owners, or
//! timestamps to surface.
//!
//! Derived only from the public format (kernel
//! `Documentation/filesystems/romfs.rst`); the GPL-2 `fs/romfs` driver is fenced
//! off. Note: `neotron-romfs` is a *different, incompatible* format and a GPL-3
//! sibling — deliberately not consulted.

#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod romfs;

pub use romfs::Romfs;
