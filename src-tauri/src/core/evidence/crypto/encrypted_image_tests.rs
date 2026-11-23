use std::io::Read;
use std::{path::PathBuf, sync::Arc};

use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::core::common::error::{code_of, ErrorCode};
use crate::core::evidence::crypto::encrypted_image::{
    unlock, EncryptionScheme, UnlockCredential, UnlockOptions,
};
use crate::core::evidence::image::{DiskImage, ImageFormat, PartitionKind, UnlockedPartition};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/encrypted")
        .join(name)
}

fn bitlocker_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("vendor/bitlocker/tests/data")
        .join(name)
}

fn password(value: &str) -> UnlockCredential {
    UnlockCredential::Password {
        password: value.into(),
    }
}

fn master_key(key_hex: String) -> UnlockCredential {
    UnlockCredential::MasterKey {
        key_hex,
        tweak_hex: None,
    }
}

fn options(scheme: EncryptionScheme) -> UnlockOptions {
    UnlockOptions {
        target: Default::default(),
        scheme,
        credential: password("lovelycarve-test"),
        logical_volume_index: None,
        encrypted_root_plist_path: None,
        partition_id: None,
        offset: None,
        length: None,
        hidden: false,
        pim: if scheme == EncryptionScheme::Veracrypt {
            1
        } else {
            0
        },
    }
}

fn error_code(result: anyhow::Result<impl Sized>) -> Option<ErrorCode> {
    code_of(&result.err().expect("the unlock should fail"))
}

#[test]
fn luks_master_keys_unlock_both_versions_without_password() {
    let expected = std::fs::read(fixture("plain.img")).unwrap();
    let key = hex::encode((0..64u8).collect::<Vec<_>>());
    for name in ["luks1.img", "luks2.img"] {
        let source = Arc::new(DiskImage::open(&fixture(name)).unwrap());
        let mut credentials = options(EncryptionScheme::Luks);
        credentials.credential = master_key(key.clone());
        let (image, _) = unlock(&source, credentials, CancellationToken::new()).unwrap();
        assert_eq!(image.read_range(0, expected.len()).unwrap(), expected);
        for invalid in ["", "0", "invalid", &"ab".repeat(257), &"00".repeat(64)] {
            let mut credentials = options(EncryptionScheme::Luks);
            credentials.credential = master_key(invalid.into());
            assert!(unlock(&source, credentials, CancellationToken::new()).is_err());
        }
    }
}

#[test]
fn credentials_and_options_of_another_scheme_fail_before_reading() {
    let source = Arc::new(DiskImage::open(&fixture("luks1.img")).unwrap());
    let mismatched = [
        (EncryptionScheme::Luks, UnlockCredential::ClearKey),
        (
            EncryptionScheme::Luks,
            UnlockCredential::MasterKey {
                key_hex: "00".into(),
                tweak_hex: Some("11".into()),
            },
        ),
        (EncryptionScheme::Veracrypt, master_key("00".into())),
        (
            EncryptionScheme::Filevault,
            UnlockCredential::Vmk {
                key_hex: "00".into(),
            },
        ),
        (EncryptionScheme::Luks, password("")),
    ];
    for (scheme, credential) in mismatched {
        let mut credentials = options(scheme);
        credentials.credential = credential;
        assert_eq!(
            error_code(unlock(&source, credentials, CancellationToken::new())),
            Some(ErrorCode::InvalidInput)
        );
    }
    let mut pim = options(EncryptionScheme::Bitlocker);
    pim.pim = 1;
    assert_eq!(
        error_code(unlock(&source, pim, CancellationToken::new())),
        Some(ErrorCode::InvalidInput)
    );
}

