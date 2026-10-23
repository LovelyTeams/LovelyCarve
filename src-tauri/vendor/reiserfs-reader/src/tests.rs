use super::*;
use lamfold::{BlockCache, FileKind, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};
use std::io::Read;
fn cx(cache: &mut BlockCache) -> SubstrateCtx<'_> {
    SubstrateCtx {
        cache,
        verifier: &NoVerifier,
    }
}
fn invented(compressed: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    flate2::read::ZlibDecoder::new(compressed)
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}
fn sample() -> Vec<u8> {
    invented(include_bytes!("../tests/fixtures/v36-4096-relocated.zlib"))
}
fn external() -> Vec<u8> {
    zstd::stream::decode_all(include_bytes!("../tests/fixtures/reiserfsprogs.img.zst").as_slice())
        .unwrap()
}
fn read(
    reader: &mut ReiserFs<SliceSource<'_>>,
    node: u64,
    context: &mut SubstrateCtx<'_>,
) -> Vec<u8> {
    let mut bytes = vec![0; reader.metadata(node, context).unwrap().size as usize];
    assert_eq!(
        reader.read_at(node, 0, &mut bytes, context).unwrap(),
        bytes.len()
    );
    bytes
}
#[test]
fn external_reiserfsprogs_tail_indirect_directories_and_links() {
    let bytes = external();
    let mut cache = BlockCache::new(1024 * 1024);
    let mut context = cx(&mut cache);
    let mut reader = ReiserFs::open(SliceSource::new(&bytes), &mut context).unwrap();
    assert_eq!(reader.header().format, Format::V36);
    assert_eq!(reader.header().label, b"reiser36_test");
    let root = reader.root();
    let entries = reader.read_dir(root, &mut context).unwrap();
    assert!(entries.iter().any(|entry| entry.name == ".reiserfs_priv"));
    for (name, content) in [
        ("hello.txt", b"Hello, ReiserFS!".as_slice()),
        ("tiny.txt", b"tiny bytes".as_slice()),
    ] {
        let node = reader.lookup(root, name, &mut context).unwrap().unwrap();
        let actual = read(&mut reader, node, &mut context);
        assert_eq!(actual, content);
    }
    let link = reader
        .lookup(root, "link.txt", &mut context)
        .unwrap()
        .unwrap();
    assert_eq!(
        reader.read_link(link, &mut context).unwrap().unwrap(),
        b"hello.txt"
    );
    let big = reader
        .lookup(root, "large.bin", &mut context)
        .unwrap()
        .unwrap();
    assert_eq!(
        read(&mut reader, big, &mut context),
        (0..24576).map(|i| (i * 37 + 11) as u8).collect::<Vec<_>>()
    );
    let sub = reader
        .lookup(root, "subdir", &mut context)
        .unwrap()
        .unwrap();
    let nested = reader
        .lookup(sub, "nested.txt", &mut context)
        .unwrap()
        .unwrap();
    assert_eq!(read(&mut reader, nested, &mut context), b"nested file");
}
#[test]
fn versions_block_sizes_bitmap_layouts_and_internal_mixed_keys() {
    for compressed in [
        include_bytes!("../tests/fixtures/v35-1024-old.zlib").as_slice(),
        include_bytes!("../tests/fixtures/v36-1024.zlib").as_slice(),
        include_bytes!("../tests/fixtures/v35-2048.zlib").as_slice(),
        include_bytes!("../tests/fixtures/v36-4096-relocated.zlib").as_slice(),
        include_bytes!("../tests/fixtures/v36-8192.zlib").as_slice(),
    ] {
        let bytes = invented(compressed);
        let mut cache = BlockCache::new(1024 * 1024);
        let mut context = cx(&mut cache);
        let mut reader = ReiserFs::open(SliceSource::new(&bytes), &mut context).unwrap();
        let bs = reader.header().block_size as usize;
        let root = reader.root();
        let entries = reader.read_dir(root, &mut context).unwrap();
        assert_eq!(entries.len(), 8);
        assert!(!entries.iter().any(|entry| entry.name == "hidden"));
        let big = reader
            .lookup(root, "big.bin", &mut context)
            .unwrap()
            .unwrap();
        assert_eq!(
            reader.lookup(root, "alias.bin", &mut context).unwrap(),
            Some(big)
        );
        let expected = [
            (0..bs).map(|i| (i * 17 + 5) as u8).collect::<Vec<_>>(),
            vec![0; bs],
            (0..bs).map(|i| (i * 23 + 7) as u8).collect(),
            b"packed tail".to_vec(),
        ]
        .concat();
        assert_eq!(read(&mut reader, big, &mut context), expected);
        for offset in [
            0,
            bs - 7,
            2 * bs - 3,
            3 * bs - 2,
            expected.len(),
            usize::MAX,
        ] {
            let mut buf = [0x55; 71];
            let count = reader
                .read_at(big, offset as u64, &mut buf, &mut context)
                .unwrap();
            let start = offset.min(expected.len());
            let end = start.saturating_add(71).min(expected.len());
            assert_eq!(&buf[..count], &expected[start..end]);
            assert!(buf[count..].iter().all(|byte| *byte == 0x55));
        }
        let name = reader
            .lookup(root, "中文\\evidence", &mut context)
            .unwrap()
            .unwrap();
        assert_eq!(read(&mut reader, name, &mut context), b"names");
        let times = reader.timestamps(big, &mut context).unwrap();
        assert_eq!(times.modified, Some((4_000_000_000, 0)));
        assert_eq!(times.accessed, Some((0, 0)));
        assert!(times.created.is_none());
        let sub = reader.lookup(root, "sub", &mut context).unwrap().unwrap();
        let child = reader.read_dir(sub, &mut context).unwrap()[0].node;
        assert_eq!(child >> 32, 88);
        assert_eq!(read(&mut reader, child, &mut context), b"nested");
        let empty = reader.lookup(root, "Empty", &mut context).unwrap().unwrap();
        assert!(read(&mut reader, empty, &mut context).is_empty());
        assert_ne!(
            reader.lookup(root, "empty", &mut context).unwrap(),
            Some(empty)
        );
        let link = reader.lookup(root, "link", &mut context).unwrap().unwrap();
        assert_eq!(
            reader.metadata(link, &mut context).unwrap().kind,
            FileKind::Symlink
        );
        assert_eq!(
            reader.read_link(link, &mut context).unwrap().unwrap(),
            b"big.bin"
        );
    }
}
fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
#[test]
fn corrupt_headers_dirty_truncation_and_reserved_free_blocks_fail() {
    let original = sample();
    for (offset, value) in [
        (65536 + 50, 2),
        (65536 + 44, 512),
        (65536 + 68, 7),
        (65536 + 70, 0),
        (65536 + 72, 7),
    ] {
        let mut bytes = original.clone();
        put16(&mut bytes, offset, value);
        assert!(Header::read(&mut SliceSource::new(&bytes)).is_err());
    }
    assert!(Header::read(&mut SliceSource::new(&original[..original.len() - 1])).is_err());
    let header = Header::read(&mut SliceSource::new(&original)).unwrap();
    for block in [
        0,
        16,
        17,
        header.journal_start,
        header.journal_start + header.journal_blocks,
        header.blocks,
    ] {
        assert!(header.data_block(block).is_err());
    }
    let mut bytes = original.clone();
    let block = header.root;
    let bitmap = header.bitmap(block / (header.block_size * 8));
    let bit = block % (header.block_size * 8);
    bytes[bitmap as usize * 4096 + bit as usize / 8] &= !(1 << (bit % 8));
    assert!(ReiserFs::open(
        SliceSource::new(&bytes),
        &mut cx(&mut BlockCache::new(1024))
    )
    .is_err());
}
#[test]
fn tree_child_cycles_levels_sizes_keys_and_leaf_bounds_fail() {
    let original = sample();
    let header = Header::read(&mut SliceSource::new(&original)).unwrap();
    let root = header.root as usize * 4096;
    let count = le16(&original, root + 2).unwrap() as usize;
    let child = le32(&original, root + 24 + count * 16).unwrap() as usize * 4096;
    let child_count = le16(&original, child + 2).unwrap() as usize;
    let leaf = le32(&original, child + 24 + child_count * 16).unwrap() as usize * 4096;
    for (offset, value) in [
        (root, 1),
        (root + 24 + count * 16 + 4, 7),
        (child, 1),
        (leaf + 24 + 20, 0),
        (leaf + 4, 0),
    ] {
        let mut bytes = original.clone();
        put16(&mut bytes, offset, value);
        assert!(
            ReiserFs::open(
                SliceSource::new(&bytes),
                &mut cx(&mut BlockCache::new(1024))
            )
            .is_err(),
            "{offset}"
        );
    }
    let mut bytes = original.clone();
    put32(&mut bytes, root + 24 + count * 16, header.root);
    assert!(ReiserFs::open(
        SliceSource::new(&bytes),
        &mut cx(&mut BlockCache::new(1024))
    )
    .is_err());
}
#[test]
fn incomplete_content_offset_overlap_free_pointer_and_short_source_fail() {
    let original = sample();
    let header = Header::read(&mut SliceSource::new(&original)).unwrap();
    let mut tree = tree::Tree::new();
    let big = (2u64 << 32) | 4;
    let mut items = tree
        .object(&mut SliceSource::new(&original), &header, big)
        .unwrap();
    let stat = objects::Stat::parse(&items).unwrap();
    items.pop();
    assert!(objects::content(&stat, &items, 4096).is_err());
    items[1].key.offset = 2;
    assert!(objects::content(&stat, &items, 4096).is_err());
    let mut bytes = original.clone();
    let block = header.blocks - 7;
    let bitmap = header.bitmap(block / (header.block_size * 8));
    let bit = block % (header.block_size * 8);
    bytes[bitmap as usize * 4096 + bit as usize / 8] &= !(1 << (bit % 8));
    let mut cache = BlockCache::new(1024);
    let mut context = cx(&mut cache);
    let mut reader = ReiserFs::open(SliceSource::new(&bytes), &mut context).unwrap();
    assert!(reader.read_at(big, 0, &mut [0; 3], &mut context).is_err());
}

