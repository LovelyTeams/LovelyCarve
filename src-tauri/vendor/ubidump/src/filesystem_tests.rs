use super::*;
use std::{collections::BTreeSet, path::Path};
struct Bytes(Vec<u8>);
impl ReadAt for Bytes {
    fn size(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
        Ok(self
            .0
            .get(offset as usize..offset as usize + length)
            .context("short fixture")?
            .to_vec())
    }
}
#[test]
fn ubifs_independent_codecs_sparse_ranges_hardlinks_and_metadata() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test_data");
    let expected = std::fs::read(root.join("payload.bin")).unwrap();
    for name in ["stored", "lzo", "deflate", "zstd", "zstd-compressed"] {
        let source = Bytes(std::fs::read(root.join(format!("synthetic-{name}.ubifs"))).unwrap());
        let fs = UbifsImage::open(&source).unwrap();
        assert_eq!(fs.commit, 7);
        assert_eq!(fs.children(1).unwrap().len(), 7);
        assert_eq!(fs.resolve("/子目录/tail.txt").unwrap(), 2);
        assert_eq!(fs.resolve("/Ak").unwrap(), fs.resolve("/B`").unwrap());
        assert_eq!(fs.inode(2).unwrap().accessed, (-123, 123456789));
        assert_eq!(fs.inode(2).unwrap().links, 5);
        assert_eq!(fs.read_range(&source, 2, 0, usize::MAX).unwrap(), expected);
        for (offset, length) in [
            (0, 12),
            (4080, 60),
            (8180, 100),
            (8990, 100),
            (u64::MAX, 30),
        ] {
            let start = usize::try_from(offset)
                .unwrap_or(usize::MAX)
                .min(expected.len());
            let end = start.saturating_add(length).min(expected.len());
            assert_eq!(
                fs.read_range(&source, 2, offset, length).unwrap(),
                expected[start..end]
            );
        }
        assert_eq!(fs.read_range(&source, 4, 0, 4096).unwrap(), b"payload.bin");
        assert_eq!(fs.read_range(&source, 5, 0, 4096).unwrap(), b"");
    }
}
#[test]
#[ignore = "set UBIFS_EXTERNAL_SAMPLES to semicolon-separated public UBIFS fixtures"]
fn ubifs_external_committed_tree() {
    for path in std::env::var("UBIFS_EXTERNAL_SAMPLES").unwrap().split(';') {
        let source = Bytes(std::fs::read(path).unwrap());
        let fs = UbifsImage::open(&source).unwrap();
        let mut pending = vec![1];
        let mut contents = BTreeSet::new();
        while let Some(parent) = pending.pop() {
            for entry in fs.children(parent).unwrap() {
                if fs.inode(entry.inode).unwrap().kind == FileType::Directory {
                    pending.push(entry.inode);
                } else {
                    contents.insert(fs.read_range(&source, entry.inode, 0, 4096).unwrap());
                }
            }
        }
        assert_eq!(
            contents,
            BTreeSet::from([
                b"banana1\n".to_vec(),
                b"banana2\n".to_vec(),
                b"banana3\n".to_vec(),
                b"banana4\n".to_vec()
            ])
        );
    }
}

