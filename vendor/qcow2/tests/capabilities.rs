use positioned_io::ReadAt;
use qcow2::{EncryptionKey, Error, Qcow2};
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    sync::Arc,
};

const CLUSTER: usize = 512;
fn put32(image: &mut [u8], offset: usize, value: u32) {
    image[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}
fn put64(image: &mut [u8], offset: usize, value: u64) {
    image[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
}
fn image(version: u32) -> Vec<u8> {
    let mut bytes = vec![0; CLUSTER * 5];
    put32(&mut bytes, 0, 0x5146_49fb);
    put32(&mut bytes, 4, version);
    put64(&mut bytes, 24, (CLUSTER * 3) as u64);
    put64(&mut bytes, 40, CLUSTER as u64);
    if version == 1 {
        bytes[32] = 9;
        bytes[33] = 6;
    } else {
        put32(&mut bytes, 20, 9);
        put32(&mut bytes, 36, 1);
    }
    if version == 3 {
        put32(&mut bytes, 96, 4);
        put32(&mut bytes, 100, 104);
    }
    put64(&mut bytes, CLUSTER, (CLUSTER * 2) as u64);
    put64(&mut bytes, CLUSTER * 2, (CLUSTER * 3) as u64);
    bytes[CLUSTER * 3..CLUSTER * 4].fill(0x31);
    bytes
}
fn backing(bytes: &mut [u8], name: &str) {
    put64(bytes, 8, 256);
    put32(bytes, 16, name.len() as u32);
    bytes[256..256 + name.len()].copy_from_slice(name.as_bytes());
}
fn fixture(name: &str) -> Vec<u8> {
    fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data")
            .join(name),
    )
    .unwrap()
}

#[test]
fn independent_deflate_fixtures_match_full_random_and_seek_reads() {
    let plain = fixture("plain.bin");
    for version in 1..=3 {
        let qcow = Qcow2::open(fixture(&format!("v{version}-deflate.qcow"))).unwrap();
        assert_eq!(qcow.info().version, version);
        let mut expected = plain.clone();
        expected.resize(8192, 0);
        let mut full = vec![0xff; 8192];
        assert_eq!(qcow.read_at(0, &mut full).unwrap(), 8192);
        assert_eq!(full, expected);
        for position in [0, 1, 497, 511, 512, 3979, 4095, 4096, 8125] {
            let mut bytes = vec![0xff; 333];
            let count = qcow.read_at(position, &mut bytes).unwrap();
            assert_eq!(
                &bytes[..count],
                &expected[position as usize..position as usize + count]
            );
        }
        let mut reader = qcow.reader().unwrap();
        reader.seek(SeekFrom::Start(3997)).unwrap();
        let mut cross = [0xff; 200];
        reader.read_exact(&mut cross).unwrap();
        assert_eq!(&cross, &expected[3997..4197]);
    }
}

#[test]
fn independent_aes_fixtures_match_unaligned_and_reverse_reads() {
    let mut expected = vec![0; 4096];
    expected.extend(fixture("plain.bin"));
    for version in 1..=3 {
        let bytes = fixture(&format!("v{version}-aes.qcow"));
        let locked = Qcow2::open(bytes.clone()).unwrap();
        assert!(locked.is_locked());
        assert_eq!(locked.info().encryption_method, 1);
        assert!(matches!(
            locked.read_at(0, &mut [0; 1]),
            Err(Error::KeyRequired)
        ));
        let qcow = Qcow2::open_with_key(bytes, EncryptionKey::from_password("qcow-test").unwrap())
            .unwrap();
        let mut full = vec![0; 8192];
        qcow.read_at(0, &mut full).unwrap();
        assert_eq!(full, expected);
        for position in [8001, 4607, 4096, 4097, 4499, 4095, 0] {
            let mut random = [0xff; 155];
            let count = qcow.read_at(position, &mut random).unwrap();
            assert_eq!(
                &random[..count],
                &expected[position as usize..position as usize + count]
            );
        }
        let qcow = Arc::new(qcow);
        let handles: Vec<_> = (0..4)
            .map(|index| {
                let qcow = Arc::clone(&qcow);
                let expected = expected.clone();
                std::thread::spawn(move || {
                    let mut bytes = [0; 517];
                    let position = 4111 + index * 531;
                    qcow.read_at(position as u64, &mut bytes).unwrap();
                    assert_eq!(
                        bytes.as_slice(),
                        &expected[position..position + bytes.len()]
                    );
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
    }
}

#[test]
fn version_one_has_independent_l2_geometry_and_unaligned_l1() {
    let mut bytes = image(1);
    bytes[33] = 7;
    put64(&mut bytes, 40, 48);
    put64(&mut bytes, 48, 512);
    bytes[512..1536].fill(0);
    put64(&mut bytes, 512, 1536);
    let qcow = Qcow2::open(bytes).unwrap();
    assert_eq!(qcow.read_at(0, &mut [0; 10]).unwrap(), 10);
}

#[test]
fn oversized_l1_and_modern_v3_headers_are_valid() {
    let mut bytes = image(3);
    put32(&mut bytes, 36, 2);
    put32(&mut bytes, 100, 120);
    put64(&mut bytes, 72, 1);
    put64(&mut bytes, 80, u64::MAX);
    put64(&mut bytes, 88, u64::MAX);
    let qcow = Qcow2::open(bytes).unwrap();
    assert_eq!(qcow.info().incompatible_features, 1);
    let mut output = [0; 16];
    qcow.read_at(0, &mut output).unwrap();
    assert_eq!(output, [0x31; 16]);
}

#[test]
fn backing_holes_inherit_but_zero_clusters_override_parent() {
    let mut bytes = image(3);
    backing(&mut bytes, "parent.raw");
    put64(&mut bytes, CLUSTER * 2 + 8, 1); // zero with no host allocation
    put64(&mut bytes, CLUSTER * 2 + 16, 0);
    let mut qcow = Qcow2::open(bytes).unwrap();
    assert!(matches!(
        qcow.read_at(1024, &mut [0; 1]),
        Err(Error::MissingBacking)
    ));
    qcow.set_backing(Arc::new(vec![0x79; 1400])).unwrap();
    let mut full = [0xff; 1536];
    qcow.read_at(0, &mut full).unwrap();
    assert_eq!(&full[..512], &[0x31; 512]);
    assert_eq!(&full[512..1024], &[0; 512]);
    assert_eq!(&full[1024..1400], &[0x79; 376]);
    assert_eq!(&full[1400..], &[0; 136]);
    assert!(qcow.set_backing(Arc::new(vec![0; 1536])).is_err());
}

#[test]
fn zero_allocated_and_unallocated_have_identical_bytes() {
    let mut bytes = image(3);
    put64(&mut bytes, CLUSTER * 2, 1536 | 1);
    let qcow = Qcow2::open(bytes).unwrap();
    let mut output = [0xff; 512];
    qcow.read_at(0, &mut output).unwrap();
    assert_eq!(output, [0; 512]);
    let mut bytes = image(3);
    put64(&mut bytes, CLUSTER * 2, 3072 | 1);
    assert!(Qcow2::open(bytes).unwrap().read_at(0, &mut output).is_err());
}

#[test]
fn relative_backing_chains_and_declared_raw_formats() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    fs::create_dir(root.join("sub")).unwrap();
    let mut raw = vec![0x99; 1536];
    raw[..4].copy_from_slice(b"QFI\xfb");
    fs::write(root.join("parent.raw"), raw).unwrap();
    let mut child = image(2);
    backing(&mut child, "../parent.raw");
    // Explicit RAW overrides signature detection, including a RAW QFI prefix.
    put32(&mut child, 72, 0xe279_2aca);
    put32(&mut child, 76, 3);
    child[80..83].copy_from_slice(b"raw");
    fs::write(root.join("sub/child.qcow2"), &child).unwrap();
    let qcow = Qcow2::open_path(root.join("sub/child.qcow2")).unwrap();
    let mut output = [0; 12];
    qcow.read_at(512, &mut output).unwrap();
    assert_eq!(output, [0x99; 12]);
    assert_eq!(qcow.info().backing_format.as_deref(), Some("raw"));
    let mut top = image(1);
    backing(&mut top, "sub/child.qcow2");
    fs::write(root.join("top.qcow"), top).unwrap();
    let qcow = Qcow2::open_path(root.join("top.qcow")).unwrap();
    qcow.read_at(1024, &mut output).unwrap();
    assert_eq!(output, [0x99; 12]);
}

#[test]
fn missing_parent_cycles_and_depth_limits_fail() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("child.qcow2");
    let mut child = image(2);
    backing(&mut child, "absent.qcow2");
    fs::write(&path, &child).unwrap();
    assert!(matches!(
        Qcow2::open_path(&path),
        Err(Error::BackingFile { .. })
    ));
    backing(&mut child, "child.qcow2");
    fs::write(&path, child).unwrap();
    assert!(Qcow2::open_path(&path)
        .unwrap_err()
        .to_string()
        .contains("cycle"));
    for index in 0..33 {
        let mut bytes = image(2);
        if index < 32 {
            backing(&mut bytes, &format!("{}.qcow2", index + 1));
        }
        fs::write(temporary.path().join(format!("{index}.qcow2")), bytes).unwrap();
    }
    assert!(Qcow2::open_path(temporary.path().join("0.qcow2"))
        .unwrap_err()
        .to_string()
        .contains("depth"));
}

#[test]
fn truncated_allocated_data_and_tables_never_turn_into_zeroes() {
    let mut bytes = image(2);
    bytes.truncate(1538);
    let qcow = Qcow2::open(bytes).unwrap();
    assert!(qcow.read_at(0, &mut [0; 3]).is_err());
    let mut bytes = image(2);
    put64(&mut bytes, CLUSTER, 2560);
    let qcow = Qcow2::open(bytes).unwrap();
    assert!(qcow.read_at(0, &mut [0; 1]).is_err());
    let mut bytes = image(2);
    put32(&mut bytes, 36, u32::MAX);
    assert!(Qcow2::open(bytes).is_err());
    let mut bytes = image(2);
    put64(&mut bytes, 40, u64::MAX - 511);
    assert!(Qcow2::open(bytes).is_err());
}

#[test]
fn malformed_descriptors_and_v2_zero_bit_are_rejected() {
    for (version, entry) in [
        (2, 1),
        (2, 1537),
        (3, 1536 | 2),
        (2, 1536 | (1 << 56)),
        (1, 1537),
    ] {
        let mut bytes = image(version);
        put64(&mut bytes, CLUSTER * 2, entry);
        let qcow = Qcow2::open(bytes).unwrap();
        assert!(qcow.read_at(0, &mut [0; 1]).is_err());
    }
    let mut bytes = image(2);
    put64(&mut bytes, CLUSTER, 1024 | (1 << 62));
    assert!(Qcow2::open(bytes).unwrap().read_at(0, &mut [0; 1]).is_err());
}

#[test]
fn corrupted_compression_is_never_published_as_complete() {
    for name in ["v2-deflate-short.qcow", "v2-deflate-long.qcow"] {
        let qcow = Qcow2::open(fixture(name)).unwrap();
        assert!(qcow.read_at(0, &mut [0; 1]).is_err());
    }
    for version in 1..=3 {
        let mut bytes = fixture(&format!("v{version}-deflate.qcow"));
        bytes.pop();
        let qcow = Qcow2::open(bytes).unwrap();
        assert!(qcow.read_at(0, &mut [0; 1]).is_err());
        let mut bytes = fixture(&format!("v{version}-deflate.qcow"));
        bytes[3 * 4096 + 497] = 0x07;
        let qcow = Qcow2::open(bytes).unwrap();
        assert!(qcow.read_at(0, &mut [0; 1]).is_err());
    }
}

#[test]
fn unsupported_features_and_named_unknown_bits_fail_explicitly() {
    for bit in [2, 4, 16, 1 << 63] {
        let mut bytes = image(3);
        put64(&mut bytes, 72, bit);
        assert!(Qcow2::open(bytes).is_err());
    }
    let mut bytes = image(3);
    put64(&mut bytes, 72, 1 << 40);
    put32(&mut bytes, 104, 0x6803_f857);
    put32(&mut bytes, 108, 48);
    bytes[113] = 40;
    bytes[114..120].copy_from_slice(b"future");
    assert!(Qcow2::open(bytes)
        .unwrap_err()
        .to_string()
        .contains("future"));
    for encryption in [2, 3] {
        let mut bytes = image(2);
        put32(&mut bytes, 32, encryption);
        assert!(Qcow2::open(bytes).is_err());
    }
    let mut bytes = image(3);
    put32(&mut bytes, 100, 112);
    bytes[104] = 1;
    put64(&mut bytes, 72, 8);
    assert!(Qcow2::open(bytes)
        .unwrap_err()
        .to_string()
        .contains("compression type"));
}

#[test]
fn extension_and_backing_name_ranges_are_bounded() {
    let mut bytes = image(3);
    put32(&mut bytes, 104, 0x9999);
    put32(&mut bytes, 108, u32::MAX);
    assert!(Qcow2::open(bytes).is_err());
    let mut bytes = image(2);
    backing(&mut bytes, "parent");
    put32(&mut bytes, 72, 0x9999);
    put32(&mut bytes, 76, 180);
    assert!(Qcow2::open(bytes).is_err());
    let mut bytes = image(2);
    backing(&mut bytes, "parent");
    bytes[259] = 0;
    assert!(Qcow2::open(bytes).is_err());
    let mut bytes = image(2);
    put32(&mut bytes, 72, 0x9999);
    put32(&mut bytes, 80, 0x9999);
    assert!(Qcow2::open(bytes).is_err());
}

#[test]
fn cancellation_seek_and_eof_preserve_read_contracts() {
    let qcow = Qcow2::open(image(2)).unwrap();
    assert!(matches!(
        qcow.read_at_cancellable(0, &mut [0; 12], &|| true),
        Err(Error::Cancelled)
    ));
    let calls = std::cell::Cell::new(0);
    assert!(matches!(
        qcow.read_at_cancellable(0, &mut [0; 1536], &|| {
            let call = calls.get() + 1;
            calls.set(call);
            call >= 4
        }),
        Err(Error::Cancelled)
    ));
    let mut reader = qcow.reader().unwrap();
    let mut bytes = [0; 16];
    reader.read_exact(&mut bytes).unwrap();
    assert_eq!(reader.position(), 16);
    reader.read_exact_at(0, &mut bytes).unwrap();
    assert_eq!(reader.position(), 16);
    assert!(reader.seek(SeekFrom::Current(-17)).is_err());
    assert_eq!(reader.position(), 16);
    assert!(reader.seek(SeekFrom::Start(u64::MAX)).is_err());
    assert_eq!(reader.seek(SeekFrom::End(-2)).unwrap(), 1534);
    assert_eq!(reader.read(&mut bytes).unwrap(), 2);
    assert_eq!(reader.read(&mut bytes).unwrap(), 0);
    assert_eq!(qcow.read_at(u64::MAX, &mut bytes).unwrap(), 0);
}

#[test]
fn signature_probe_and_password_encodings() {
    assert!(!qcow2::check_signature(&vec![0; 3]).unwrap());
    assert!(qcow2::check_signature(&image(1)).unwrap());
    assert!(EncryptionKey::from_password("中文").is_err());
    assert!(EncryptionKey::from_password("a\0b").is_err());
    assert!(EncryptionKey::from_utf16_password(&[0xd800]).is_err());
    let key = EncryptionKey::from_utf16_password(&"qcow-test".encode_utf16().collect::<Vec<_>>())
        .unwrap();
    let qcow = Qcow2::open_with_key(fixture("v2-aes.qcow"), key).unwrap();
    let mut bytes = [0; 100];
    qcow.read_at(4096, &mut bytes).unwrap();
    assert_eq!(bytes.as_slice(), &fixture("plain.bin")[..100]);
    assert!(!format!("{qcow:?}").contains("qcow-test"));
}

#[test]
fn unicode_backing_paths_and_snapshot_count_metadata() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("检材 空格");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("父镜像.raw"), vec![0x99; 1536]).unwrap();
    let mut bytes = image(3);
    backing(&mut bytes, "父镜像.raw");
    put32(&mut bytes, 60, 3);
    put64(&mut bytes, 64, 2048);
    fs::write(root.join("子镜像.qcow2"), bytes).unwrap();
    let qcow = Qcow2::open_path(root.join("子镜像.qcow2")).unwrap();
    assert_eq!(qcow.info().snapshot_count, 3);
    let mut output = [0; 100];
    qcow.read_at(512, &mut output).unwrap();
    assert_eq!(output, [0x99; 100]);
}