#[test]
fn lossless_names_directory_states_and_unknown_item_versions() {
    let bytes = sample();
    let header = Header::read(&mut SliceSource::new(&bytes)).unwrap();
    let root = (1u64 << 32) | 2;
    let mut tree = tree::Tree::new();
    let original = tree
        .object(&mut SliceSource::new(&bytes), &header, root)
        .unwrap();
    let stat = objects::Stat::parse(&original).unwrap();
    for invalid in [0xff, b'/'] {
        let mut items = original.clone();
        let location = le16(&items[1].body, 2 * 16 + 12).unwrap() as usize;
        items[1].body[location] = invalid;
        assert!(objects::directory(root, &stat, &items).is_err());
    }
    for state in [1, 8] {
        let mut items = original.clone();
        put16(&mut items[1].body, 2 * 16 + 14, state);
        assert!(objects::directory(root, &stat, &items).is_err());
    }
    let mut bytes = bytes.clone();
    let root_page = header.root as usize * 4096;
    let child = le32(&bytes, root_page + 40).unwrap() as usize * 4096;
    let count = le16(&bytes, child + 2).unwrap() as usize;
    let leaf = le32(&bytes, child + 24 + count * 16).unwrap() as usize * 4096;
    put16(&mut bytes, leaf + 24 + 22, 0x8001);
    assert!(ReiserFs::open(
        SliceSource::new(&bytes),
        &mut cx(&mut BlockCache::new(1024))
    )
    .is_err());
}

