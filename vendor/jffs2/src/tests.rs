use super::*;
use std::cell::Cell;

struct Bytes(Vec<u8>);
impl ReadAt for Bytes {
    fn size(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
        Ok(self
            .0
            .get(offset as usize..offset as usize + length)
            .context("test outside source")?
            .to_vec())
    }
}
fn fixture(order: &str, codec: &str) -> Bytes {
    Bytes(
        std::fs::read(format!(
            "{}/test_data/synthetic-{order}-{codec}.jffs2",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
}
fn offsets(bytes: &[u8]) -> Vec<usize> {
    let mut offsets = Vec::new();
    let mut offset = 0;
    while offset + 12 <= bytes.len() {
        let Ok((_, _, length)) = validate_header(&bytes[offset..]) else {
            break;
        };
        offsets.push(offset);
        offset += (length as usize + 3) & !3;
    }
    offsets
}
fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn repair_header(bytes: &mut [u8]) {
    let crc = checksum(&bytes[..8]);
    put_u32(bytes, 8, crc);
}
fn repair_inode(bytes: &mut [u8]) {
    let crc = checksum(&bytes[..60]);
    put_u32(bytes, 64, crc);
}
fn repair_dirent(bytes: &mut [u8]) {
    let crc = checksum(&bytes[..32]);
    put_u32(bytes, 32, crc);
}

#[test]
fn both_endian_and_all_codecs_share_logical_bytes() {
    let expected = include_bytes!("../test_data/payload.bin");
    for order in ["le", "be"] {
        for codec in ["stored", "rtime", "zlib", "lzo"] {
            let bytes = fixture(order, codec);
            let image = Jffs2Image::open(&bytes).unwrap();
            assert_eq!(
                image.byte_order(),
                if order == "le" {
                    ByteOrder::Little
                } else {
                    ByteOrder::Big
                }
            );
            let names: Vec<_> = image
                .children(1)
                .unwrap()
                .iter()
                .map(|entry| entry.name.as_str())
                .collect();
            assert_eq!(
                names,
                [
                    "alias.bin",
                    "cut.txt",
                    "empty.txt",
                    "link",
                    "payload.bin",
                    "子目录"
                ]
            );
            for path in ["/payload.bin", "/alias.bin", "/子目录/tail.txt"] {
                let inode = image.resolve(path).unwrap();
                assert_eq!(inode, 2);
                assert_eq!(image.read_range(&bytes, inode, 0, 10000).unwrap(), expected);
                for (start, count) in [
                    (0, 8),
                    (6, 17),
                    (4090, 30),
                    (8180, 40),
                    (8997, 100),
                    (9001, 7),
                ] {
                    assert_eq!(
                        image.read_range(&bytes, inode, start, count).unwrap(),
                        expected[start as usize..(start as usize + count).min(expected.len())]
                    );
                }
                let mut streamed = Vec::new();
                for start in (0..expected.len()).step_by(317) {
                    streamed.extend(image.read_range(&bytes, inode, start as u64, 317).unwrap());
                }
                assert_eq!(streamed, expected);
            }
            assert_eq!(
                image
                    .read_range(&bytes, image.resolve("/cut.txt").unwrap(), 0, 100)
                    .unwrap(),
                b"01234"
            );
            assert_eq!(image.read_range(&bytes, 4, 0, 100).unwrap(), b"payload.bin");
            assert!(image.read_range(&bytes, 5, 0, 100).unwrap().is_empty());
            let inode = image.inode(2).unwrap();
            assert_eq!(
                (
                    inode.uid,
                    inode.gid,
                    inode.accessed,
                    inode.modified,
                    inode.changed
                ),
                (1000, 1001, 1700000001, 1700000002, 1700000003)
            );
            assert!(image.resolve("/deleted.txt").is_err());
        }
    }
}

#[test]
fn independent_crc_vectors_and_implicit_root() {
    assert_eq!(checksum(b"123456789"), 0x2dfd2d88);
    let mut bytes = fixture("le", "stored");
    let offsets = offsets(&bytes.0);
    let root = offsets[1];
    // Clear ACCURATE without changing the original CRC, as NOR obsoletion does.
    bytes.0[root + 3] &= !0x20;
    let image = Jffs2Image::open(&bytes).unwrap();
    assert!(image.inode(1).unwrap().implicit_root);
    assert_eq!(image.children(1).unwrap().len(), 6);
}

#[test]
fn header_inode_dirent_name_and_lazy_data_crc_failures() {
    let fixture = fixture("le", "zlib");
    let positions = offsets(&fixture.0);
    for position in [0, positions[1] + 20, positions[2] + 16, positions[2] + 40] {
        let mut bytes = Bytes(fixture.0.clone());
        bytes.0[position] ^= 1;
        assert!(Jffs2Image::open(&bytes).is_err(), "offset {position}");
    }
    let mut bytes = Bytes(fixture.0.clone());
    bytes.0[positions[3] + 69] ^= 1;
    let image = Jffs2Image::open(&bytes).unwrap();
    assert!(image.read_range(&bytes, 2, 8192, 20).is_err());
}

#[test]
fn crc_valid_lengths_modes_flags_and_references_are_checked() {
    for (position, value) in [
        (4, 67),
        (48, u32::MAX),
        (52, 1024 * 1024 + 1),
        (44, u32::MAX),
        (20, 0),
    ] {
        let mut bytes = fixture("le", "stored");
        let root = offsets(&bytes.0)[1];
        put_u32(&mut bytes.0[root..], position, value);
        repair_header(&mut bytes.0[root..]);
        repair_inode(&mut bytes.0[root..]);
        assert!(Jffs2Image::open(&bytes).is_err());
    }
    let mut bytes = fixture("le", "stored");
    let root = offsets(&bytes.0)[1];
    bytes.0[root + 58] = 0x80;
    repair_inode(&mut bytes.0[root..]);
    assert!(Jffs2Image::open(&bytes).is_err());
    for inode in [1, 999] {
        let mut bytes = fixture("le", "stored");
        let dirent = offsets(&bytes.0)[2];
        put_u32(&mut bytes.0[dirent..], 20, inode);
        repair_dirent(&mut bytes.0[dirent..]);
        assert!(Jffs2Image::open(&bytes).is_err());
    }
}

#[test]
fn conflicting_gc_versions_and_incompatible_nodes_fail() {
    let mut bytes = fixture("le", "stored");
    let duplicate = offsets(&bytes.0)[20];
    put_u32(&mut bytes.0[duplicate..], 28, 5000);
    repair_inode(&mut bytes.0[duplicate..]);
    assert!(Jffs2Image::open(&bytes).is_err());
    let mut bytes = fixture("le", "stored");
    bytes.0[2..4].copy_from_slice(&0xe00au16.to_le_bytes());
    repair_header(&mut bytes.0);
    assert!(Jffs2Image::open(&bytes).is_err());
}

#[test]
fn path_limits_and_source_short_reads() {
    let bytes = fixture("be", "lzo");
    let image = Jffs2Image::open(&bytes).unwrap();
    for path in [
        "payload.bin",
        "/../payload.bin",
        "/子目录//tail.txt",
        "/link/child",
        "/alias.bin/",
    ] {
        assert!(image.resolve(path).is_err());
    }
    assert!(image.read_range(&bytes, 1, 0, 10).is_err());
    struct Short<'a>(&'a Bytes);
    impl ReadAt for Short<'_> {
        fn size(&self) -> u64 {
            self.0.size()
        }
        fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
            let mut bytes = self.0.read_at(offset, length)?;
            bytes.pop();
            Ok(bytes)
        }
    }
    assert!(Jffs2Image::open(&Short(&bytes)).is_err());
    assert!(image.read_range(&Short(&bytes), 2, 0, 10).is_err());
    for cut in [1, 12, 13, 70, 150] {
        assert!(Jffs2Image::open(&Bytes(bytes.0[..cut].to_vec())).is_err());
    }
    assert!(
        image
            .read_range(&bytes, 2, u64::MAX, 10)
            .unwrap()
            .is_empty()
    );
    assert!(
        image
            .read_range(&bytes, 2, 0, 256 * 1024 * 1024 + 1)
            .is_err()
    );
}

