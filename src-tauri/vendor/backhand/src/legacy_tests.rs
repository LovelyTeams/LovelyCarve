use super::*;
use crate::traits::filesystem::BackhandInnerNode;
use std::io::{BufReader, Cursor};

#[test]
#[ignore = "set SQUASHFS_LEGACY_EXTERNAL to semicolon-separated public v1/v2 images"]
fn legacy_external_walks_files_and_exact_ranges() {
    let paths =
        std::env::var("SQUASHFS_LEGACY_EXTERNAL").expect("SQUASHFS_LEGACY_EXTERNAL required");
    for path in paths.split(';') {
        let reader = BufReader::new(std::fs::File::open(path).unwrap());
        let image = FilesystemReader::from_reader(reader).unwrap();
        let mut count = 0;
        for node in image.files() {
            if let BackhandInnerNode::File(file) = node.inner {
                let bytes = image.file_data(&file).unwrap_or_else(|error| {
                    panic!("{path}: {}: {error:?}", node.fullpath.display())
                });
                assert_eq!(bytes.len(), file.file_len());
                for offset in [
                    0,
                    1,
                    4095,
                    32765,
                    65533,
                    bytes.len() / 2,
                    bytes.len(),
                    bytes.len() + 1,
                ] {
                    let start = offset.min(bytes.len());
                    assert_eq!(
                        image.file_data_range(&file, offset as u64, 8193).unwrap(),
                        bytes[start..(start + 8193).min(bytes.len())]
                    );
                }
                let mut streamed = Vec::new();
                image.file_data_to_writer(&file, &mut streamed).unwrap();
                assert_eq!(streamed, bytes);
                if image.header.version == 1 {
                    assert!(matches!(
                        node.fullpath.to_str(),
                        Some("/apple.txt" | "/banana.txt" | "/cherry.txt")
                    ));
                    assert_eq!(
                        bytes,
                        format!("{}\n", node.fullpath.file_stem().unwrap().to_str().unwrap())
                            .as_bytes()
                    );
                }
                if image.header.version == 2 && image.header.count == 5 {
                    assert!(matches!(
                        node.fullpath.file_name().and_then(|name| name.to_str()),
                        Some("apple1.txt" | "apple2.txt" | "apple3.txt" | "apple4.txt")
                    ));
                    assert_eq!(
                        bytes,
                        format!("{}\n", node.fullpath.file_stem().unwrap().to_str().unwrap())
                            .as_bytes()
                    );
                }
                count += 1;
            }
        }
        assert!(count > 0);
    }
}

#[test]
fn legacy_bit_fields_and_zlib_bounds() {
    assert_eq!(wire::Order::Little.bits(&[0x21, 0x43], 4, 8).unwrap(), 0x32);
    assert_eq!(wire::Order::Big.bits(&[0x21, 0x43], 4, 8).unwrap(), 0x14);
    assert!(wire::Order::Big.bits(&[0], 4, 8).is_err());
    assert!(FilesystemReader::from_reader(BufReader::new(Cursor::new(vec![0; 64]))).is_err());
}

