use super::*;
use sha2::{Digest, Sha256};
use std::sync::Arc;

fn evidence() -> Vec<u8> {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor/bitlocker/tests/data/bdetogo-projection.bin.gz");
    let file = std::fs::File::open(fixture).unwrap();
    let mut data = Vec::new();
    flate2::read::GzDecoder::new(file)
        .read_to_end(&mut data)
        .unwrap();
    data
}

struct MemorySource {
    bytes: Vec<u8>,
    end: Option<u64>,
}

#[test]
fn bitlocker_recovery_password_matches_independent_python_ciphertext() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor/bitlocker/tests/data/recovery-cbc.bin");
    let bytes = std::fs::read(path).unwrap();
    let length = bytes.len() as u64;
    let source: Arc<dyn VolumeByteReader> = Arc::new(MemorySource { bytes, end: None });
    let password = "111111-111111-111111-111111-111111-111111-111111-111111";
    let mut volume = BdeVolume::open_with_key_io(
        Arc::downgrade(&source),
        0,
        length,
        BdeUnlockKey::RecoveryPassword(password),
    )
    .unwrap();
    let mut bytes = vec![0; 512];
    volume.read_at(0, &mut bytes).unwrap();
    assert_eq!(
        bytes,
        (0..512)
            .map(|index| (index as u8) ^ 0x3c)
            .collect::<Vec<_>>()
    );
    assert_eq!(volume.encryption_method(), BdeEncryptionMethod::Aes128Cbc);
}
impl VolumeByteReader for MemorySource {
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> anyhow::Result<usize> {
        let end = self.end.unwrap_or(self.bytes.len() as u64);
        let count = end.saturating_sub(offset).min(buffer.len() as u64) as usize;
        if count > 0 {
            buffer[..count].copy_from_slice(&self.bytes[offset as usize..offset as usize + count]);
        }
        Ok(count)
    }
}

#[test]
fn bitlocker_password_slice_matches_independent_pybde_oracle_and_releases_source() {
    let bytes = evidence();
    let length = bytes.len() as u64;
    let bytes = [vec![0xab; 4096], bytes, vec![0xcd; 4096]].concat();
    let source: Arc<dyn VolumeByteReader> = Arc::new(MemorySource { bytes, end: None });
    let mut volume = BdeVolume::open_with_key_io(
        Arc::downgrade(&source),
        4096,
        length,
        BdeUnlockKey::Password("bde-TEST"),
    )
    .unwrap();
    assert_eq!(
        (volume.size(), volume.encryption_method()),
        (length, BdeEncryptionMethod::Aes128CbcDiffuser)
    );
    for (offset, size, expected) in [
        (
            0,
            512,
            "139b857c537e341ceb98bcfde2d31825efcf4b0c0281dd66672e954b34ed28f3",
        ),
        (
            2048,
            512,
            "bf762af77278b30a1b8bbdfb6cbf2e3565cdf6fcb09d48e683c464af2c3abd71",
        ),
        (
            35840,
            512,
            "48ddda42757a815d391ff1d3f01a37b085fd05ec57a9d903946412551bfe5a7b",
        ),
        (
            0x8000,
            4096,
            "1d138f11707f0a3b4832c5173167c756e1bac9796518b64800d5dc374986fe4d",
        ),
    ] {
        let mut buffer = vec![0; size];
        assert_eq!(volume.read_at(offset, &mut buffer).unwrap(), size);
        assert_eq!(hex::encode(Sha256::digest(&buffer)), expected);
    }
    let mut whole = [0; 1024];
    volume.read_at(0, &mut whole).unwrap();
    let mut unaligned = [0; 37];
    volume.read_at(499, &mut unaligned).unwrap();
    assert_eq!(unaligned.as_slice(), &whole[499..536]);
    assert_eq!(volume.read_at(u64::MAX, &mut whole).unwrap(), 0);
    drop(source);
    assert!(volume.read_at(34603008, &mut whole).is_err());
    assert!(format!("{:#}", volume.read_at(0, &mut whole).unwrap_err())
        .contains("source image is closed"));
}

