use super::*;
use crate::test_support::{self, build_image, BLOCK, KEY1, PASSWORD, PV_ID};
use std::io::{Cursor, Read, Seek, SeekFrom};

fn metadata(image: &[u8]) -> Vec<u8> {
    let mut bytes = image[8 * BLOCK..14 * BLOCK].to_vec();
    crate::xts::decrypt_units(&mut bytes, &KEY1, &PV_ID, 8192, 0);
    bytes
}
fn replace_metadata(image: &mut [u8], mut bytes: Vec<u8>) {
    for object in bytes.chunks_exact_mut(8192) {
        test_support::seal_object(object);
    }
    crate::xts::encrypt_units(&mut bytes, &KEY1, &PV_ID, 8192, 0);
    image[8 * BLOCK..8 * BLOCK + bytes.len()].copy_from_slice(&bytes);
}
fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn put64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
fn replace_plist(object: &mut [u8], kind: u16, text: &[u8]) {
    let (sizes, offset) = if kind == 0x19 { (104, 112) } else { (120, 128) };
    put32(object, sizes, text.len() as u32);
    put32(object, sizes + 4, text.len() as u32);
    put32(object, offset, 184);
    put32(object, offset + 4, text.len() as u32);
    object[184..].fill(0);
    object[184..184 + text.len()].copy_from_slice(text);
}
#[test]
fn strict_synthetic_inventory_and_fragmented_reads() {
    let mut image = build_image();
    let mut meta = metadata(&image);
    // Two non-contiguous physical extents, with logical tweaks continuing across the split.
    let tail = image[18 * BLOCK..20 * BLOCK].to_vec();
    image[14 * BLOCK..16 * BLOCK].copy_from_slice(&tail);
    let map = &mut meta[2 * 8192..];
    put32(map, 64, 2);
    put32(map, 72 + 16, 2);
    put64(map, 112 + 8, 2);
    put32(map, 112 + 16, 2);
    put64(map, 112 + 32, 14);
    replace_metadata(&mut image, meta);
    let container = Container::open(Cursor::new(image), None).unwrap();
    assert_eq!(container.volumes().next().unwrap().segments.len(), 2);
    let mut volume = container.unlock_with_password(0, PASSWORD).unwrap();
    let mut full = Vec::new();
    volume.read_to_end(&mut full).unwrap();
    let expected: Vec<u8> = (0..4 * BLOCK).map(|index| (index & 255) as u8).collect();
    assert_eq!(full, expected);
    volume
        .seek(SeekFrom::Start((2 * BLOCK - 7) as u64))
        .unwrap();
    let mut range = [0; 29];
    volume.read_exact(&mut range).unwrap();
    assert_eq!(&range[..], &expected[2 * BLOCK - 7..2 * BLOCK + 22]);
}
#[test]
fn rejects_cross_member_holes_overlaps_and_truncated_maps() {
    for (offset, value) in [
        (72 + 32, 16 | (1 << 48)),
        (72 + 8, 1),
        (64, u64::from(u32::MAX)),
    ] {
        let mut image = build_image();
        let mut meta = metadata(&image);
        put64(&mut meta, 2 * 8192 + offset, value);
        replace_metadata(&mut image, meta);
        assert!(Container::open(Cursor::new(image), None).is_err());
    }
}
#[test]
fn checksum_damage_and_truncated_ciphertext_do_not_fabricate_bytes() {
    let mut image = build_image();
    image[96] ^= 1;
    assert!(Container::open(Cursor::new(image), None).is_err());
    let mut image = build_image();
    image[8 * BLOCK + 100] ^= 1;
    assert!(Container::open(Cursor::new(image), None).is_err());
    let mut image = build_image();
    image.truncate(image.len() - 1);
    assert!(Container::open(Cursor::new(image), None).is_err());
    let keys = crate::unlock::keys_from_master([1; 16], &[2; 16]);
    let mut reader = DecryptedVolume::with_segments(
        Cursor::new(vec![0; 511]),
        keys,
        vec![Segment {
            logical_offset: 0,
            physical_offset: 0,
            length: 512,
        }],
        512,
        512,
    )
    .unwrap();
    assert!(reader.read_at(0, &mut [0; 512]).is_err());
}
#[test]
fn valid_backup_header_recovers_corrupt_primary_with_provenance() {
    let mut image = build_image();
    let header = image[..512].to_vec();
    let end = image.len() - 512;
    image[end..].copy_from_slice(&header);
    image[0] ^= 1;
    let container = Container::open(Cursor::new(image), None).unwrap();
    assert_eq!(container.header_offset, end as u64);
}
#[test]
fn conversion_states_and_cancellation_fail_explicitly() {
    let image = test_support::build_image_with_parameters(1000, "Converting");
    let container = Container::open(Cursor::new(image), None).unwrap();
    assert_eq!(
        container
            .volumes()
            .next()
            .unwrap()
            .info
            .conversion_status
            .as_deref(),
        Some("Converting")
    );
    assert!(container.unlock_with_password(0, PASSWORD).is_err());
    let container = Container::open(Cursor::new(build_image()), None).unwrap();
    assert!(container
        .unlock_with_password_cancellable(0, PASSWORD, || true)
        .is_err());
    assert!(crate::unlock::passphrase_key("password", &[0; 16], 0).is_err());
    assert!(crate::unlock::passphrase_key("password", &[0; 16], 2_000_001).is_err());
}
#[test]
fn recovery_and_later_password_protectors_are_tried_without_cross_association() {
    use base64::Engine;
    let mut image = build_image();
    let mut meta = metadata(&image);
    let root = crate::storage_plist::parse(&meta[184..184 + le_u32(&meta, 104) as usize]).unwrap();
    let user = &root.get("CryptoUsers").unwrap().array()[0];
    let pw = base64::engine::general_purpose::STANDARD.encode(
        user.get("PassphraseWrappedKEKStruct")
            .unwrap()
            .data()
            .unwrap(),
    );
    let key = base64::engine::general_purpose::STANDARD.encode(
        user.get("KEKWrappedVolumeKeyStruct")
            .unwrap()
            .data()
            .unwrap(),
    );
    // An unrelated first user has the wrong KEK identifier; the recovery protector follows.
    let xml=format!("<dict><key>CryptoUsers</key><array><dict><key>UserType</key><integer>268435457</integer><key>KeyEncryptingKeyIdent</key><string>other</string><key>PassphraseWrappedKEKStruct</key><data>{pw}</data></dict><dict><key>UserType</key><integer>268435465</integer><key>KeyEncryptingKeyIdent</key><string>right</string><key>PassphraseWrappedKEKStruct</key><data>{pw}</data></dict></array><key>WrappedVolumeKeys</key><array><dict><key>KeyEncryptingKeyIdent</key><string>right</string><key>BlockAlgorithm</key><string>AES-XTS</string><key>KEKWrappedVolumeKeyStruct</key><data>{key}</data></dict></array><key>ConversionStatus</key><string>Complete</string></dict>");
    replace_plist(&mut meta[..8192], 0x19, xml.as_bytes());
    replace_metadata(&mut image, meta);
    let container = Container::open(Cursor::new(image), None).unwrap();
    assert_eq!(
        container.volumes().next().unwrap().info.protectors[1].kind,
        ProtectorKind::Recovery
    );
    assert!(container.unlock_with_password(0, PASSWORD).is_ok());
}
#[test]
fn cf_references_entities_duplicates_and_bombs_are_checked() {
    let value=crate::storage_plist::parse(b"<dict><key>a</key><string ID=\"1\">A&amp;B</string><key>b</key><reference IDREF=\"1\"/></dict>").unwrap();
    assert_eq!(value.get("b").unwrap().string(), Some("A&B"));
    assert!(crate::storage_plist::parse(
        b"<dict><key>a</key><integer>1</integer><key>a</key><integer>2</integer></dict>"
    )
    .is_err());
    assert!(crate::storage_plist::parse(b"<reference IDREF=\"0\"/>").is_err());
    let mut xml = String::from("<array><string ID=\"0\">x</string>");
    for index in 1..28 {
        xml.push_str(&format!(
            "<array ID=\"{index}\"><reference IDREF=\"{}\"/><reference IDREF=\"{}\"/></array>",
            index - 1,
            index - 1
        ));
    }
    xml.push_str("</array>");
    assert!(crate::storage_plist::parse(xml.as_bytes()).is_err());
}
#[test]
fn deflate_plist_is_length_checked_and_never_silently_truncated() {
    use std::io::Write;
    let mut image = build_image();
    let mut meta = metadata(&image);
    let plain = meta[184..184 + le_u32(&meta, 104) as usize].to_vec();
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&plain).unwrap();
    let compressed = encoder.finish().unwrap();
    replace_plist(&mut meta[..8192], 0x19, &compressed);
    put32(&mut meta, 108, plain.len() as u32);
    replace_metadata(&mut image, meta.clone());
    assert!(Container::open(Cursor::new(image), None).is_ok());
    let mut image = build_image();
    put32(&mut meta, 108, plain.len() as u32 - 1);
    replace_metadata(&mut image, meta);
    assert!(Container::open(Cursor::new(image), None).is_err());
}
#[test]
fn external_plaintext_and_wipekey_contexts_match_internal_context() {
    let image = build_image();
    let meta = metadata(&image);
    let context = meta[184..184 + le_u32(&meta, 104) as usize].to_vec();
    let container = Container::open(Cursor::new(image.clone()), Some(&context)).unwrap();
    assert!(container.unlock_with_password(0, PASSWORD).is_ok());
    let mut encrypted = context.clone();
    crate::xts::encrypt_units(&mut encrypted, &PV_ID, &[0; 16], context.len(), 0);
    let container = Container::open(Cursor::new(image), Some(&encrypted)).unwrap();
    assert!(container.unlock_with_password(0, PASSWORD).is_ok());
}
#[test]
fn recovered_keys_require_a_valid_plaintext_header_and_debug_is_redacted() {
    let keys = crate::unlock::keys_from_master(test_support::VMK, &test_support::FAMILY);
    assert_eq!(format!("{keys:?}"), "VolumeKeys { [REDACTED] }");
    let container = Container::open(Cursor::new(build_image()), None).unwrap();
    assert!(container
        .unlock_with_master_key(0, &test_support::VMK)
        .is_err());
    assert_eq!(crate::read::le_u64(&[], usize::MAX), 0);
    assert_eq!(crate::read::bytes16(&[], usize::MAX), [0; 16]);
}

