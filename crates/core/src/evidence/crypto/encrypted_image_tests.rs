use std::{path::PathBuf, sync::Arc};

use tokio_util::sync::CancellationToken;

use crate::evidence::crypto::encrypted_image::{unlock, EncryptionScheme, UnlockOptions};
use crate::evidence::image::{DiskImage, ImageFormat};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/encrypted")
        .join(name)
}

#[test]
fn luks_master_keys_unlock_both_versions_without_password_and_reject_conflicting_credentials() {
    let expected = std::fs::read(fixture("plain.img")).unwrap();
    let key = hex::encode((0..64u8).collect::<Vec<_>>());
    for name in ["luks1.img", "luks2.img"] {
        let source = Arc::new(DiskImage::open(&fixture(name)).unwrap());
        let mut credentials = options(EncryptionScheme::Luks);
        credentials.password.clear();
        credentials.master_key_hex = Some(key.clone());
        let (image, _) = unlock(&source, credentials, CancellationToken::new()).unwrap();
        assert_eq!(image.read_range(0, expected.len()).unwrap(), expected);
        let mut conflict = options(EncryptionScheme::Luks);
        conflict.master_key_hex = Some(key.clone());
        assert!(unlock(&source, conflict, CancellationToken::new()).is_err());
        for invalid in ["", "0", "invalid", &"ab".repeat(257), &"00".repeat(64)] {
            let mut credentials = options(EncryptionScheme::Luks);
            credentials.password.clear();
            credentials.master_key_hex = Some(invalid.into());
            assert!(unlock(&source, credentials, CancellationToken::new()).is_err());
        }
    }
}

#[test]
fn independent_luks_compatibility_corpus_is_accepted_by_application_validation() {
    use base64::Engine;
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../vendor/luks/tests/data/compatibility.json"
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
        credentials.password = "LovelyCarve-密码".into();
        let (image, _) = unlock(&source, credentials, CancellationToken::new())
            .unwrap_or_else(|error| panic!("{} / {}: {error:#}", case["cipher"], case["mode"]));
        assert_eq!(image.read_range(507, 1029).unwrap(), expected[507..1536]);
    }
}

fn options(scheme: EncryptionScheme) -> UnlockOptions {
    UnlockOptions {
        target: Default::default(),
        scheme,
        password: "lovelycarve-test".into(),
        master_key_hex: None,
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
    wrong.password = "wrong".into();
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
        assert!(
            crate::evidence::crypto::luks_metadata::validate(&mut reader, bytes.len() as u64)
                .is_err()
        );
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
        let error = crate::evidence::crypto::luks_metadata::validate(
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
    use crate::catalog::snapshot::volume_snapshot;
    use crate::filesystem::evidence_session::EvidenceSession;
    use crate::filesystem::tree::fs_tree;

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
            .attach_decrypted_partition(2, plain, result.read_cancellation)
            .unwrap();
        token.cancel(); // The completed view outlives its initiating task.
        assert_eq!(source.read_range(0, 4096).unwrap(), raw);
        assert_eq!(source.info().id, disk.id);
        assert_eq!(source.info().unlocked_partitions, vec![2]);
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
    use crate::evidence::crypto::filevault::{inspect, InspectOptions};
    use crate::evidence::partitions::PartitionKind;
    use crate::filesystem::{evidence_session::EvidenceSession, tree::fs_tree};
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
    credentials.password = "fvde-TEST".into();
    credentials.logical_volume_index = Some(0);
    credentials.partition_id = Some(2);
    let (plain, result) = unlock(&disk, credentials, CancellationToken::new()).unwrap();
    assert_eq!(&plain.read_range(1024, 4).unwrap(), b"H+\0\x04");
    let source = Arc::new(EvidenceSession::from(Arc::clone(&disk)));
    source
        .attach_decrypted_partition(2, plain, result.read_cancellation)
        .unwrap();
    assert_eq!(source.info().id, disk.id);
    assert_eq!(source.info().unlocked_partitions, vec![2]);
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