fn fixture(name: &str) -> FilesystemReader<'static> {
    let bytes = std::fs::read(format!(
        "{}/test_data/legacy/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    FilesystemReader::from_reader(BufReader::new(Cursor::new(bytes))).unwrap()
}

#[test]
fn legacy_independent_endian_metadata_fragments_and_known_lzma() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test_data/legacy");
    for path in std::fs::read_dir(&root).unwrap() {
        let path = path.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("squashfs") {
            continue;
        }
        let name = path.file_name().unwrap().to_str().unwrap();
        let image = fixture(name);
        let expected = std::fs::read(root.join(if name.contains("32k") {
            "payload-32k.bin"
        } else if name.contains("64k") {
            "payload-64k.bin"
        } else {
            "payload.bin"
        }))
        .unwrap();
        assert_eq!(image.files().count(), 12, "{name}");
        let mut count = 0;
        for node in image.files() {
            if let BackhandInnerNode::File(file) = &node.inner {
                let bytes = image.file_data(file).unwrap_or_else(|error| {
                    panic!("{name} {}: {error:?}", node.fullpath.display())
                });
                if file.file_len() != 0 {
                    assert_eq!(bytes, expected, "{name}");
                    count += 1;
                }
                let mut streamed = Vec::new();
                image.file_data_to_writer(file, &mut streamed).unwrap();
                assert_eq!(streamed, bytes);
                for offset in [
                    0,
                    1,
                    4090,
                    8180,
                    32765,
                    65530,
                    expected.len(),
                    expected.len() + 1,
                ] {
                    let start = offset.min(bytes.len());
                    assert_eq!(
                        image.file_data_range(file, offset as u64, 8193).unwrap(),
                        bytes[start..(start + 8193).min(bytes.len())],
                        "{name}"
                    );
                }
                assert!(node.header.mtime_known);
                if file.file_len() == 0 {
                    assert_eq!(node.header.mtime, -123);
                }
                assert_eq!(node.header.uid, 1035);
                assert_eq!(node.header.gid, 1035);
            }
            if !matches!(
                node.inner,
                BackhandInnerNode::File(_) | BackhandInnerNode::Dir
            ) {
                assert!(!node.header.mtime_known);
            }
        }
        assert_eq!(count, 3);
    }
}