fn expand_metadata(image: &mut Vec<u8>, mut meta: Vec<u8>) {
    for object in meta.chunks_exact_mut(8192) {
        test_support::seal_object(object);
    }
    let mut pointer = image[BLOCK..BLOCK + 8192].to_vec();
    put64(&mut pointer, 208, (meta.len() / BLOCK) as u64);
    test_support::seal_object(&mut pointer);
    image[BLOCK..BLOCK + 8192].copy_from_slice(&pointer);
    crate::xts::encrypt_units(&mut meta, &KEY1, &PV_ID, 8192, 0);
    image[8 * BLOCK..8 * BLOCK + meta.len()].copy_from_slice(&meta);
}
#[test]
fn multiple_logical_volumes_require_explicit_selection_and_keep_identity() {
    let mut image = build_image();
    let mut meta = metadata(&image);
    let mut second = meta[8192..16384].to_vec();
    put64(&mut second, 24, 12);
    let size = le_u32(&second, 120) as usize;
    let xml = String::from_utf8(second[184..184 + size].to_vec())
        .unwrap()
        .replace(
            "00000000-0000-0000-0000-000000000001",
            "00000000-0000-0000-0000-000000000002",
        )
        .replace("SynthLV", "SecondLV");
    replace_plist(&mut second, 0x1a, xml.as_bytes());
    meta.extend(second);
    expand_metadata(&mut image, meta);
    let container = Container::open(Cursor::new(image.clone()), None).unwrap();
    let names: Vec<_> = container
        .volumes()
        .map(|volume| volume.info.lv_name.clone().unwrap())
        .collect();
    assert_eq!(names, vec!["SynthLV", "SecondLV"]);
    assert_eq!(
        container
            .unlock_with_password(1, PASSWORD)
            .unwrap()
            .info()
            .lv_name
            .as_deref(),
        Some("SecondLV")
    );
    assert!(crate::FileVaultVolume::unlock_with_password(Cursor::new(image), PASSWORD).is_err());
}
#[test]
fn secondary_encrypted_metadata_recovers_primary_damage() {
    let mut image = build_image();
    let encrypted = image[8 * BLOCK..14 * BLOCK].to_vec();
    image.resize(26 * BLOCK, 0);
    image[20 * BLOCK..26 * BLOCK].copy_from_slice(&encrypted);
    put64(&mut image, 64, (26 * BLOCK) as u64);
    let crc = crate::storage_metadata::checksum(&image[8..512]);
    put32(&mut image, 0, crc);
    let mut pointer = image[BLOCK..BLOCK + 8192].to_vec();
    put64(&mut pointer, 240, 20);
    test_support::seal_object(&mut pointer);
    image[BLOCK..BLOCK + 8192].copy_from_slice(&pointer);
    image[8 * BLOCK + 100] ^= 1;
    let container = Container::open(Cursor::new(image), None).unwrap();
    assert_eq!(container.encrypted_metadata_offset, (20 * BLOCK) as u64);
    assert!(container.unlock_with_password(0, PASSWORD).is_ok());
}
#[test]
fn fragmented_context_plist_continuations_are_assembled_by_object_id() {
    let mut image = build_image();
    let mut meta = metadata(&image);
    let size = le_u32(&meta, 104) as usize;
    let xml = meta[184..184 + size].to_vec();
    let split = size / 2;
    let mut continuation = vec![0; 8192];
    continuation[10..12].copy_from_slice(&0x24u16.to_le_bytes());
    put64(&mut continuation, 24, 13);
    put32(&mut continuation, 72, (size - split) as u32);
    continuation[80..80 + size - split].copy_from_slice(&xml[split..]);
    put64(&mut meta, 96, 13);
    put32(&mut meta, 116, split as u32);
    meta[184 + split..8192].fill(0);
    meta.extend(continuation);
    expand_metadata(&mut image, meta);
    let container = Container::open(Cursor::new(image), None).unwrap();
    assert!(container.unlock_with_password(0, PASSWORD).is_ok());
}
#[test]
fn newer_extent_objects_cannot_replace_an_older_volume_transaction() {
    let mut image = build_image();
    let mut meta = metadata(&image);
    let mut future = meta[2 * 8192..3 * 8192].to_vec();
    put64(&mut future, 72 + 32, u64::MAX);
    meta.extend(future);
    expand_metadata(&mut image, meta);
    // Seal uses transaction 1; change only the new object's transaction and checksum.
    let mut encoded = image[8 * BLOCK..16 * BLOCK].to_vec();
    crate::xts::decrypt_units(&mut encoded, &KEY1, &PV_ID, 8192, 0);
    let object = &mut encoded[3 * 8192..];
    put64(object, 16, 2);
    let crc = crate::storage_metadata::checksum(&object[8..]);
    put32(object, 0, crc);
    crate::xts::encrypt_units(&mut encoded, &KEY1, &PV_ID, 8192, 0);
    image[8 * BLOCK..16 * BLOCK].copy_from_slice(&encoded);
    let container = Container::open(Cursor::new(image), None).unwrap();
    assert!(container.unlock_with_password(0, PASSWORD).is_ok());
}