#[test]
fn cancellation_does_not_require_a_runtime() {
    let bytes = fixture("le", "zlib");
    assert!(Jffs2Image::open_with_cancel(&bytes, &|| true).is_err());
    let calls = Cell::new(0);
    assert!(
        Jffs2Image::open_with_cancel(&bytes, &|| {
            calls.set(calls.get() + 1);
            calls.get() > 25
        })
        .is_err()
    );
}

#[test]
fn auxiliary_nodes_and_reachable_directory_cycles_are_checked() {
    let original = fixture("le", "stored");
    let positions = offsets(&original.0);
    assert_eq!(positions.len(), 25);
    for (node, field) in [(22, 20), (22, 32), (23, 12), (24, 24), (24, 32)] {
        let mut bytes = Bytes(original.0.clone());
        bytes.0[positions[node] + field] ^= 1;
        assert!(Jffs2Image::open(&bytes).is_err());
    }
    let mut bytes = Bytes(original.0.clone());
    let position = positions[10];
    let (_, _, length) = validate_header(&bytes.0[position..]).unwrap();
    let mut cycle = bytes.0[position..position + length as usize].to_vec();
    put_u32(&mut cycle, 12, 3);
    put_u32(&mut cycle, 16, 100);
    repair_dirent(&mut cycle);
    bytes.0.extend_from_slice(&cycle);
    assert!(Jffs2Image::open(&bytes).is_err());
    let image = Jffs2Image::open(&original).unwrap();
    assert_eq!(image.inode(1).unwrap().modified, 1700000004);
    assert_eq!(image.inode(1).unwrap().changed, 1700000004);
    assert_eq!(image.inode(1).unwrap().accessed, 1700000001);
}

