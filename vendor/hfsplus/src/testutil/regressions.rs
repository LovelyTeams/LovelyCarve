//! Multi-extent and malformed-image regression corpus, constructed field by field.
use super::*;
use crate::{HeaderCopy, HfsVolume, unicode};
use std::io::{Cursor, Read, Seek, SeekFrom};

fn put32(image: &mut [u8], offset: usize, value: u32) {
    image[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}
fn file_record_offset(image: &[u8]) -> usize {
    let node = &image[3 * BLOCK_SIZE..4 * BLOCK_SIZE];
    let count = u16::from_be_bytes([node[10], node[11]]) as usize;
    for index in 0..count {
        let at = BLOCK_SIZE - 2 * (index + 1);
        let start = u16::from_be_bytes([node[at], node[at + 1]]) as usize;
        let key_len = u16::from_be_bytes([node[start], node[start + 1]]) as usize;
        let value = start + 2 + key_len;
        if node[value..value + 2] == [0, 2] {
            return 3 * BLOCK_SIZE + value;
        }
    }
    panic!("missing file record")
}
fn overflow_record(
    fork_type: u8,
    file_id: u32,
    logical: u32,
    physical: u32,
    count: u32,
) -> Vec<u8> {
    let mut record = 10u16.to_be_bytes().to_vec();
    record.extend_from_slice(&[fork_type, 0]);
    push_u32(&mut record, file_id);
    push_u32(&mut record, logical);
    push_u32(&mut record, physical);
    push_u32(&mut record, count);
    record.resize(76, 0);
    record
}
fn fragmented_image(resource: bool) -> (Vec<u8>, Vec<u8>) {
    let content: Vec<u8> = (0..10 * BLOCK_SIZE)
        .map(|index| (index / 512) as u8)
        .collect();
    let mut builder = HfsPlusImageBuilder::new();
    if resource {
        builder.add_file_with_xattrs("f", b"data", 0o644, &[], &content);
    } else {
        builder.add_file("f", &content, 0o644);
    }
    let mut image = builder.build();
    let leaf = image.len() / BLOCK_SIZE;
    image.resize(image.len() + BLOCK_SIZE, 0);
    put32(&mut image, 1024 + 44, (leaf + 1) as u32);
    let record = file_record_offset(&image);
    let fork = record + if resource { 168 } else { 88 };
    let physical = u32::from_be_bytes(image[fork + 16..fork + 20].try_into().unwrap());
    put32(&mut image, fork + 20, 8);
    // Extents tree node 1 lives in a separate physical extent, not next to node 0.
    image[1024 + 192..1024 + 200].copy_from_slice(&(2 * BLOCK_SIZE as u64).to_be_bytes());
    put32(&mut image, 1024 + 192 + 12, 2);
    put32(&mut image, 1024 + 192 + 24, leaf as u32);
    put32(&mut image, 1024 + 192 + 28, 1);
    write_btree_header_node(&mut image, BLOCK_SIZE, 1, 1, 10, 0);
    let overflow = overflow_record(if resource { 0xff } else { 0 }, 16, 8, physical + 8, 2);
    write_leaf_node(&mut image, leaf * BLOCK_SIZE, &[overflow]);
    (image, content)
}
#[test]
fn overflow_full_stream_and_seek_agree() {
    let (image, content) = fragmented_image(false);
    let mut volume = HfsVolume::open(Cursor::new(image)).unwrap();
    assert_eq!(volume.read_file("/f").unwrap(), content);
    let mut stream = Vec::new();
    assert_eq!(
        volume.read_file_to("/f", &mut stream).unwrap(),
        content.len() as u64
    );
    assert_eq!(stream, content);
    let mut file = volume.open_file("/f").unwrap();
    file.seek(SeekFrom::Start((8 * BLOCK_SIZE - 9) as u64))
        .unwrap();
    let mut output = [0; 31];
    file.read_exact(&mut output).unwrap();
    assert_eq!(output, content[8 * BLOCK_SIZE - 9..8 * BLOCK_SIZE + 22]);
    assert!(file.seek(SeekFrom::Current(i64::MIN)).is_err());
    assert_eq!(file.seek(SeekFrom::Start(u64::MAX)).unwrap(), u64::MAX);
    assert!(file.seek(SeekFrom::Current(1)).is_err());
}
#[test]
fn resource_overflow_uses_resource_key() {
    let (image, content) = fragmented_image(true);
    let mut volume = HfsVolume::open(Cursor::new(image)).unwrap();
    let mut output = Vec::new();
    volume
        .open_resource_fork("/f")
        .unwrap()
        .read_to_end(&mut output)
        .unwrap();
    assert_eq!(output, content);
    assert_eq!(volume.read_file("/f").unwrap(), b"data");
}
#[test]
fn missing_and_out_of_volume_overflow_are_errors() {
    let (mut image, _) = fragmented_image(false);
    let leaf = image.len() - BLOCK_SIZE;
    // Change the overflow key's logical start: no exact next extent exists.
    put32(&mut image, leaf + 14 + 8, 9);
    let mut volume = HfsVolume::open(Cursor::new(image)).unwrap();
    assert!(volume.read_file("/f").is_err());
    let (mut image, _) = fragmented_image(false);
    let leaf = image.len() - BLOCK_SIZE;
    put32(&mut image, leaf + 14 + 12, u32::MAX);
    let mut volume = HfsVolume::open(Cursor::new(image)).unwrap();
    assert!(volume.open_file("/f").is_err());
}
#[test]
fn rejects_geometry_truncation_and_signature_version_mismatch() {
    let image = HfsPlusImageBuilder::new()
        .add_file("f", b"bytes", 0o644)
        .build();
    for field in [40, 44] {
        let mut invalid = image.clone();
        put32(&mut invalid, 1024 + field, u32::MAX);
        assert!(HfsVolume::open(Cursor::new(invalid)).is_err());
    }
    let mut invalid = image.clone();
    invalid[1024 + 2..1024 + 4].copy_from_slice(&4u16.to_be_bytes());
    assert!(HfsVolume::open(Cursor::new(invalid)).is_err());
    let mut invalid = image;
    invalid.pop();
    assert!(HfsVolume::open(Cursor::new(invalid)).is_err());
}
#[test]
fn alternate_header_recovery_preserves_provenance() {
    let mut image = HfsPlusImageBuilder::new()
        .add_file("f", b"body", 0o644)
        .build();
    let header = image[1024..1536].to_vec();
    let alternate = image.len() - 1024;
    image[alternate..alternate + 512].copy_from_slice(&header);
    image[1024] = 0;
    let mut volume = HfsVolume::open(Cursor::new(image)).unwrap();
    assert_eq!(volume.header_copy, HeaderCopy::Alternate);
    assert_eq!(volume.read_file("/f").unwrap(), b"body");
}
#[test]
fn cyclic_leaf_chain_and_malformed_offsets_fail() {
    let mut image = HfsPlusImageBuilder::new().build();
    put32(&mut image, 3 * BLOCK_SIZE, 1);
    let mut volume = HfsVolume::open(Cursor::new(image)).unwrap();
    assert!(volume.list_directory("/").is_err());
    let mut image = HfsPlusImageBuilder::new().build();
    write_u16_at(&mut image, 4 * BLOCK_SIZE - 2, 0);
    let mut volume = HfsVolume::open(Cursor::new(image)).unwrap();
    assert!(volume.list_directory("/").is_err());
}
#[test]
fn hfsx_casefold_and_unicode32_names() {
    let mut image = HfsPlusImageBuilder::new()
        .add_file("e\u{301}", b"nfd", 0o644)
        .build();
    // HFSX case folding is selected by the Catalog header, not signature alone.
    image[2 * BLOCK_SIZE + 14 + 37] = 0xcf;
    let mut volume = HfsVolume::open(Cursor::new(image)).unwrap();
    assert_eq!(volume.read_file("/É").unwrap(), b"nfd");
    assert_eq!(
        unicode::compare_case_insensitive(
            &unicode::string_to_utf16("АБВ"),
            &unicode::string_to_utf16("абв")
        ),
        std::cmp::Ordering::Equal
    );
    assert_eq!(
        unicode::compare_case_insensitive(
            &[b'a' as u16, 0x200c, b'b' as u16],
            &[b'A' as u16, b'B' as u16]
        ),
        std::cmp::Ordering::Equal
    );
    assert_eq!(unicode::string_to_utf16("가"), vec![0x1100, 0x1161]);
    assert_eq!(unicode::string_to_utf16("\u{212b}"), vec![0x212b]);
}
#[test]
fn cnid_access_resource_xattr_and_parent_identity() {
    let image = HfsPlusImageBuilder::new()
        .add_file_with_xattrs("f", b"data", 0o644, &[("tag", b"value")], b"resource")
        .build();
    let mut volume = HfsVolume::open(Cursor::new(image)).unwrap();
    let metadata = volume.entry_by_id(16).unwrap();
    assert_eq!(metadata.name, "f");
    assert_eq!(metadata.parent_identifier, 2);
    assert_eq!(volume.volume_name().unwrap(), "TestVolume");
    let mut output = Vec::new();
    volume
        .open_file_by_id(16)
        .unwrap()
        .read_to_end(&mut output)
        .unwrap();
    assert_eq!(output, b"data");
    let mut attribute = volume.open_xattr("/f", "tag").unwrap().unwrap();
    attribute.seek(SeekFrom::Start(2)).unwrap();
    let mut output = Vec::new();
    attribute.read_to_end(&mut output).unwrap();
    assert_eq!(output, b"lue");
    assert!(volume.open_xattr("/f", "absent").unwrap().is_none());
}
#[test]
fn attribute_continuations_live_in_attributes_tree() {
    let mut image = HfsPlusImageBuilder::new()
        .add_file("f", b"body", 0o644)
        .build();
    let first = image.len() / BLOCK_SIZE;
    image.resize(image.len() + 2 * BLOCK_SIZE, 0);
    put32(&mut image, 1024 + 44, (first + 2) as u32);
    image[first * BLOCK_SIZE..(first + 1) * BLOCK_SIZE].fill(b'a');
    image[(first + 1) * BLOCK_SIZE..].fill(b'b');
    let mut initial = build_inline_attribute(16, "long", b"");
    let value = initial.len() - 16;
    initial.truncate(value);
    push_u32(&mut initial, 0x20);
    push_u32(&mut initial, 0);
    push_fork_data(&mut initial, (BLOCK_SIZE + 7) as u64, 2, first as u32, 1);
    let mut continuation = build_inline_attribute(16, "long", b"");
    let value = continuation.len() - 16;
    continuation.truncate(value);
    put32(&mut continuation, 8, 1);
    push_u32(&mut continuation, 0x30);
    push_u32(&mut continuation, 0);
    push_u32(&mut continuation, (first + 1) as u32);
    push_u32(&mut continuation, 1);
    continuation.resize(continuation.len() + 56, 0);
    write_leaf_node(&mut image, 5 * BLOCK_SIZE, &[initial, continuation]);
    write_btree_header_node(&mut image, 4 * BLOCK_SIZE, 1, 2, 266, 0);
    let mut volume = HfsVolume::open(Cursor::new(image)).unwrap();
    let value = volume.get_xattr("/f", "long").unwrap().unwrap();
    assert_eq!(value.len(), BLOCK_SIZE + 7);
    assert!(value[..BLOCK_SIZE].iter().all(|b| *b == b'a'));
    assert_eq!(&value[BLOCK_SIZE..], b"bbbbbbb");
    assert_eq!(volume.list_xattrs("/f").unwrap().len(), 1);
}

#[test]
fn hardlinks_resolve_backing_data_but_keep_entry_identity() {
    let mut image = HfsPlusImageBuilder::new()
        .add_file("f", b"hardlink body", 0o644)
        .build();
    let offset = file_record_offset(&image);
    let original = image[offset..offset + 248].to_vec();
    let mut backing = original.clone();
    put32(&mut backing, 8, 20);
    put32(&mut backing, 44, 2);
    let mut alias = original;
    alias[2..4].copy_from_slice(&0x20u16.to_be_bytes());
    put32(&mut alias, 44, 20);
    alias[48..56].copy_from_slice(b"hlnkhfs+");
    alias[88..168].fill(0);
    let records = vec![
        build_catalog_entry(1, "TestVolume", &build_folder_record(2, 2, 0o755)),
        build_catalog_entry(
            2,
            "",
            &build_thread_record(RECORD_TYPE_FOLDER_THREAD, 1, "TestVolume"),
        ),
        build_catalog_entry(
            2,
            "\0\0\0\0HFS+ Private Data",
            &build_folder_record(18, 1, 0o700),
        ),
        build_catalog_entry(2, "f", &alias),
        build_catalog_entry(18, "iNode20", &backing),
    ];
    write_leaf_node(&mut image, 3 * BLOCK_SIZE, &records);
    write_btree_header_node(&mut image, 2 * BLOCK_SIZE, 1, 5, 516, 0xbc);
    let mut volume = HfsVolume::open(Cursor::new(image)).unwrap();
    let metadata = volume.entry_by_id(16).unwrap();
    assert_eq!(metadata.identifier, 16);
    assert_eq!(metadata.link_identifier, Some(20));
    assert_eq!(volume.read_file("/f").unwrap(), b"hardlink body");
    assert_eq!(volume.stat("/f").unwrap().permissions.special, 2);
    assert_eq!(volume.stat("/f").unwrap().link_count, Some(2));
    let listing = volume.list_directory("/").unwrap();
    let alias = listing.iter().find(|entry| entry.name == "f").unwrap();
    assert_eq!(alias.cnid, 16);
    assert_eq!(alias.size, 13);
}
#[test]
fn preserves_backup_added_and_bsd_fields() {
    let mut image = HfsPlusImageBuilder::new()
        .add_file("f", b"body", 0o644)
        .build();
    let offset = file_record_offset(&image);
    image[offset + 2..offset + 4].copy_from_slice(&0x80u16.to_be_bytes());
    put32(&mut image, offset + 28, 0xf1234567);
    put32(&mut image, offset + 32, 501);
    put32(&mut image, offset + 36, 20);
    put32(&mut image, offset + 68, u32::MAX);
    let mut volume = HfsVolume::open(Cursor::new(image)).unwrap();
    let stat = volume.stat("/f").unwrap();
    assert_eq!(stat.backup_date, 0xf1234567);
    assert_eq!(stat.added_time, Some(-1));
    assert_eq!(stat.permissions.owner_id, 501);
    assert_eq!(stat.permissions.group_id, 20);
}
#[test]
fn compressed_flag_without_attribute_cannot_yield_an_empty_file() {
    let mut image = HfsPlusImageBuilder::new().add_file("f", b"", 0o644).build();
    let offset = file_record_offset(&image);
    image[offset + 41] = 0x20;
    let mut volume = HfsVolume::open(Cursor::new(image)).unwrap();
    assert!(volume.read_file("/f").is_err());
}