#[test]
fn legacy_markerless_lzma_never_synthesizes_truncated_or_extra_bytes() {
    let bytes = include_bytes!("../test_data/legacy/known-length.block");
    let expected = include_bytes!("../test_data/legacy/payload.bin");
    let expected = &expected[8192..];
    assert_eq!(
        wire::decode_data(bytes, 4096, expected.len()).unwrap(),
        expected
    );
    for cut in 0..bytes.len() {
        assert!(
            wire::decode_data(&bytes[..cut], 4096, expected.len()).is_err(),
            "cut {cut}"
        );
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(wire::decode_data(&trailing, 4096, expected.len()).is_err());
    assert!(wire::decode_data(bytes, 4096, expected.len() - 1).is_err());
    assert!(wire::decode_data(bytes, 4096, expected.len() + 1).is_err());
}

fn stored_fixture() -> Vec<u8> {
    std::fs::read(format!(
        "{}/test_data/legacy/v2.0-le-stored.squashfs",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}
fn open_bytes(bytes: Vec<u8>) -> Result<FilesystemReader<'static>> {
    FilesystemReader::from_reader(BufReader::new(Cursor::new(bytes)))
}
fn put_bits(bytes: &mut [u8], offset: usize, count: usize, value: u64) {
    for index in 0..count {
        let bit = offset + index;
        let mask = 1 << (bit % 8);
        bytes[bit / 8] =
            (bytes[bit / 8] & !mask) | if value & (1 << index) != 0 { mask } else { 0 };
    }
}

#[test]
fn legacy_header_marker_and_table_bounds_are_enforced() {
    for (offset, value) in [
        (8, u32::MAX),
        (20, u32::MAX),
        (24, 0),
        (12, 1),
        (55, u32::MAX),
        (59, 0),
    ] {
        let mut bytes = stored_fixture();
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        assert!(open_bytes(bytes).is_err(), "field {offset}");
    }
    let mut bytes = stored_fixture();
    bytes[37] = 0;
    assert!(open_bytes(bytes).is_err());
    let bytes = stored_fixture();
    for cut in [0, 50, 62, bytes.len() - 1] {
        assert!(open_bytes(bytes[..cut].to_vec()).is_err());
    }
    let mut bytes = stored_fixture();
    let inode_start = u32::from_le_bytes(bytes[20..24].try_into().unwrap()) as usize;
    bytes[inode_start + 2] = 0;
    assert!(open_bytes(bytes).is_err());
}

#[test]
fn legacy_cycles_duplicate_names_and_invalid_paths_fail() {
    let mut bytes = stored_fixture();
    let inode_start = u32::from_le_bytes(bytes[20..24].try_into().unwrap()) as usize;
    let directory_start = u32::from_le_bytes(bytes[24..28].try_into().unwrap()) as usize;
    let root = u64::from_le_bytes(bytes[43..51].try_into().unwrap());
    let root_position = inode_start + (root >> 16) as usize + 3 + (root & 0xffff) as usize;
    let directory_offset = ((u32::from_le_bytes(
        bytes[root_position + 4..root_position + 8]
            .try_into()
            .unwrap(),
    ) >> 19)
        & 0x1fff) as usize;
    let entry = directory_start + 3 + directory_offset;
    put_bits(&mut bytes, entry * 8 + 8, 24, root >> 16);
    put_bits(&mut bytes, (entry + 4) * 8, 13, root & 0xffff);
    put_bits(&mut bytes, (entry + 4) * 8 + 13, 3, 1);
    assert!(open_bytes(bytes).is_err());
    let bytes = stored_fixture();
    let name = bytes[directory_start..]
        .windows(4)
        .position(|bytes| bytes == b"char")
        .unwrap()
        + directory_start;
    let mut duplicate = bytes.clone();
    duplicate[name..name + 4].copy_from_slice(b"fifo");
    assert!(open_bytes(duplicate).is_err());
    let mut slash = bytes;
    slash[name] = b'/';
    assert!(open_bytes(slash).is_err());
}

#[test]
fn legacy_bad_zlib_is_not_reinterpreted_as_markerless_lzma() {
    let image = fixture("v2.0-le-zlib.squashfs");
    let plan = image
        .plans
        .values()
        .find(|plan| plan.descriptor.file_len() == 9001)
        .unwrap();
    let block = &plan.blocks[1];
    let mut reader = image.reader.lock().unwrap();
    let bytes = wire::read_at(
        &mut **reader,
        block.start,
        block.size,
        image.header.inode_start,
    )
    .unwrap();
    for cut in 0..bytes.len() {
        assert!(wire::decode_data(&bytes[..cut], 4096, 4096).is_err());
    }
    let mut bad = bytes;
    let last = bad.len() - 1;
    bad[last] ^= 1;
    assert!(wire::decode_data(&bad, 4096, 4096).is_err());
}

#[test]
fn legacy_truncated_eos_lzma_cannot_change_into_a_known_size_stream() {
    let image = fixture("v2.0-le-lzma-props.squashfs");
    let plan = image
        .plans
        .values()
        .find(|plan| plan.descriptor.file_len() == 9001)
        .unwrap();
    let block = &plan.blocks[1];
    let mut reader = image.reader.lock().unwrap();
    let bytes = wire::read_at(
        &mut **reader,
        block.start,
        block.size,
        image.header.inode_start,
    )
    .unwrap();
    for cut in 0..bytes.len() {
        assert!(
            wire::decode_data(&bytes[..cut], 4096, 4096).is_err(),
            "truncated EOS at {cut}/{}",
            bytes.len()
        );
    }
}

#[test]
fn legacy_source_failures_and_forged_descriptors_do_not_become_holes() {
    use std::io::{Read, Seek, SeekFrom};
    struct Fault {
        bytes: Cursor<Vec<u8>>,
        from: u64,
        to: u64,
    }
    impl Read for Fault {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            if (self.from..self.to).contains(&self.bytes.position()) {
                return Ok(0);
            }
            self.bytes.read(output)
        }
    }
    impl Seek for Fault {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.bytes.seek(position)
        }
    }
    let bytes = stored_fixture();
    let end = u32::from_le_bytes(bytes[20..24].try_into().unwrap()) as u64;
    let reader = BufReader::with_capacity(
        1,
        Fault {
            bytes: Cursor::new(bytes),
            from: 63,
            to: end,
        },
    );
    let image = FilesystemReader::from_reader(reader).unwrap();
    let file = image
        .plans
        .values()
        .find(|plan| plan.descriptor.file_len() == 9001)
        .unwrap()
        .descriptor
        .clone();
    assert!(image.file_data_range(&file, 0, 10).is_err());
    let mut forged = file;
    if let BackhandSquashfsFileReader::Basic { blocks_start, .. } = &mut forged {
        *blocks_start = u64::MAX;
    }
    assert!(image.file_data_range(&forged, 0, 10).is_err());
}