#[test]
fn independent_luks_compatibility_corpus_is_accepted_by_application_validation() {
    use base64::Engine;
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../vendor/luks/tests/data/compatibility.json"
    ))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let expected: Vec<u8> = (0..1536).map(|index| (index * 17 + 7) as u8).collect();
    for (index, case) in cases["containers"].as_array().unwrap().iter().enumerate() {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(case["image"].as_str().unwrap())
            .unwrap();
        let path = directory.path().join(format!("luks-{index}.img"));
        std::fs::write(&path, &bytes).unwrap();
        let source = Arc::new(DiskImage::open(&path).unwrap());
        let mut credentials = options(EncryptionScheme::Luks);
        credentials.credential = password("LovelyCarve-密码");
        let (image, _) = unlock(&source, credentials, CancellationToken::new())
            .unwrap_or_else(|error| panic!("{} / {}: {error:#}", case["cipher"], case["mode"]));
        assert_eq!(image.read_range(507, 1029).unwrap(), expected[507..1536]);
    }
}

#[test]
fn independent_encrypted_fixtures_match_plaintext_and_detect_fat() {
    let expected = std::fs::read(fixture("plain.img")).unwrap();
    for (name, scheme, hidden) in [
        ("luks1.img", EncryptionScheme::Luks, false),
        ("luks2.img", EncryptionScheme::Luks, false),
        ("veracrypt.hc", EncryptionScheme::Veracrypt, false),
        ("hidden.hc", EncryptionScheme::Veracrypt, true),
    ] {
        let source = Arc::new(DiskImage::open(&fixture(name)).unwrap());
        let mut options = options(scheme);
        options.hidden = hidden;
        let (image, result) = unlock(&source, options, CancellationToken::new()).unwrap();
        assert_eq!(
            image.read_range(0, expected.len()).unwrap(),
            expected,
            "{name}"
        );
        assert_eq!(image.read_range(507, 1031).unwrap(), expected[507..1538]);
        assert_eq!(result.image.source_format, ImageFormat::Decrypted);
        assert_eq!(image.partitions.len(), 1);
        assert_eq!(result.source_length, source.len());
        assert!(image.read_range(image.len(), 1).unwrap().is_empty());
        drop(source);
        assert!(
            image.read_range(0, 512).is_err(),
            "closed source must not yield fabricated plaintext"
        );
    }
}

#[test]
fn veracrypt_regions_must_be_whole_sectors() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ragged.hc");
    let mut bytes = std::fs::read(fixture("veracrypt.hc")).unwrap();
    let container = bytes.len() as u64;
    bytes.extend_from_slice(&[0; 100]);
    std::fs::write(&path, &bytes).unwrap();
    let source = Arc::new(DiskImage::open(&path).unwrap());
    assert_eq!(source.sector_size, 512);
    // The whole file is no VeraCrypt volume: 100 bytes past the last sector.
    let error = unlock(
        &source,
        options(EncryptionScheme::Veracrypt),
        CancellationToken::new(),
    )
    .err()
    .unwrap();
    assert_eq!(code_of(&error), Some(ErrorCode::InvalidInput));
    assert!(
        format!("{error:#}").contains("512-byte sectors"),
        "{error:#}"
    );
    // The container's own whole-sector span opens.
    let mut bounded = options(EncryptionScheme::Veracrypt);
    bounded.offset = Some(0);
    bounded.length = Some(container);
    let (image, _) = unlock(&source, bounded, CancellationToken::new()).unwrap();
    assert_eq!(
        image.read_range(0, 512).unwrap(),
        std::fs::read(fixture("plain.img")).unwrap()[..512]
    );
}

#[test]
fn encrypted_region_is_bounded_and_can_start_inside_an_image() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("offset.img");
    let encrypted = std::fs::read(fixture("luks1.img")).unwrap();
    let mut bytes = vec![0; 4096];
    bytes.extend_from_slice(&encrypted);
    bytes.extend_from_slice(&[0x55; 512]);
    std::fs::write(&path, bytes).unwrap();
    let source = Arc::new(DiskImage::open(&path).unwrap());
    let mut bounded = options(EncryptionScheme::Luks);
    bounded.offset = Some(4096);
    bounded.length = Some(encrypted.len() as u64);
    let (image, _) = unlock(&source, bounded, CancellationToken::new()).unwrap();
    assert_eq!(
        image.read_range(0, image.len() as usize).unwrap(),
        std::fs::read(fixture("plain.img")).unwrap()
    );
    let mut outside = options(EncryptionScheme::Luks);
    outside.offset = Some(u64::MAX);
    assert!(unlock(&source, outside, CancellationToken::new()).is_err());
}

