//! BitLocker volumes read directly from decoded evidence through a pure Rust reader.
use crate::evidence::crypto::bde_image::{
    BdeEncryptionMethod, BdeUnlockKey, BdeVolume, VolumeByteReader,
};
use crate::evidence::image::DiskImage;
use anyhow::{anyhow, ensure, Context};
use serde::Serialize;
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

const FVE_FS_SIGNATURE: &[u8; 8] = b"-FVE-FS-";
const TO_GO_SIGNATURE: &[u8; 8] = b"MSWIN4.1";

/// Unlock lifecycle observer. Progress emits zero before derivation and the
/// volume length on successful completion; cancellation is checked at both ends.
pub struct UnlockObserver<'a> {
    pub progress: &'a dyn Fn(u64, u64),
    pub cancel: &'a AtomicBool,
}
impl UnlockObserver<'_> {
    pub fn noop() -> UnlockObserver<'static> {
        UnlockObserver {
            progress: &NOOP_PROGRESS,
            cancel: &NEVER_CANCEL,
        }
    }
}
fn noop_progress(_done: u64, _total: u64) {}
const NOOP_PROGRESS: fn(u64, u64) = noop_progress;
static NEVER_CANCEL: AtomicBool = AtomicBool::new(false);

/// Payload of the existing `bitlocker-unlock-progress` event. Bytes represent
/// the volume unlocked, not copied; no encrypted-volume temporary file is made.
#[derive(Debug, Clone, Serialize)]
pub struct BitlockerUnlockProgress {
    pub task_id: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

/// A BitLocker volume located inside a `DiskImage`. Offset/length are absolute
/// byte coordinates against the decoded image (i.e. raw partition slice).
#[derive(Debug, Clone, Copy)]
pub struct BitLockerLocation {
    pub partition_id: Option<u8>,
    pub offset: u64,
    pub length: u64,
}

/// Returns every BitLocker volume signature found in the image: each known
/// partition slice is probed, and the first sector of the image itself is
/// inspected for the no-partition-table case (e.g. BitLocker To Go USB).
pub fn locate_bitlocker_volumes(image: &DiskImage) -> anyhow::Result<Vec<BitLockerLocation>> {
    let mut hits = Vec::new();

    let head = image.read_range(0, 4096)?;
    if is_bitlocker_boot_sector_bytes(&head) {
        hits.push(BitLockerLocation {
            partition_id: None,
            offset: 0,
            length: image.len(),
        });
    }

    for partition in &image.partitions {
        let offset = partition.start_lba * image.sector_size as u64;
        let length = partition.length_lba * image.sector_size as u64;
        if length == 0 {
            continue;
        }
        let probe = image.read_range(offset, 4096)?;
        if is_bitlocker_boot_sector_bytes(&probe) {
            hits.push(BitLockerLocation {
                partition_id: Some(partition.id),
                offset,
                length,
            });
        }
    }

    Ok(hits)
}

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

/// Unlock the encrypted partition in place, without temporary extraction.
pub fn unlock_location(
    image: &Arc<DiskImage>,
    location: BitLockerLocation,
    key: BdeUnlockKey<'_>,
    observer: &UnlockObserver<'_>,
) -> anyhow::Result<UnlockedVolume> {
    ensure!(
        !observer.cancel.load(Ordering::Relaxed),
        "BitLocker 解锁已取消"
    );
    ensure!(
        location
            .offset
            .checked_add(location.length)
            .is_some_and(|end| end <= image.len()),
        "BitLocker partition lies outside the source image"
    );
    (observer.progress)(0, location.length);
    let image_dyn: Arc<dyn VolumeByteReader> = image.clone();
    let reader: Weak<dyn VolumeByteReader> = Arc::downgrade(&image_dyn);
    let volume = BdeVolume::open_with_key_io(reader, location.offset, location.length, key)
        .with_context(|| {
            format!(
                "failed to unlock BitLocker volume at offset {}",
                location.offset
            )
        })?;
    ensure!(
        !observer.cancel.load(Ordering::Relaxed),
        "BitLocker 解锁已取消"
    );
    (observer.progress)(location.length, location.length);
    Ok(UnlockedVolume::new(volume))
}

/// Convenience: locate the first BitLocker volume in the image and unlock it
/// with a recovery password.
pub fn unlock_first_with_recovery_password(
    image: &Arc<DiskImage>,
    recovery_password: &str,
) -> anyhow::Result<UnlockedVolume> {
    let location = locate_bitlocker_volumes(image)?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no BitLocker volume found in image"))?;
    unlock_location(
        image,
        location,
        BdeUnlockKey::RecoveryPassword(recovery_password),
        &UnlockObserver::noop(),
    )
}

/// Shared plaintext volume. The source is borrowed weakly, so closing a case
/// releases the underlying evidence without a reference cycle or temp dump.
#[derive(Debug)]
pub struct UnlockedVolume {
    inner: Mutex<BdeVolume>,
    size: u64,
    encryption_method: Option<BdeEncryptionMethod>,
}

impl UnlockedVolume {
    fn new(volume: BdeVolume) -> Self {
        let size = volume.size();
        let encryption_method = Some(volume.encryption_method());
        Self {
            inner: Mutex::new(volume),
            size,
            encryption_method,
        }
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn read_at(&self, offset: u64, buf: &mut [u8]) -> anyhow::Result<usize> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| anyhow!("BDE volume mutex poisoned"))?;
        guard.read_at(offset, buf)
    }

