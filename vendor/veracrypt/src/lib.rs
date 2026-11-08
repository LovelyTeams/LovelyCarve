//! # veracrypt-core — pure-Rust VeraCrypt/TrueCrypt reader and decryptor
//!
//! Brute the volume header's PRF + cipher from a password, recover the master
//! key, and decrypt the data area. Panic-free and `forbid(unsafe)`; every crypto
//! primitive is an audited RustCrypto crate.
//!
//! ```no_run
//! use std::fs::File;
//! let mut vol = veracrypt::VeraVolume::unlock_with_password(
//!     File::open("container.vc")?,
//!     b"passphrase",
//! )?;
//! let mut first = [0u8; 512];
//! vol.read_at(0, &mut first)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Correctness is validated against `cryptsetup --veracrypt` on a real VeraCrypt
//! volume with a published password (Tier-1); see `docs/validation.md`.

#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::trivially_copy_pass_by_ref,
        clippy::seek_from_current
    )
)]

mod crypto;
mod error;
mod header;
#[cfg(feature = "vfs")]
pub mod vfs;
mod volume;

pub use crypto::{Cipher, Prf};
pub use error::{Result, VeraError};
pub use header::{Flavor, VeraHeader};
pub use volume::{DecryptedVolume, VeraVolume, VolumeInfo};