fn fixture(name: &str) -> Bytes {
    Bytes(
        std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("test_data/synthetic-{name}.ubifs")),
        )
        .unwrap(),
    )
}
fn refresh(bytes: &mut [u8], start: usize) {
    // Independent bit-by-bit JAMCRC, not the production crc crate.
    let length = u32::from_le_bytes(bytes[start + 16..start + 20].try_into().unwrap()) as usize;
    let mut checksum = u32::MAX;
    for byte in &bytes[start + 8..start + length] {
        checksum ^= u32::from(*byte);
        for _ in 0..8 {
            checksum = (checksum >> 1) ^ if checksum & 1 != 0 { 0xedb88320 } else { 0 };
        }
    }
    bytes[start + 4..start + 8].copy_from_slice(&checksum.to_le_bytes());
}
#[test]
fn ubifs_geometry_flags_and_master_copies_fail_explicitly() {
    let original = fixture("deflate").0;
    for (offset, value) in [
        (28, 0x10u32),
        (28, 0x20),
        (32, 0),
        (36, u32::MAX),
        (40, 1),
        (44, 1),
        (72, u32::MAX),
        (80, 6),
        (124, 1),
    ] {
        let mut bytes = original.clone();
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        refresh(&mut bytes, 0);
        assert!(UbifsImage::open(&Bytes(bytes)).is_err(), "field {offset}");
    }
    let first = 28672 + 512;
    let second = 2 * 28672 + 1024;
    let mut one_bad = original.clone();
    one_bad[first + 4] ^= 1;
    assert_eq!(UbifsImage::open(&Bytes(one_bad.clone())).unwrap().commit, 7);
    one_bad[second + 4] ^= 1;
    assert!(UbifsImage::open(&Bytes(one_bad)).is_err());
    for flags in [0u32, 1, 3, 4, 6, 8] {
        let mut bytes = original.clone();
        bytes[second + 40..second + 44].copy_from_slice(&flags.to_le_bytes());
        refresh(&mut bytes, second);
        assert!(UbifsImage::open(&Bytes(bytes)).is_err());
    }
    let mut conflicting = original.clone();
    conflicting[second + 48..second + 52].copy_from_slice(&6u32.to_le_bytes());
    refresh(&mut conflicting, second);
    assert!(UbifsImage::open(&Bytes(conflicting)).is_err());
    for end in [0, 24, 4095, original.len() - 1] {
        assert!(UbifsImage::open(&Bytes(original[..end].to_vec())).is_err());
    }
}
#[test]
fn ubifs_crc_valid_index_and_namespace_corruption_is_rejected() {
    let original = fixture("stored");
    let root = 7 * 28672;
    for change in 0..4 {
        let mut bytes = original.0.clone();
        match change {
            0 => bytes[root + 26..root + 28].copy_from_slice(&65u16.to_le_bytes()),
            1 => {
                bytes[root + 28..root + 32].copy_from_slice(&7u32.to_le_bytes());
                bytes[root + 32..root + 36].copy_from_slice(&0u32.to_le_bytes());
            }
            2 => bytes[root + 32..root + 36].copy_from_slice(&28664u32.to_le_bytes()),
            3 => bytes[root + 40..root + 44].copy_from_slice(&99u32.to_le_bytes()),
            _ => unreachable!(),
        }
        refresh(&mut bytes, root);
        assert!(UbifsImage::open(&Bytes(bytes)).is_err());
    }
    // Find a real directory leaf through its valid on-disk header and name.
    let name = b"payload.bin";
    // The first occurrence is inode 4's symlink target; select the directory
    // occurrence by checking node type 2 at its 56-byte name offset.
    let entry_start = original
        .0
        .windows(name.len())
        .enumerate()
        .find_map(|(offset, window)| {
            (window == name && offset >= 56 && original.0[offset - 36] == 2).then(|| offset - 56)
        })
        .unwrap();
    for change in 0..4 {
        let mut bytes = original.0.clone();
        match change {
            0 => bytes[entry_start + 4] ^= 1,
            1 => bytes[entry_start + 40..entry_start + 48].copy_from_slice(&999u64.to_le_bytes()),
            2 => bytes[entry_start + 56] = b'/',
            3 => {
                bytes[entry_start + 40..entry_start + 48].copy_from_slice(&1u64.to_le_bytes());
                bytes[entry_start + 49] = 1;
            }
            _ => unreachable!(),
        }
        if change != 0 {
            refresh(&mut bytes, entry_start);
        }
        assert!(UbifsImage::open(&Bytes(bytes)).is_err());
    }
}
#[test]
fn ubifs_allocated_corruption_never_becomes_holes_and_codec_streams_are_exact() {
    for name in ["stored", "lzo", "deflate", "zstd", "zstd-compressed"] {
        let original = fixture(name);
        let fs = UbifsImage::open(&original).unwrap();
        let location = fs.data[&(2, 2)];
        let start = fs.geometry.address(location).unwrap() as usize;
        let node = &original.0[start..start + location.length as usize];
        let compression = u16::from_le_bytes(node[44..46].try_into().unwrap());
        let length = word(node, 40).unwrap() as usize;
        let compressed = &node[48..];
        for invalid in [
            &compressed[..compressed.len() - 1],
            &[compressed, &[0x99]].concat(),
        ] {
            assert!(
                codec::decode(invalid, compression, length).is_err(),
                "{name}"
            );
        }
        if name == "zstd-compressed" {
            let mut invalid = compressed.to_vec();
            let end = invalid.len() - 1;
            invalid[end] ^= 1;
            assert!(codec::decode(&invalid, compression, length).is_err());
        }
        let mut corrupt = original.0.clone();
        corrupt[start + 48] ^= 1;
        let source = Bytes(corrupt);
        let readable_metadata = UbifsImage::open(&source).unwrap();
        assert!(readable_metadata.read_range(&source, 2, 8180, 100).is_err());
        let mut wrong_size = original.0.clone();
        wrong_size[start + 40..start + 44].copy_from_slice(&4097u32.to_le_bytes());
        refresh(&mut wrong_size, start);
        assert!(fs.read_range(&Bytes(wrong_size), 2, 8192, 100).is_err());
    }
    let source = fixture("stored");
    let fs = UbifsImage::open(&source).unwrap();
    for path in [
        "/",
        "//empty",
        "/../payload.bin",
        "/relative-link/payload.bin",
        "/子目录/..",
    ] {
        assert!(fs.resolve(path).is_err());
    }
    assert!(codec::decode(&[0x28, 0xb5, 0x2f, 0xfd, 0, 0xff, 1, 0, 0], 3, 1).is_err());
}

struct ShortNode<'a> {
    source: &'a Bytes,
    address: u64,
}
impl ReadAt for ShortNode<'_> {
    fn size(&self) -> u64 {
        self.source.size()
    }
    fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
        let mut bytes = self.source.read_at(offset, length)?;
        if offset == self.address && length > 40 {
            bytes.truncate(length - 1);
        }
        Ok(bytes)
    }
}
#[test]
fn ubifs_source_short_read_is_not_a_logical_hole() {
    let source = fixture("stored");
    let fs = UbifsImage::open(&source).unwrap();
    let address = fs.geometry.address(fs.data[&(2, 0)]).unwrap();
    let short = ShortNode {
        source: &source,
        address,
    };
    let fs = UbifsImage::open(&short).unwrap();
    assert!(fs.read_range(&short, 2, 0, 100).is_err());
}
