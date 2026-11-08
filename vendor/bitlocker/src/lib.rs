//! # bitlocker-core — a from-scratch BitLocker (BDE) reader and decryptor
//!
//! Parses the BitLocker Drive Encryption on-disk format (the `-FVE-FS-` /
//! BitLocker To Go volume header, the FVE metadata block, and its key-protector
//! entries) and decrypts a volume from a password or recovery password, exposing
//! a plaintext `Read + Seek` view.
//!
//! LovelyCarve adds all six CBC/diffuser/XTS methods, variable logical sector
//! sizes, Vista boot restoration, used-space-only EOW maps and direct VMK/FVEK
//! keys. Password, recovery password, clear-key and BEK protectors are supported;
//! TPM and SID protectors remain unsupported. See `LOVELYCARVE.md` and
//! `../../../docs/formats/BITLOCKER.md` for the comparison and validation boundaries.
//!
//! Every primitive comes from an audited crate — `aes`, `cbc`, `ccm`, `sha2`,
//! and `xts-mode` for the XTS methods — except the Elephant Diffuser, for which
//! no ecosystem crate exists; it lives in our own [`elephant_diffuser`] crate
//! (extracted from here) and is validated **in situ** by this repo's Tier-1
//! oracle (never a self-authored round-trip, which would prove nothing).
//!
//! ```no_run
//! use std::fs::File;
//! use bitlocker::BitLockerVolume;
//!
//! let image = File::open("bdetogo.raw")?;
//! let mut volume = BitLockerVolume::unlock_with_password(image, "bde-TEST")?;
//! let mut boot = [0u8; 512];
//! volume.read_at(0, &mut boot)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod bytes;
mod crypto;
mod eow;
mod error;
mod guid;
mod header;
mod metadata;
mod method;
#[cfg(feature = "vfs")]
pub mod vfs;
#[cfg(feature = "vfs")]
pub use vfs::BitlockerLayer;
mod volume;

pub use error::{BdeError, Result};
pub use guid::format_guid;
pub use header::{BdeVariant, VolumeHeader};
pub use metadata::{FveMetadata, MetadataEntry};
pub use volume::{BitLockerVolume, DecryptedVolume};