#[test]
fn wrong_password_cancellation_and_truncation_fail_closed() {
    let source = Arc::new(DiskImage::open(&fixture("luks1.img")).unwrap());
    let mut wrong = options(EncryptionScheme::Luks);
    wrong.credential = password("wrong");
    assert!(unlock(&source, wrong, CancellationToken::new()).is_err());
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(unlock(&source, options(EncryptionScheme::Luks), cancelled).is_err());
    let mut truncated = options(EncryptionScheme::Luks);
    truncated.length = Some(source.len() - 1);
    assert!(unlock(&source, truncated, CancellationToken::new()).is_err());
    let mut conflict = options(EncryptionScheme::Luks);
    conflict.partition_id = Some(0);
    conflict.offset = Some(0);
    assert!(unlock(&source, conflict, CancellationToken::new()).is_err());
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The minimized dfVFS `bdetogo.raw` projection: a BitLocker To Go volume
/// whose published password is `bde-TEST`.
fn bdetogo() -> (tempfile::TempDir, Arc<DiskImage>) {
    let mut bytes = Vec::new();
    flate2::read::GzDecoder::new(
        std::fs::File::open(bitlocker_fixture("bdetogo-projection.bin.gz")).unwrap(),
    )
    .read_to_end(&mut bytes)
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bdetogo.raw");
    std::fs::write(&path, bytes).unwrap();
    let image = Arc::new(DiskImage::open(&path).unwrap());
    (directory, image)
}

#[test]
fn bitlocker_unlocks_through_the_shared_path_and_attaches_in_place() {
    use crate::core::filesystem::evidence_session::{EvidenceSession, ImageExportMode};

    let (_directory, disk) = bdetogo();
    assert_eq!(disk.partitions[0].kind, PartitionKind::Bitlocker);
    let partition_id = disk.partitions[0].id;
    let mut credentials = options(EncryptionScheme::Bitlocker);
    credentials.partition_id = Some(partition_id);
    credentials.credential = password("bde-TEST");
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert_eq!(
        error_code(unlock(&disk, credentials, cancelled)),
        Some(ErrorCode::Cancelled)
    );
    let mut wrong = options(EncryptionScheme::Bitlocker);
    wrong.credential = password("wrong-test-password");
    assert!(unlock(&disk, wrong, CancellationToken::new()).is_err());

    let mut credentials = options(EncryptionScheme::Bitlocker);
    credentials.partition_id = Some(partition_id);
    credentials.credential = password("bde-TEST");
    let (plain, result) = unlock(&disk, credentials, CancellationToken::new()).unwrap();
    assert_eq!(
        (
            result.scheme,
            result.cipher.as_str(),
            result.encryption_sector_size,
            result.source_length,
            result.recovery_password.as_deref(),
        ),
        (
            EncryptionScheme::Bitlocker,
            "BitLocker AES-128-CBC + diffuser",
            512,
            disk.len(),
            None,
        )
    );
    // One volume filling the plaintext, typed by its own boot sector.
    assert_eq!(plain.partitions.len(), 1);
    assert_eq!(
        (
            plain.partitions[0].start_lba,
            plain.partitions[0].length_lba
        ),
        (0, plain.len() / 512)
    );
    let kind = plain.partitions[0].kind;
    assert!(
        matches!(
            kind,
            PartitionKind::Fat12 | PartitionKind::Fat16 | PartitionKind::Fat32
        ),
        "{kind:?}"
    );
    // Logical sectors the independent pybde oracle published.
    assert_eq!(
        sha256_hex(&plain.read_range(0, 512).unwrap()),
        "139b857c537e341ceb98bcfde2d31825efcf4b0c0281dd66672e954b34ed28f3"
    );
    assert_eq!(
        sha256_hex(&plain.read_range(0x8000, 4096).unwrap()),
        "1d138f11707f0a3b4832c5173167c756e1bac9796518b64800d5dc374986fe4d"
    );

    let source = Arc::new(EvidenceSession::from(Arc::clone(&disk)));
    let raw = source.read_range(0, 512).unwrap();
    source
        .attach_decrypted_partition(partition_id, plain, result.scheme, result.read_cancellation)
        .unwrap();
    let info = source.info();
    assert_eq!(
        info.unlocked_partitions,
        vec![UnlockedPartition {
            id: partition_id,
            scheme: EncryptionScheme::Bitlocker,
        }]
    );
    assert_eq!(info.partitions[0].kind, kind);
    assert_eq!(
        source.read_range(0, 512).unwrap(),
        raw,
        "source bytes stay ciphertext"
    );
    let export = source
        .raw_export_reader(ImageExportMode::UnlockedPartitions)
        .unwrap();
    let mut sector = vec![0; 512];
    export.read_into(0, &mut sector).unwrap();
    assert_eq!(
        sha256_hex(&sector),
        "139b857c537e341ceb98bcfde2d31825efcf4b0c0281dd66672e954b34ed28f3"
    );
    source.revoke_decrypted_partitions();
    assert!(source.info().unlocked_partitions.is_empty());
}

/// An independent BitLocker cipher projection (`parity-*.bin`) laid out the
/// way the vendored reader's tests expand it.
fn bitlocker_parity(directory: &tempfile::TempDir, method: u16, sector: usize) -> Arc<DiskImage> {
    let projection = std::fs::read(bitlocker_fixture(&format!(
        "parity-{method:04x}-{sector}.bin"
    )))
    .unwrap();
    let block = u32::from_le_bytes(projection[4..8].try_into().unwrap()) as usize;
    let mut image = vec![0; 0x80000];
    image[..512].copy_from_slice(&projection[8..520]);
    for offset in [0x20000, 0x30000, 0x40000] {
        image[offset..offset + block].copy_from_slice(&projection[520..520 + block]);
    }
    let sectors = &projection[520 + block..];
    image[0x10000..0x10000 + sector].copy_from_slice(&sectors[..sector]);
    image[0x12000..0x12000 + sector].copy_from_slice(&sectors[sector..2 * sector]);
    let path = directory
        .path()
        .join(format!("parity-{method:04x}-{sector}.raw"));
    std::fs::write(&path, image).unwrap();
    Arc::new(DiskImage::open(&path).unwrap())
}

#[test]
fn every_bitlocker_credential_opens_its_independent_projection() {
    let directory = tempfile::tempdir().unwrap();
    let startup_key = bitlocker_fixture("parity-startup.bek");
    for (method, sector, credential) in [
        (0x8001, 512, UnlockCredential::ClearKey),
        (
            0x8005,
            4096,
            UnlockCredential::StartupKey {
                path: startup_key.to_string_lossy().into_owned(),
            },
        ),
        (
            0x8004,
            512,
            UnlockCredential::Vmk {
                key_hex: "46".repeat(32),
            },
        ),
        (
            0x8001,
            4096,
            UnlockCredential::MasterKey {
                key_hex: hex::encode((1..=32u8).collect::<Vec<_>>()),
                tweak_hex: Some(hex::encode((65..=96u8).collect::<Vec<_>>())),
            },
        ),
    ] {
        let label = credential.label();
        let source = bitlocker_parity(&directory, method, sector);
        let mut credentials = options(EncryptionScheme::Bitlocker);
        credentials.credential = credential;
        let (plain, result) = unlock(&source, credentials, CancellationToken::new())
            .unwrap_or_else(|error| panic!("{label}: {error:#}"));
        let mut expected: Vec<u8> = (0..sector).map(|index| (index * 7 + 19) as u8).collect();
        expected[3..11].copy_from_slice(b"NTFS    ");
        assert_eq!(plain.read_range(0, sector).unwrap(), expected, "{label}");
        assert_eq!(
            plain.read_range(0x12000, sector).unwrap(),
            expected,
            "{label}"
        );
        // Format-defined regions read as zeroes rather than ciphertext.
        assert_eq!(plain.read_range(0x10000, sector).unwrap(), vec![0; sector]);
        assert_eq!(
            (
                result.encryption_sector_size,
                result.recovery_password,
                plain.partitions.len()
            ),
            (sector, None, 1),
            "{label}"
        );
    }
}
#[test]
fn corrupt_and_unbounded_luks2_headers_are_rejected_before_kdf() {
    for oversized in [false, true] {
        let mut bytes = std::fs::read(fixture("luks2.img")).unwrap();
        if oversized {
            bytes[8..16].copy_from_slice(&u64::MAX.to_be_bytes());
        } else {
            bytes[4200] ^= 1;
        }
        let mut reader = std::io::Cursor::new(&bytes);
        assert!(crate::core::evidence::crypto::luks_metadata::validate(
            &mut reader,
            bytes.len() as u64
        )
        .is_err());
    }
}

#[test]
fn checksum_valid_luks2_headers_with_invalid_or_overflowing_iv_are_rejected() {
    use sha2::{Digest, Sha256};
    for tweak in ["not-a-number", "18446744073709551615"] {
        let mut bytes = std::fs::read(fixture("luks2.img")).unwrap();
        let end = bytes[4096..16384]
            .iter()
            .position(|byte| *byte == 0)
            .unwrap()
            + 4096;
        let mut metadata: serde_json::Value = serde_json::from_slice(&bytes[4096..end]).unwrap();
        metadata["segments"]["0"]["iv_tweak"] = tweak.into();
        let encoded = serde_json::to_vec(&metadata).unwrap();
        bytes[4096..16384].fill(0);
        bytes[4096..4096 + encoded.len()].copy_from_slice(&encoded);
        bytes[448..512].fill(0);
        let checksum = Sha256::digest(&bytes[..16384]);
        bytes[448..480].copy_from_slice(&checksum);
        let error = crate::core::evidence::crypto::luks_metadata::validate(
            &mut std::io::Cursor::new(&bytes),
            bytes.len() as u64,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("numeric metadata")
                || error.to_string().contains("IV sector range"),
            "{error}"
        );
    }
}

#[test]
fn attached_decryption_preserves_source_bytes_ids_and_snapshot_identity() {
    use crate::core::catalog::snapshot::volume_snapshot;
    use crate::core::filesystem::evidence_session::EvidenceSession;
    use crate::core::filesystem::tree::fs_tree;

    for name in ["luks1.img", "luks2.img", "veracrypt.hc"] {
        let scheme = if name.ends_with(".hc") {
            EncryptionScheme::Veracrypt
        } else {
            EncryptionScheme::Luks
        };
        let mut disk = DiskImage::open(&fixture(name)).unwrap();
        // Retain a nonzero source id so accidental part:0 remapping is detectable.
        if let Some(partition) = disk.partitions.first_mut() {
            partition.id = 2;
        }
        let disk = Arc::new(disk);
        let raw = disk.read_range(0, 4096).unwrap();
        let source = Arc::new(EvidenceSession::from(Arc::clone(&disk)));
        let token = CancellationToken::new();
        let (plain, result) = unlock(&disk, options(scheme), token.clone()).unwrap();
        let plain_size = plain.len();
        source
            .attach_decrypted_partition(2, plain, scheme, result.read_cancellation)
            .unwrap();
        token.cancel(); // The completed view outlives its initiating task.
        assert_eq!(source.read_range(0, 4096).unwrap(), raw);
        assert_eq!(source.info().id, disk.id);
        assert_eq!(
            source.info().unlocked_partitions,
            vec![UnlockedPartition { id: 2, scheme }]
        );
        let children = fs_tree::children_for_image(&source, "part:2").unwrap();
        let hello = children
            .iter()
            .find(|node| node.name == "HELLO.TXT")
            .unwrap();
        assert!(hello.id.starts_with("fat:part:2:"));
        let bytes = fs_tree::read_file(&source, &hello.id).unwrap();
        assert!(!bytes.is_empty());
        assert_eq!(
            fs_tree::read_file_range(&source, &hello.id, 1, 5).unwrap(),
            bytes[1..6]
        );
        let boot = fs_tree::read_file_range(&source, "sysfile:part:2:0x0:0x200", 0, 512).unwrap();
        assert_eq!(boot, std::fs::read(fixture("plain.img")).unwrap()[..512]);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("unlocked.dirz");
        let built = volume_snapshot::build_volume_snapshot(
            &source,
            "part:2",
            &path,
            100,
            "test".into(),
            &CancellationToken::new(),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(built.header.source_volume_size, plain_size);
        let snapshot = volume_snapshot::load_snapshot(&path).unwrap();
        assert_eq!(snapshot.header.image_id, disk.id);
        assert!(snapshot
            .entries
            .iter()
            .any(|entry| entry.node.id == hello.id));
        source.revoke_decrypted_partitions();
        assert!(fs_tree::read_file(&source, &hello.id).is_err());
        assert_eq!(source.read_range(0, 4096).unwrap(), raw);
    }
}

#[test]
fn filevault_real_oracle_integrates_inventory_hfs_ranges_and_source_identity() {
    use crate::core::evidence::crypto::filevault::{inspect, InspectOptions};
    use crate::core::evidence::partitions::PartitionKind;
    use crate::core::filesystem::{evidence_session::EvidenceSession, tree::fs_tree};
    let Ok(path) = std::env::var("FVDE_ORACLE_IMAGE") else {
        return;
    };
    let mut disk = DiskImage::open(&path).unwrap();
    assert_eq!(disk.partitions[0].kind, PartitionKind::Filevault);
    disk.partitions[0].id = 2;
    let disk = Arc::new(disk);
    let info = inspect(
        &disk,
        InspectOptions {
            partition_id: Some(2),
            offset: None,
            length: None,
            encrypted_root_plist_path: None,
        },
        CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(info.volumes.len(), 1);
    assert_eq!(info.volumes[0].transaction, "7");
    assert_eq!(info.volumes[0].pbkdf2_iterations, 90506);
    let mut credentials = options(EncryptionScheme::Filevault);
    credentials.credential = password("fvde-TEST");
    credentials.logical_volume_index = Some(0);
    credentials.partition_id = Some(2);
    let (plain, result) = unlock(&disk, credentials, CancellationToken::new()).unwrap();
    assert_eq!(&plain.read_range(1024, 4).unwrap(), b"H+\0\x04");
    let source = Arc::new(EvidenceSession::from(Arc::clone(&disk)));
    source
        .attach_decrypted_partition(2, plain, result.scheme, result.read_cancellation)
        .unwrap();
    assert_eq!(source.info().id, disk.id);
    assert_eq!(
        source.info().unlocked_partitions,
        vec![UnlockedPartition {
            id: 2,
            scheme: EncryptionScheme::Filevault,
        }]
    );
    assert_eq!(
        source.info().partitions[0].length_lba,
        disk.partitions[0].length_lba
    );
    let children = fs_tree::children_for_image(&source, "part:2").unwrap();
    assert!(!children.is_empty());
    for node in children.iter().filter(|node| {
        node.kind == fs_tree::NodeKind::File && node.size > 0 && node.size < 1024 * 1024
    }) {
        let full = fs_tree::read_file(&source, &node.id).unwrap();
        assert_eq!(
            fs_tree::read_file_range(&source, &node.id, 1, 17).unwrap(),
            full[1..full.len().min(18)]
        );
        assert!(fs_tree::file_stat(&source, &node.id)
            .unwrap()
            .data_runs
            .is_empty());
    }
    source.revoke_decrypted_partitions();
    assert!(source.info().unlocked_partitions.is_empty());
}