#[test]
fn bitlocker_bad_credentials_and_truncated_ciphertext_fail() {
    let bytes = evidence();
    let length = bytes.len() as u64;
    // Metadata precedes the relocated boot data; unlock can succeed but a sector
    // read must fail when the underlying encrypted evidence is truncated.
    let source: Arc<dyn VolumeByteReader> = Arc::new(MemorySource {
        bytes,
        end: Some(34670592 + 100),
    });
    let wrong = BdeVolume::open_with_key_io(
        Arc::downgrade(&source),
        0,
        length,
        BdeUnlockKey::Password("wrong-test-password"),
    )
    .unwrap_err();
    let message = format!("{wrong:#}");
    assert!(message.contains("authentication failed"));
    assert!(!message.contains("wrong-test-password"));
    assert!(BdeVolume::open_with_key_io(
        Arc::downgrade(&source),
        0,
        length,
        BdeUnlockKey::RecoveryPassword("invalid")
    )
    .is_err());
    let mut volume = BdeVolume::open_with_key_io(
        Arc::downgrade(&source),
        0,
        length,
        BdeUnlockKey::Password("bde-TEST"),
    )
    .unwrap();
    assert!(volume.read_at(0, &mut [0; 512]).is_err());
}

#[test]
fn bitlocker_invalid_sector_size_and_cipher_are_explicit_errors() {
    let mut bytes = evidence();
    let length = bytes.len() as u64;
    bytes[11..13].copy_from_slice(&768u16.to_le_bytes());
    let source: Arc<dyn VolumeByteReader> = Arc::new(MemorySource {
        bytes: bytes.clone(),
        end: None,
    });
    let error = BdeVolume::open_with_key_io(
        Arc::downgrade(&source),
        0,
        length,
        BdeUnlockKey::Password("bde-TEST"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("unsupported bytes per sector"));
    bytes[11..13].copy_from_slice(&512u16.to_le_bytes());
    bytes[34603008 + 100..34603008 + 102].copy_from_slice(&0x8006u16.to_le_bytes());
    let source: Arc<dyn VolumeByteReader> = Arc::new(MemorySource { bytes, end: None });
    let error = BdeVolume::open_with_key_io(
        Arc::downgrade(&source),
        0,
        length,
        BdeUnlockKey::Password("bde-TEST"),
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("0x8006"));
}

#[test]
fn bitlocker_diskimage_unlock_observes_cancellation_and_plaintext() {
    use crate::{
        evidence::crypto::bitlocker::{self, BitLockerLocation, UnlockObserver},
        evidence::image::DiskImage,
    };
    use std::sync::{atomic::AtomicBool, Mutex};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bde.bin");
    std::fs::write(&path, evidence()).unwrap();
    let image = Arc::new(DiskImage::open(path).unwrap());
    let location = BitLockerLocation {
        partition_id: None,
        offset: 0,
        length: image.len(),
    };
    assert!(!bitlocker::locate_bitlocker_volumes(&image)
        .unwrap()
        .is_empty());
    let cancelled = AtomicBool::new(true);
    assert!(bitlocker::unlock_location(
        &image,
        location,
        BdeUnlockKey::Password("bde-TEST"),
        &UnlockObserver {
            progress: &|_, _| panic!("cancelled unlock emitted progress"),
            cancel: &cancelled
        }
    )
    .is_err());
    let progress = Mutex::new(Vec::new());
    let observer = |done, total| progress.lock().unwrap().push((done, total));
    let cancelled = AtomicBool::new(false);
    let volume = bitlocker::unlock_location(
        &image,
        location,
        BdeUnlockKey::Password("bde-TEST"),
        &UnlockObserver {
            progress: &observer,
            cancel: &cancelled,
        },
    )
    .unwrap();
    let bytes = bitlocker::read_plaintext_exact(&volume, 0, 512).unwrap();
    assert_eq!(&bytes[3..11], b"mkdosfs\0");
    assert_eq!(
        *progress.lock().unwrap(),
        vec![(0, image.len()), (image.len(), image.len())]
    );
}
