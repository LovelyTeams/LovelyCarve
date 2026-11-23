//! # veracrypt-core — VeraCrypt and TrueCrypt volumes in pure Rust
//!
//! Unlock a VeraCrypt or TrueCrypt volume from its password, PIM and
//! keyfiles, and read its plaintext. The header search follows VeraCrypt's
//! `ReadVolumeHeader`: every PRF (SHA-512, SHA-256, BLAKE2s-256, Whirlpool,
//! Streebog, RIPEMD-160, Argon2id) at VeraCrypt's and TrueCrypt's iteration
//! counts, every cipher and cascade in XTS mode, the normal, hidden, backup
//! and system (pre-boot) header locations. Derivations run on all cores and
//! stop on request; reads take `&self` and decrypt large spans in parallel.
//!
//! ```no_run
//! use veracrypt::{unlock, Control, FileSource, HeaderSelection, UnlockRequest};
//! let volume = unlock(
//!     FileSource::open("container.hc")?,
//!     &UnlockRequest { password: b"passphrase", pim: 0, prf: None, headers: HeaderSelection::Auto },
//!     &Control { cancelled: &|| false, progress: None, threads: None },
//! )?;
//! let mut sector = [0u8; 512];
//! volume.read_at(0, &mut sector)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod cipher;
mod error;
mod header;
pub mod kdf;
pub mod keyfile;
mod layout;
mod search;
mod source;
mod volume;
mod xts;

pub use cipher::{Cipher, CipherChain};
pub use error::{Result, VeraError};
pub use header::{Flavor, FLAG_IN_PLACE, FLAG_SYSTEM};
pub use kdf::{argon2_cost, Family, Kdf, Prf, MAX_BOOT_PIM, MAX_PIM};
pub use keyfile::KeyfilePool;
pub use layout::{HeaderKind, HeaderSelection, SystemRange, SYSTEM_HEADER_OFFSET};
pub use search::{Control, Progress};
pub use source::{FileSource, ReadAt, SeekSource};
pub use volume::{unlock, UnlockRequest, Volume, VolumeInfo, AUTO_ARGON2_MAX_PASSES};
pub use xts::DATA_UNIT;

#[cfg(test)]
mod volume_tests;
