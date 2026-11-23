//! BitLocker volumes read directly from decoded evidence through a pure Rust
//! reader: boot-sector recognition and the BitLocker part of the shared unlock
//! in `encrypted_image`.
use std::fs::File;
use std::io::Read;
use std::sync::{Arc, Mutex, Weak};

use anyhow::{anyhow, ensure, Context};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use crate::core::common::error::Failure;
use crate::core::evidence::crypto::bde_image::{
    BdeEncryptionMethod, BdeUnlockKey, BdeVolume, VolumeByteReader,
};
use crate::core::evidence::crypto::bde_vmk;
use crate::core::evidence::crypto::credential::UnlockCredential;
use crate::core::evidence::image::DiskImage;

const FVE_FS_SIGNATURE: &[u8; 8] = b"-FVE-FS-";
const TO_GO_SIGNATURE: &[u8; 8] = b"MSWIN4.1";
/// Largest startup key (`.BEK`) file accepted.
const STARTUP_KEY_LIMIT: u64 = 64 * 1024;
/// Longest FVEK or TWEAK hex accepted, whitespace included.
const KEY_HEX_LIMIT: usize = 256;

pub fn is_bitlocker_boot_sector_bytes(bytes: &[u8]) -> bool {
    if bytes.len() < 11 {
        return false;
    }
    let oem = &bytes[3..11];
    // Vista / Win7 lay down `-FVE-FS-` directly. BitLocker To Go uses the
    // standard FAT OEM string and stores the BitLocker volume header GUID at
    // bytes 424..440 of the first sector.
    if oem == FVE_FS_SIGNATURE {
        return true;
    }
    if oem == TO_GO_SIGNATURE && bytes.len() >= 440 {
        const TO_GO_GUID: [u8; 16] = [
            0x3b, 0xd6, 0x67, 0x49, 0x29, 0x2e, 0xd8, 0x4a, 0x83, 0x99, 0xf6, 0xa3, 0x39, 0xe3,
            0xd0, 0x01,
        ];
        if bytes[424..440] == TO_GO_GUID {
            return true;
        }
    }
    false
}

/// Encrypted bytes of one BitLocker volume, read from the evidence until the
/// unlock, or the plaintext view built on it, is cancelled. The source is held
/// weakly, so an unlocked view never keeps closed evidence alive.
pub(super) struct Ciphertext {
    source: Weak<DiskImage>,
    cancellation: Arc<Mutex<CancellationToken>>,
}

impl VolumeByteReader for Ciphertext {
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> anyhow::Result<usize> {
        let cancelled = self
            .cancellation
            .lock()
            .map_err(|_| anyhow!("encrypted cancellation lock poisoned"))?
            .is_cancelled();
        ensure!(
            !cancelled,
            Failure::cancelled("encrypted volume operation cancelled")
        );
        let source = self.source.upgrade().context("encrypted evidence closed")?;
        // The evidence refuses to hand synthesized (unreadable) bytes to a decryptor.
        VolumeByteReader::read_at(source.as_ref(), offset, buffer)
    }
}

/// An unlocked BitLocker volume and what its unlock learned.
pub(super) struct UnlockedBitlocker {
    pub(super) volume: BdeVolume,
    /// Keeps alive the reader `volume` holds weakly.
    pub(super) ciphertext: Arc<Ciphertext>,
    pub(super) cipher: String,
    pub(super) sector_size: usize,
    /// The recovery password a VMK credential yielded, when it yields one.
    pub(super) recovery_password: Option<String>,
}

/// Open the BitLocker volume at `[offset, offset + length)` of `source`. Key
/// derivation runs to completion once started; the caller checks cancellation
/// around it.
pub(super) fn unlock(
    source: &Arc<DiskImage>,
    offset: u64,
    length: u64,
    credential: &UnlockCredential,
    cancellation: Arc<Mutex<CancellationToken>>,
) -> anyhow::Result<UnlockedBitlocker> {
    let ciphertext = Arc::new(Ciphertext {
        source: Arc::downgrade(source),
        cancellation,
    });
    let reader: Arc<dyn VolumeByteReader> = ciphertext.clone();
    let open = |key: BdeUnlockKey<'_>| {
        BdeVolume::open_with_key_io(Arc::downgrade(&reader), offset, length, key)
    };
    let mut recovered = None;
    let volume = match credential {
        UnlockCredential::Password { password } => open(BdeUnlockKey::Password(password))?,
        UnlockCredential::RecoveryPassword { recovery_password } => {
            open(BdeUnlockKey::RecoveryPassword(recovery_password))?
        }
        UnlockCredential::StartupKey { path } => {
            let key = read_startup_key(path)?;
            open(BdeUnlockKey::StartupKey(&key))?
        }
        UnlockCredential::MasterKey { key_hex, tweak_hex } => {
            let fvek = decode_key(key_hex, "FVEK")?;
            let tweak = match tweak_hex {
                Some(tweak) => decode_key(tweak, "TWEAK")?,
                None => Zeroizing::new(Vec::new()),
            };
            open(BdeUnlockKey::Keys {
                fvek: &fvek,
                tweak: &tweak,
            })?
        }
        UnlockCredential::Vmk { key_hex } => {
            let vmk = Zeroizing::new(bde_vmk::parse_vmk_hex(key_hex)?);
            if vmk.len() == 16 {
                // A 128-bit key opens the volume through the recovery password
                // its recovery datum holds.
                let password = bde_vmk::recovery_key_at(source, offset, &vmk)?.context(
                    Failure::invalid_input("the supplied key does not unlock a recovery datum"),
                )?;
                let volume = open(BdeUnlockKey::RecoveryPassword(&password))?;
                recovered = Some(password);
                volume
            } else {
                let volume = open(BdeUnlockKey::Vmk(&vmk))?;
                recovered =
                    bde_vmk::recovery_key_at(source, offset, &vmk).unwrap_or_else(|error| {
                        tracing::warn!(
                        "BitLocker unlocked; recovery-password extraction unavailable: {error:#}"
                    );
                        None
                    });
                volume
            }
        }
        UnlockCredential::ClearKey => open(BdeUnlockKey::Clear)?,
    };
    Ok(UnlockedBitlocker {
        cipher: format!("BitLocker {}", method_label(volume.encryption_method())),
        sector_size: usize::from(volume.bytes_per_sector()),
        volume,
        ciphertext,
        recovery_password: recovered,
    })
}