#[test]
fn all_deleted_names_still_form_an_empty_implicit_root() {
    let mut bytes = fixture("le", "stored");
    for position in offsets(&bytes.0) {
        let (_, kind, _) = validate_header(&bytes.0[position..]).unwrap();
        if kind == 0xe001 {
            put_u32(&mut bytes.0[position..], 20, 0);
            repair_dirent(&mut bytes.0[position..]);
        }
    }
    let root = offsets(&bytes.0)[1];
    bytes.0[root + 3] &= !0x20;
    let image = Jffs2Image::open(&bytes).unwrap();
    assert!(image.inode(1).unwrap().implicit_root);
    assert!(image.children(1).unwrap().is_empty());
}

#[test]
fn codecs_enforce_eof_size_checksum_and_explicit_unsupported_errors() {
    assert_eq!(codec::decode(b"A\x05", 2, 6).unwrap(), b"AAAAAA");
    for (bytes, kind, size) in [
        (b"A\x05".as_slice(), 2, 5),
        (b"A", 2, 6),
        (b"A\0X\0", 2, 1),
        (b"X", 1, 1),
        (b"X", 0, 2),
        (b"X", 3, 1),
        (b"X", 5, 1),
        (b"X", 8, 1),
    ] {
        assert!(codec::decode(bytes, kind, size).is_err());
    }
    let source = fixture("le", "zlib");
    let image = Jffs2Image::open(&source).unwrap();
    let node = image
        .nodes
        .iter()
        .find(|node| node.compression == 6 && node.size == 809)
        .unwrap();
    let data = source
        .read_at(node.source, node.compressed as usize)
        .unwrap();
    assert!(codec::decode(&data, 6, 808).is_err());
    assert!(codec::decode(&data[..data.len() - 1], 6, 809).is_err());
    let mut trailing = data.clone();
    trailing.push(0);
    assert!(codec::decode(&trailing, 6, 809).is_err());
    let mut checksum = data;
    let end = checksum.len() - 1;
    checksum[end] ^= 1;
    assert!(codec::decode(&checksum, 6, 809).is_err());
    assert!(codec::decode(b"\x12X\x11\0\0X", 7, 1).is_err());
}

#[test]
fn rtime_history_positions_wrap_at_sixteen_bits() {
    let mut encoded = b"A\0".repeat(65535);
    encoded.extend_from_slice(b"X\0X\x01");
    let mut expected = vec![b'A'; 65535];
    expected.extend_from_slice(b"XXA");
    assert_eq!(
        codec::decode(&encoded, 2, expected.len()).unwrap(),
        expected
    );
}

#[test]
#[ignore = "set JFFS2_EXTERNAL_SAMPLES to semicolon-separated Unblob fruits new LE/BE zlib images"]
fn external_fruits() {
    let paths = std::env::var("JFFS2_EXTERNAL_SAMPLES").expect("JFFS2_EXTERNAL_SAMPLES required");
    for path in paths.split(';') {
        let source = Bytes(std::fs::read(path).unwrap());
        let image = Jffs2Image::open(&source).unwrap();
        assert!(image.inode(1).unwrap().implicit_root);
        assert_eq!(image.children(1).unwrap().len(), 3);
        for (path, expected) in [
            ("/apple.txt", b"AAAAAAAAAAAAAAAAAAAAAAAAA\n".as_slice()),
            ("/banana.txt", b"banana\n"),
            ("/cherry.txt", b"cherry\n"),
        ] {
            let inode = image.resolve(path).unwrap();
            assert_eq!(image.read_range(&source, inode, 0, 100).unwrap(), expected);
            for offset in 0..expected.len() {
                assert_eq!(
                    image.read_range(&source, inode, offset as u64, 5).unwrap(),
                    expected[offset..(offset + 5).min(expected.len())]
                );
            }
        }
    }
}
