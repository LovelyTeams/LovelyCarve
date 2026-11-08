//! # filevault — Apple CoreStorage / FileVault 2 (FVDE) reader & decryptor
//!
//! A panic-free, bounds-checked decryptor for CoreStorage / FileVault 2 volumes
//! (macOS 10.7–10.15, AES-XTS-128, password protector). Parses the physical
//! volume header, decrypts the CoreStorage metadata, derives the volume key
//! hierarchy from a password (PBKDF2-HMAC-SHA256 → RFC 3394 AES-KW → SHA-256
//! tweak key, all RustCrypto — never hand-rolled), and exposes the plaintext
//! logical volume as a `Read + Seek` stream.
//!
//! APFS-native encryption (10.13+) is a separate format and is out of scope.
//!
//! ```no_run
//! use std::fs::File;
//! use filevault::FileVaultVolume;
//!
//! let image = File::open("cs_partition.raw")?;
//! let mut volume = FileVaultVolume::unlock_with_password(image, "fvde-TEST")?;
//! let mut sector = [0u8; 512];
//! volume.read_at(0, &mut sector)?;
//! # Ok::<(), filevault::FileVaultError>(())
//! ```

#![forbid(unsafe_code)]
// `doc_markdown` false-positives on the domain proper nouns that pervade these
// docs (CoreStorage, FileVault, FVDE, PBKDF2, RustCrypto, UserType, …).
#![allow(clippy::doc_markdown)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::io::{Read, Seek, SeekFrom};

pub mod context;
pub mod error;
pub mod metadata;
pub mod read;
#[cfg(any(test, feature = "testutil"))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub mod test_support;
pub mod unlock;
#[cfg(feature = "vfs")]
pub mod vfs;
#[cfg(feature = "vfs")]
pub use vfs::FileVaultLayer;
pub mod storage;
mod storage_metadata;
mod storage_plist;
pub mod volume;
pub mod volume_header;
pub mod xts;

pub use context::{EncryptionContext, LogicalVolumeInfo, Protector, ProtectorKind};
pub use error::FileVaultError;
pub use volume::DecryptedVolume;
pub use volume_header::VolumeHeader;

/// Password-independent metadata parsed from a CoreStorage volume.
///
/// Everything here is derivable WITHOUT the password, so `filevault-forensic`
/// can audit protector inventory, encryption state, and KDF strength on a locked
/// volume. Producing it never unwraps any key.
#[derive(Debug, Clone)]
pub struct FileVaultInfo {
    /// Physical volume identifier (UUID string).
    pub physical_volume_identifier: String,
    /// PBKDF2 iteration count of the password protector.
    pub pbkdf2_iterations: u32,
    /// PBKDF2 salt (16 bytes).
    pub pbkdf2_salt: [u8; 16],
    /// Logical volume family UUID (string).
    pub family_uuid: String,
    /// Logical volume identifier UUID (string), if present.
    pub lv_identifier: Option<String>,
    /// Logical volume name, if present.
    pub lv_name: Option<String>,
    /// Logical volume size in bytes.
    pub lv_size: u64,
    /// Encryption method label (always AES-XTS-128 for a supported volume).
    pub encryption_method: &'static str,
    /// LV conversion status (`Complete` / `Converting` / …), if present.
    pub conversion_status: Option<String>,
    /// Protectors (crypto users) present.
    pub protectors: Vec<Protector>,
}

/// An unlocked CoreStorage / FileVault volume presenting the decrypted logical
/// volume as a `Read + Seek` stream.
#[derive(Debug)]
pub struct FileVaultVolume<R: Read + Seek> {
    decrypted: DecryptedVolume<R>,
    info: FileVaultInfo,
}

impl<R: Read + Seek> FileVaultVolume<R> {
    /// Parse, decrypt the metadata, derive the key hierarchy from `password`,
    /// and return an unlocked volume.
    ///
    /// # Errors
    /// Any [`FileVaultError`] from parsing (not CoreStorage, unsupported method,
    /// missing metadata) or key derivation (wrong password → `KeyUnwrap`).
    pub fn unlock_with_password(reader: R, password: &str) -> Result<Self, FileVaultError> {
        let container = storage::Container::open(reader, None)?;
        if container.volumes().count() != 1 {
            return Err(storage_metadata::invalid(
                "multiple logical volumes require explicit selection",
            ));
        }
        container.unlock_with_password(0, password)
    }

    /// Read and decrypt `buf.len()` bytes at logical `offset` (see
    /// [`DecryptedVolume::read_at`]).
    ///
    /// # Errors
    /// [`FileVaultError::Io`] if the underlying read fails.
    pub fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, FileVaultError> {
        self.decrypted.read_at(offset, buf)
    }

    /// The logical volume size in bytes.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.decrypted.size()
    }

    /// The password-independent parsed metadata.
    #[must_use]
    pub fn info(&self) -> &FileVaultInfo {
        &self.info
    }

    /// Consume the volume, returning the inner decrypted `Read + Seek` stream.
    #[must_use]
    pub fn into_decrypted(self) -> DecryptedVolume<R> {
        self.decrypted
    }
}

impl<R: Read + Seek> Read for FileVaultVolume<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.decrypted.read(buf)
    }
}

impl<R: Read + Seek> Seek for FileVaultVolume<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.decrypted.seek(pos)
    }
}

/// Parse only the password-independent metadata from a reader, WITHOUT the
/// password — the entry point `filevault-forensic` uses to audit a locked volume.
///
/// # Errors
/// Any [`FileVaultError`] from header/metadata parsing.
pub fn parse_info<R: Read + Seek>(reader: R) -> Result<FileVaultInfo, FileVaultError> {
    let container = storage::Container::open(reader, None)?;
    if container.volumes().count() != 1 {
        return Err(storage_metadata::invalid(
            "multiple logical volumes require explicit selection",
        ));
    }
    let info = container
        .volumes()
        .next()
        .ok_or_else(|| storage_metadata::invalid("logical volume missing"))?
        .info
        .clone();
    Ok(info)
}