fn read_startup_key(path: &str) -> anyhow::Result<Zeroizing<Vec<u8>>> {
    let mut key = Zeroizing::new(Vec::new());
    File::open(path)
        .with_context(|| format!("open startup key {path}"))?
        .take(STARTUP_KEY_LIMIT + 1)
        .read_to_end(&mut key)
        .with_context(|| format!("read startup key {path}"))?;
    ensure!(
        !key.is_empty(),
        Failure::invalid_input("the startup key file is empty")
    );
    ensure!(
        key.len() as u64 <= STARTUP_KEY_LIMIT,
        Failure::invalid_input("the startup key file exceeds 64 KiB")
    );
    Ok(key)
}

fn decode_key(value: &str, name: &str) -> anyhow::Result<Zeroizing<Vec<u8>>> {
    ensure!(
        value.len() <= KEY_HEX_LIMIT,
        Failure::invalid_input(format!("{name} hex is too long"))
    );
    let compact = Zeroizing::new(value.split_whitespace().collect::<String>());
    let key = Zeroizing::new(
        hex::decode(compact.as_str())
            .map_err(|_| anyhow!(Failure::invalid_input(format!("invalid {name} hex"))))?,
    );
    ensure!(
        !key.is_empty(),
        Failure::invalid_input(format!("{name} is empty"))
    );
    Ok(key)
}

fn method_label(method: BdeEncryptionMethod) -> String {
    match method {
        BdeEncryptionMethod::None => "none".into(),
        BdeEncryptionMethod::Aes128CbcDiffuser => "AES-128-CBC + diffuser".into(),
        BdeEncryptionMethod::Aes256CbcDiffuser => "AES-256-CBC + diffuser".into(),
        BdeEncryptionMethod::Aes128Cbc => "AES-128-CBC".into(),
        BdeEncryptionMethod::Aes256Cbc => "AES-256-CBC".into(),
        BdeEncryptionMethod::Aes128Xts => "AES-128-XTS".into(),
        BdeEncryptionMethod::Aes256Xts => "AES-256-XTS".into(),
        BdeEncryptionMethod::Other(value) => format!("unknown (0x{value:04x})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_fve_fs_signature() {
        let mut sector = vec![0u8; 512];
        sector[3..11].copy_from_slice(FVE_FS_SIGNATURE);
        assert!(is_bitlocker_boot_sector_bytes(&sector));
    }

    #[test]
    fn detects_to_go_signature() {
        let mut sector = vec![0u8; 0x600];
        sector[3..11].copy_from_slice(TO_GO_SIGNATURE);
        let guid: [u8; 16] = [
            0x3b, 0xd6, 0x67, 0x49, 0x29, 0x2e, 0xd8, 0x4a, 0x83, 0x99, 0xf6, 0xa3, 0x39, 0xe3,
            0xd0, 0x01,
        ];
        sector[424..440].copy_from_slice(&guid);
        assert!(is_bitlocker_boot_sector_bytes(&sector));
    }

    #[test]
    fn ignores_plain_ntfs_boot_sector() {
        let mut sector = vec![0u8; 512];
        sector[3..11].copy_from_slice(b"NTFS    ");
        assert!(!is_bitlocker_boot_sector_bytes(&sector));
    }

    #[test]
    fn key_hex_tolerates_spaces_and_rejects_garbage() {
        assert_eq!(*decode_key("0a 0b\t0c", "FVEK").unwrap(), vec![10, 11, 12]);
        assert!(decode_key("zz", "FVEK").is_err());
        assert!(decode_key("   ", "TWEAK").is_err());
        assert!(decode_key(&"00".repeat(KEY_HEX_LIMIT), "FVEK").is_err());
    }
}
