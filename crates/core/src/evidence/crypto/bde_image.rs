//! Pure Rust BitLocker adapter over a bounded view of the decoded evidence.
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Weak;

/// Public metadata only; key entry payloads never cross IPC.
#[derive(Debug, serde::Serialize)]
pub struct BdeMetadataInfo {
    pub version: u16,
    pub bytes_per_sector: u16,
    pub volume_guid: String,
    /// Decimal FILETIME preserves all 64 bits across the JavaScript boundary.
    pub creation_time: String,
    pub description: Option<String>,
    pub encryption_method: u16,
    pub protector_types: Vec<u16>,
    pub key_protectors: Vec<BdeProtectorInfo>,
    pub encrypted_volume_size: u64,
    pub used_space_only: bool,
}

#[derive(Debug, serde::Serialize)]
pub struct BdeProtectorInfo {
    pub identifier: String,
    pub protection_type: u16,
}

use anyhow::{ensure, Context};

/// Supplies encrypted bytes from an image without owning it. Implementers must
/// honor the requested range and propagate read failures to the caller.
pub trait VolumeByteReader: Send + Sync {
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> anyhow::Result<usize>;
}

pub enum BdeUnlockKey<'a> {
    RecoveryPassword(&'a str),
    Clear,
    StartupKey(&'a [u8]),
    Vmk(&'a [u8]),
    Keys { fvek: &'a [u8], tweak: &'a [u8] },
    Password(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BdeEncryptionMethod {
    None,
    Aes128CbcDiffuser,
    Aes256CbcDiffuser,
    Aes128Cbc,
    Aes256Cbc,
    Aes128Xts,
    Aes256Xts,
    Other(u16),
}

impl BdeEncryptionMethod {
    fn from_raw(value: u16) -> Self {
        match value {
            0 => Self::None,
            0x8000 => Self::Aes128CbcDiffuser,
            0x8001 => Self::Aes256CbcDiffuser,
            0x8002 => Self::Aes128Cbc,
            0x8003 => Self::Aes256Cbc,
            0x8004 => Self::Aes128Xts,
            0x8005 => Self::Aes256Xts,
            other => Self::Other(other),
        }
    }
}

pub struct BdeVolume {
    volume: ::bitlocker::DecryptedVolume<VolumeReader>,
    source: Weak<dyn VolumeByteReader>,
}

impl std::fmt::Debug for BdeVolume {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BdeVolume")
            .field("size", &self.size())
            .field("encryption_method", &self.encryption_method())
            .finish_non_exhaustive()
    }
}

impl BdeVolume {
    pub fn read_metadata_io(
        reader: Weak<dyn VolumeByteReader>,
        offset: u64,
        length: u64,
    ) -> anyhow::Result<BdeMetadataInfo> {
        offset
            .checked_add(length)
            .context("BitLocker slice offset overflow")?;
        let mut reader = VolumeReader {
            reader,
            offset,
            length,
            position: 0,
        };
        let mut boot = [0; 512];
        reader.read_exact(&mut boot)?;
        let header = ::bitlocker::VolumeHeader::parse(&boot)?;
        let metadata = ::bitlocker::BitLockerVolume::read_metadata(&mut reader)?;
        Ok(BdeMetadataInfo {
            version: metadata.version,
            bytes_per_sector: header.bytes_per_sector,
            volume_guid: ::bitlocker::format_guid(&metadata.volume_guid),
            creation_time: metadata.creation_time.to_string(),
            description: metadata.description()?,
            encryption_method: metadata.encryption_method,
            protector_types: metadata.protector_types(),
            key_protectors: metadata
                .vmk_entries()
                .filter_map(|entry| {
                    let identifier: [u8; 16] = entry.data.get(..16)?.try_into().ok()?;
                    Some(BdeProtectorInfo {
                        identifier: ::bitlocker::format_guid(&identifier),
                        protection_type: entry.protection_type()?,
                    })
                })
                .collect(),
            encrypted_volume_size: metadata.encrypted_volume_size,
            used_space_only: header.variant == ::bitlocker::BdeVariant::UsedDiskSpaceOnly,
        })
    }

    pub fn open_with_key_io(
        reader: Weak<dyn VolumeByteReader>,
        offset: u64,
        length: u64,
        key: BdeUnlockKey<'_>,
    ) -> anyhow::Result<Self> {
        ensure!(
            length >= 512 && length.is_multiple_of(512),
            "invalid BitLocker volume length"
        );
        offset
            .checked_add(length)
            .context("BitLocker slice offset overflow")?;
        let source = reader.clone();
        let mut reader = VolumeReader {
            reader,
            offset,
            length,
            position: 0,
        };
        let mut header = [0; 512];
        reader.read_exact(&mut header)?;
        let header = ::bitlocker::VolumeHeader::parse(&header)?;
        ensure!(
            length.is_multiple_of(u64::from(header.bytes_per_sector)),
            "BitLocker length is not sector aligned"
        );
        reader.seek(SeekFrom::Start(0))?;
        let volume = match key {
            BdeUnlockKey::Password(password) => {
                ::bitlocker::BitLockerVolume::unlock_with_password(reader, password)
            }
            BdeUnlockKey::RecoveryPassword(password) => {
                ::bitlocker::BitLockerVolume::unlock_with_recovery_password(reader, password)
            }
            BdeUnlockKey::Clear => ::bitlocker::BitLockerVolume::unlock_clear_key(reader),
            BdeUnlockKey::StartupKey(key) => {
                ::bitlocker::BitLockerVolume::unlock_with_startup_key(reader, key)
            }
            BdeUnlockKey::Vmk(key) => ::bitlocker::BitLockerVolume::unlock_with_vmk(reader, key),
            BdeUnlockKey::Keys { fvek, tweak } => {
                ::bitlocker::BitLockerVolume::unlock_with_keys(reader, fvek, tweak)
            }
        }
        .context("BitLocker key derivation or authenticated key unwrap failed")?;
        Ok(Self { volume, source })
    }

    pub fn size(&self) -> u64 {
        self.volume.volume_size()
    }
    pub fn encryption_method(&self) -> BdeEncryptionMethod {
        BdeEncryptionMethod::from_raw(self.volume.metadata().encryption_method)
    }
    pub fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> anyhow::Result<usize> {
        // Hold the source only for this call, including format-defined zero regions.
        let _source = self
            .source
            .upgrade()
            .context("BitLocker source image is closed")?;
        let count = self.size().saturating_sub(offset).min(buffer.len() as u64) as usize;
        if count == 0 {
            return Ok(0);
        }
        self.volume.read_at(offset, &mut buffer[..count])?;
        Ok(count)
    }
}

/// The weak reference avoids an image → unlocked volume → image ownership cycle.
struct VolumeReader {
    reader: Weak<dyn VolumeByteReader>,
    offset: u64,
    length: u64,
    position: u64,
}

impl Read for VolumeReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self
            .length
            .saturating_sub(self.position)
            .min(buffer.len() as u64) as usize;
        if count == 0 {
            return Ok(0);
        }
        let source = self
            .reader
            .upgrade()
            .ok_or_else(|| io::Error::other("BitLocker source image is closed"))?;
        let offset = self
            .offset
            .checked_add(self.position)
            .ok_or_else(|| io::Error::other("BitLocker source offset overflow"))?;
        let read = source
            .read_at(offset, &mut buffer[..count])
            .map_err(io::Error::other)?;
        if read > count {
            return Err(io::Error::other("BitLocker source returned too many bytes"));
        }
        self.position += read as u64;
        Ok(read)
    }
}

impl Seek for VolumeReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let position = match position {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
            SeekFrom::End(delta) => i128::from(self.length) + i128::from(delta),
        };
        self.position = u64::try_from(position)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid BitLocker seek"))?;
        Ok(self.position)
    }
}

#[cfg(test)]
#[path = "bde_image_tests.rs"]
mod tests;
