//! `ios-backup-core` — native, read-only, panic-free reader for iOS device
//! backups (Finder / iTunes / `MobileSync`), encrypted ones included.
//!
//! A backup is a directory named for the device UDID holding:
//!
//! * `Manifest.plist` — the bag of metadata: `IsEncrypted`, `BackupKeyBag`,
//!   `ManifestKey`, version and date;
//! * `Manifest.db` — a `SQLite` index with one `Files` row per captured file,
//!   itself AES-encrypted when the backup is;
//! * `Status.plist` / `Info.plist` — backup state and device metadata;
//! * content blobs at `<first-2-hex-of-fileID>/<fileID>`, where
//!   `fileID = SHA1(domain + "-" + relativePath)`.
//!
//! This crate exposes that tree and reads file bytes, decrypting on demand. It
//! emits **no** findings — auditing a backup is `ios-backup-forensic`'s job
//! (ADR-0008, reader/analyzer split).
//!
//! Nothing here writes to disk: an encrypted `Manifest.db` is decrypted into
//! memory and handed to the `SQLite` reader as bytes, so a decrypted copy of the
//! evidence never lands in a temporary file.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod backup;
pub mod credentials;
pub mod crypto;
pub mod error;
pub mod keybag;
pub mod logical;
pub mod manifest;
pub mod metadata;
pub mod nskeyed;
#[cfg(feature = "vfs")]
pub mod vfs;

pub use backup::{Backup, EncryptionState};
pub use credentials::{Credentials, Password};
pub use error::Error;
pub use manifest::{BackupFile, FileKind, ManifestFiles};
pub use metadata::BackupMetadata;