/// Assemble the password-independent [`FileVaultInfo`].
fn build_info(
    header: &VolumeHeader,
    context: &EncryptionContext,
    lv: &LogicalVolumeInfo,
    size: u64,
) -> FileVaultInfo {
    FileVaultInfo {
        physical_volume_identifier: format_uuid(&header.physical_volume_identifier),
        pbkdf2_iterations: context.iterations,
        pbkdf2_salt: context.salt,
        family_uuid: lv.family_uuid.clone(),
        lv_identifier: lv.lv_identifier.clone(),
        lv_name: lv.name.clone(),
        lv_size: size,
        encryption_method: "AES-XTS-128",
        conversion_status: context.conversion_status.clone(),
        protectors: context.protectors.clone(),
    }
}

/// Format 16 on-disk UUID bytes as the canonical `8-4-4-4-12` string.
fn format_uuid(bytes: &[u8; 16]) -> String {
    uuid::Uuid::from_bytes(*bytes).hyphenated().to_string()
}

/// Parse a UUID string to its 16 canonical bytes (as used for the tweak key).
#[cfg(test)]
fn parse_uuid_bytes(text: &str) -> Option<[u8; 16]> {
    uuid::Uuid::parse_str(text).ok().map(|u| *u.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::test_support::{build_image, BLOCK, FAMILY, ITERS, PASSWORD, PV_ID};
    use super::*;
    use std::io::Cursor;

    #[test]
    fn unlock_synthetic_volume_end_to_end() {
        let image = build_image();
        let mut vol = FileVaultVolume::unlock_with_password(Cursor::new(image), PASSWORD).unwrap();
        assert_eq!(vol.size(), 0x4000);
        assert_eq!(vol.info().pbkdf2_iterations, ITERS);
        assert_eq!(vol.info().lv_name.as_deref(), Some("SynthLV"));
        assert_eq!(vol.info().encryption_method, "AES-XTS-128");
        assert_eq!(vol.info().conversion_status.as_deref(), Some("Complete"));
        assert_eq!(vol.info().protectors.len(), 1);

        // Decrypt LV offset 0 -> known plaintext (0,1,2,...).
        let mut buf = [0u8; 512];
        assert_eq!(vol.read_at(0, &mut buf).unwrap(), 512);
        let expected: Vec<u8> = (0..512).map(|i| (i & 0xff) as u8).collect();
        assert_eq!(&buf[..], &expected[..]);

        // Read + Seek passthrough.
        assert_eq!(vol.seek(SeekFrom::Start(512)).unwrap(), 512);
        let mut b2 = [0u8; 16];
        assert!(std::io::Read::read(&mut vol, &mut b2).unwrap() > 0);

        // parse_info (no password) matches.
        let image2 = build_image();
        let info = parse_info(Cursor::new(image2)).unwrap();
        assert_eq!(info.family_uuid, vol.info().family_uuid);
        assert_eq!(info.lv_size, 0x4000);
        assert_eq!(
            info.physical_volume_identifier,
            uuid::Uuid::from_bytes(PV_ID).hyphenated().to_string()
        );

        // into_decrypted exposes the stream.
        let mut dec = vol.into_decrypted();
        assert_eq!(dec.size(), 0x4000);
        let mut b3 = [0u8; 16];
        assert_eq!(dec.read_at(0, &mut b3).unwrap(), 16);
    }

    #[test]
    fn unlock_wrong_password_fails() {
        let image = build_image();
        assert!(matches!(
            FileVaultVolume::unlock_with_password(Cursor::new(image), "nope"),
            Err(FileVaultError::KeyUnwrap { .. })
        ));
    }

    #[test]
    fn unlock_non_corestorage_fails() {
        let image = vec![0u8; 4096];
        assert!(matches!(
            FileVaultVolume::unlock_with_password(Cursor::new(image), PASSWORD),
            Err(FileVaultError::NotCoreStorage { .. })
        ));
    }

    #[test]
    fn truncated_image_errors_loudly() {
        // A header-only image: metadata read runs past EOF -> Io error, not empty.
        let mut image = vec![0u8; 600];
        image[88] = b'C';
        image[89] = b'S';
        image[96..100].copy_from_slice(&(BLOCK as u32).to_le_bytes());
        image[172..176].copy_from_slice(&2u32.to_le_bytes());
        image[104..112].copy_from_slice(&1u64.to_le_bytes());
        let err = parse_info(Cursor::new(image)).unwrap_err();
        assert!(matches!(
            err,
            FileVaultError::OutOfRange { .. } | FileVaultError::Io(_)
        ));
    }

    #[test]
    fn zero_block_size_header_errors() {
        // block_size 0 in the header -> decrypt_metadata_region OutOfRange.
        let mut image = vec![0u8; 4096];
        image[88] = b'C';
        image[89] = b'S';
        image[96..100].copy_from_slice(&0u32.to_le_bytes());
        image[172..176].copy_from_slice(&2u32.to_le_bytes());
        let err = parse_info(Cursor::new(image)).unwrap_err();
        assert!(matches!(err, FileVaultError::OutOfRange { .. }));
    }

    #[test]
    fn format_and_parse_uuid_roundtrip() {
        let s = format_uuid(&FAMILY);
        assert_eq!(parse_uuid_bytes(&s), Some(FAMILY));
        assert_eq!(parse_uuid_bytes("not-a-uuid"), None);
    }
}