#[test]
fn coalesced_runs_validate_every_block_and_preserve_source_errors() {
    let mut bytes = external();
    let header = Header::read(&mut SliceSource::new(&bytes)).unwrap();
    let block = 8213; // second block of mkfs/guestfish large.bin
    let bitmap = header.bitmap(block / (header.block_size * 8));
    let bit = block % (header.block_size * 8);
    bytes[bitmap as usize * 4096 + bit as usize / 8] &= !(1 << (bit % 8));
    let mut cache = BlockCache::new(1024);
    let mut context = cx(&mut cache);
    let mut reader = ReiserFs::open(SliceSource::new(&bytes), &mut context).unwrap();
    assert!(reader
        .read_at((2u64 << 32) | 9, 0, &mut [0; 3], &mut context)
        .is_err());
    struct FaultSource {
        bytes: Vec<u8>,
    }
    impl BlockSource for FaultSource {
        fn len(&self) -> u64 {
            self.bytes.len() as u64
        }
        fn read_at(&mut self, offset: u64, output: &mut [u8]) -> lamfold::Result<()> {
            if (8212 * 4096..8218 * 4096).contains(&offset) {
                return Err(FoldError::Io("injected source read failure"));
            }
            let offset = offset as usize;
            output.copy_from_slice(&self.bytes[offset..offset + output.len()]);
            Ok(())
        }
    }
    let mut reader = ReiserFs::open(FaultSource { bytes: external() }, &mut context).unwrap();
    assert!(matches!(
        reader.read_at((2u64 << 32) | 9, 0, &mut [0; 3], &mut context),
        Err(FoldError::Io(_))
    ));
}

#[test]
fn sparse_file_larger_than_four_gib_has_a_compact_exact_map() {
    use lamfold::FileTimes;
    let node = (2u64 << 32) | 4;
    let blocks = 1_048_577u64;
    let stat = objects::Stat {
        mode: 0x81a4,
        size: blocks * 4096,
        times: FileTimes::default(),
    };
    let mut items = vec![tree::Item {
        key: tree::Key {
            node,
            offset: 0,
            kind: 0,
        },
        version: 1,
        count: 0,
        body: vec![0; 44],
    }];
    for first in (0..blocks).step_by(1000) {
        items.push(tree::Item {
            key: tree::Key {
                node,
                offset: first * 4096 + 1,
                kind: 1,
            },
            version: 1,
            count: 0,
            body: vec![0; (blocks - first).min(1000) as usize * 4],
        });
    }
    let map = objects::content(&stat, &items, 4096).unwrap();
    assert_eq!(map.len(), 1);
    assert_eq!(map[0].length, stat.size);
    assert!(matches!(map[0].content, objects::Content::Block(0)));
    // Duplicate real blocks are refused even when contiguous pointers were
    // coalesced across item boundaries.
    put32(&mut items[1].body, 0, 100);
    put32(&mut items[1].body, 4, 101);
    put32(&mut items[1].body, 8, 100);
    assert!(objects::content(&stat, &items, 4096).is_err());
}
