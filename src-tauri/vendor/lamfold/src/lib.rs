//! # lamfold — read-only media filesystem stack (substrate core)
//!
//! `lamfold` is an **immutable-media filesystem stack**: a shared `no_std`
//! substrate under thin, clean-room format frontends (the *flock*). This crate
//! is the substrate (L2) — the shared engine every frontend sits on:
//!
//! * `codec` — the decompression-codec registry (deflate/lz4 wired; zstd/xz/lzo
//!   declared), one decoder shared across all compressed formats ([`decode`],
//!   [`lz4_block_with_dict`]).
//! * `stream` — whole-stream decoding for data of unknown size, such as a
//!   compressed initramfs member ([`decode_stream`], [`StreamCodec`]).
//! * [`BlockCache`] — an immutable decompressed-block LRU (read-only ⇒ no
//!   invalidation).
//! * `read_cap` — bounded-allocation hardening (no OOM on a hostile size;
//!   [`checked_full_read_len`], [`checked_block_len`]).
//! * [`FoldFrontend`] — the trait the flock implements; [`Verifier`] — the
//!   shepherd (integrity-verification seam).
//! * [`BlockSource`] — the byte source a frontend reads over (LamBoot adapts its
//!   own `BlockSource` to this at integration).
//!
//! Frontends (`lamfold-iso`, `-udf`, `-squash`, `-erofs`, …) live in their own
//! crates and depend on this one. See `the lamfold design spec`.
//!
//! Read-only by construction; `#![forbid(unsafe_code)]` in the substrate —
//! `zerocopy` removes the transmute class, so the parse layer needs no `unsafe`.

#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod cache;
mod codec;
mod error;
mod frontend;
mod path;
mod read_cap;
mod source;
mod stream;
mod verify;
#[cfg(feature = "verify")]
mod verity;

pub use cache::BlockCache;
pub use codec::{decode, lz4_block_with_dict, microlzma_block_decode, Codec};
pub use error::{FoldError, Result};
pub use frontend::{DirEntry, FileKind, FoldFrontend, Metadata, NodeId, SubstrateCtx};
pub use path::{metadata_path, read_dir_path, read_path, resolve, MAX_SYMLINKS};
pub use read_cap::{
    checked_block_len, checked_full_read_len, MAX_BOOT_FILE_BYTES, MAX_DECOMPRESSED_BLOCK_BYTES,
};
pub use source::{BlockSource, SliceSource};
pub use stream::{decode_stream, StreamCodec};
pub use verify::{NoVerifier, Verifier};
#[cfg(feature = "verify")]
pub use verity::{fsverity_digest_sha256, MerkleVerifier, Sha256Digest, DEFAULT_BLOCK_LOG};
