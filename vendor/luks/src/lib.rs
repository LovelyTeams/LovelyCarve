//! # luks-core — pure-Rust LUKS reader and decryptor
//!
//! Parse a LUKS container's on-disk header, recover the master key from a
//! passphrase or verified master key, and decrypt the payload. `forbid(unsafe)`: every
//! integer is read bounds-checked, every crypto primitive is an audited
//! RustCrypto crate.
//!
//! ```no_run
//! use std::fs::File;
//! let mut vol = luks::LuksVolume::unlock1_with_passphrase(
//!     File::open("container.luks")?,
//!     b"passphrase",
//! )?;
//! let mut first = [0u8; 512];
//! vol.read_at(0, &mut first)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Correctness is validated against `cryptsetup` on real containers (Tier-2);
//! see `docs/validation.md`.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod af;
mod bytes;
mod credentials;
mod crypto;
mod error;
mod header;
mod header2;
mod sector_cipher;
#[cfg(feature = "vfs")]
pub mod vfs;
#[cfg(feature = "vfs")]
pub use vfs::LuksLayer;
mod volume;

pub use error::{LuksError, Result};
pub use header::{Keyslot, Luks1Header};
pub use header2::{Luks2Digest, Luks2Header, Luks2Kdf, Luks2Keyslot, Luks2Segment};
pub use sector_cipher::validate_cipher;
pub use volume::{DecryptedPayload, LuksVolume, VolumeInfo};

/// Fuzz-only re-export (hidden from docs). Kept `pub` so the `core/fuzz` crate
/// can drive the anti-forensic merge over arbitrary bytes directly, alongside
/// the header parsers and the full unlock pipeline.
#[doc(hidden)]
pub use af::merge as af_merge;