    pub fn encryption_method(&self) -> Option<BdeEncryptionMethod> {
        self.encryption_method
    }
}

/// Reads `len` bytes of plaintext starting at `offset`, looping over
/// `read_at` until either the buffer fills or EOF is reached.
pub fn read_plaintext_exact(
    volume: &UnlockedVolume,
    offset: u64,
    len: usize,
) -> anyhow::Result<Vec<u8>> {
    let mut out = vec![0u8; len];
    let mut filled = 0;
    while filled < len {
        let read = volume.read_at(offset + filled as u64, &mut out[filled..])?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    out.truncate(filled);
    Ok(out)
}

// Provided for completeness so callers can pull plaintext via std::io::Read.
pub struct UnlockedReader<'a> {
    volume: &'a UnlockedVolume,
    cursor: u64,
}

impl<'a> UnlockedReader<'a> {
    pub fn new(volume: &'a UnlockedVolume) -> Self {
        Self { volume, cursor: 0 }
    }
}

impl Read for UnlockedReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.volume.read_at(self.cursor, buf) {
            Ok(read) => {
                self.cursor += read as u64;
                Ok(read)
            }
            Err(err) => Err(std::io::Error::other(err.to_string())),
        }
    }
}

impl Seek for UnlockedReader<'_> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let target: i128 = match pos {
            SeekFrom::Start(value) => value as i128,
            SeekFrom::Current(delta) => self.cursor as i128 + delta as i128,
            SeekFrom::End(delta) => self.volume.size() as i128 + delta as i128,
        };
        if target < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek before start of unlocked volume",
            ));
        }
        self.cursor = target as u64;
        Ok(self.cursor)
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

    /// Exercises the in-place (no temp-dump) unlock and asserts that no `.raw`
    /// dump is created under `%TEMP%\lovelycarve-bde` — the regression guard
    /// for the C:-drive temp-image fix.
    #[test]
    fn unlocks_in_place_without_temp_dump_when_env_path_is_set() {
        let Some(path) = std::env::var_os("LOVELYCARVE_BDE_TEST") else {
            return;
        };
        let recovery = std::env::var("LOVELYCARVE_BDE_RECOVERY")
            .expect("LOVELYCARVE_BDE_RECOVERY must be set alongside LOVELYCARVE_BDE_TEST");

        let image = Arc::new(DiskImage::open(path).expect("BDE-bearing image should open"));
        let location = locate_bitlocker_volumes(&image)
            .expect("scan should succeed")
            .into_iter()
            .next()
            .expect("expected a BitLocker volume");

        let cache_dir = std::env::temp_dir().join("lovelycarve-bde");
        let raw_before = count_raw_dumps(&cache_dir);

        let unlocked = unlock_location(
            &image,
            location,
            BdeUnlockKey::RecoveryPassword(&recovery),
            &UnlockObserver::noop(),
        )
        .expect("in-place BitLocker unlock should succeed");

        let head = read_plaintext_exact(&unlocked, 0, 4096).expect("plaintext read");
        let oem = &head[3..11];
        eprintln!(
            "in-place plaintext OEM bytes: {:?}",
            String::from_utf8_lossy(oem)
        );
        assert!(
            oem == b"NTFS    " || oem == b"MSDOS5.0" || oem == b"mkfs.fat",
            "unlocked head OEM bytes do not look like a recognised filesystem"
        );

        let raw_after = count_raw_dumps(&cache_dir);
        assert_eq!(
            raw_before,
            raw_after,
            "in-place unlock must not create a .raw dump under {}",
            cache_dir.display()
        );
    }

    /// Counts `*.raw` files in the BitLocker dump cache dir (0 if absent).
    fn count_raw_dumps(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|e| e.path().extension().is_some_and(|ext| ext == "raw"))
                    .count()
            })
            .unwrap_or(0)
    }
}
