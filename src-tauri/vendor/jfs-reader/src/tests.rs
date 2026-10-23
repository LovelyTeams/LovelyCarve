use super::*;
use lamfold::{BlockCache, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};
fn reference() -> Vec<u8> {
    zstd::stream::decode_all(include_bytes!("../tests/fixtures/jfsutils.img.zst").as_slice())
        .unwrap()
}
#[test]
fn external_superblocks_and_aggregate_metadata_maps() {
    let bytes = reference();
    let mut source = SliceSource::new(&bytes);
    let header = Header::read(&mut source).unwrap();
    assert_eq!(
        (
            header.version,
            header.block_size,
            header.blocks,
            header.selected_copy
        ),
        (1, 4096, 3788, HeaderCopy::Primary)
    );
    assert_eq!(header.label, b"jfs_test");
    let bmap = xtree::Map::parse(
        &mut source,
        &header,
        &bytes[45056 + 2 * 512 + 224..45056 + 3 * 512],
    )
    .unwrap();
    let mut control = [0; 4096];
    bmap.exact(&mut source, &header, 24576, 0, &mut control)
        .unwrap();
    assert_eq!(le64(&control, 0).unwrap(), header.blocks);
    let imap = xtree::Map::parse(
        &mut source,
        &header,
        &bytes[45056 + 16 * 512 + 224..45056 + 17 * 512],
    )
    .unwrap();
    let mut iag = [0; 4096];
    imap.exact(&mut source, &header, 8192, 4096, &mut iag)
        .unwrap();
    assert_eq!(le32(&iag, 8).unwrap(), 0);
    assert_eq!(le32(&iag, 2560).unwrap(), u32::MAX);
    let extent = metadata::Physical::parse(&iag, 3072, &header).unwrap();
    assert_eq!((extent.start, extent.count), (28, 4));
}
#[test]
fn secondary_header_does_not_override_dirty_or_unknown_primary() {
    let original = reference();
    let mut bytes = original.clone();
    bytes[32768] = 0;
    assert_eq!(
        Header::read(&mut SliceSource::new(&bytes))
            .unwrap()
            .selected_copy,
        HeaderCopy::Secondary
    );
    for (offset, value) in [(32768 + 40, 2), (32768 + 4, 3), (32768 + 44, 1)] {
        let mut bytes = original.clone();
        bytes[offset] = value;
        assert!(matches!(
            Header::read(&mut SliceSource::new(&bytes)),
            Err(FoldError::Unsupported(_))
        ));
    }
    assert!(Header::read(&mut SliceSource::new(&original[..32768])).is_err());
}
fn set16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn set32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn xad(bytes: &mut [u8], offset: usize, logical: u32, start: u32, length: u32, flag: u8) {
    bytes[offset] = flag;
    set32(bytes, offset + 4, logical);
    set32(bytes, offset + 8, length);
    set32(bytes, offset + 12, start);
}
#[test]
fn internal_xtree_keys_seek_sparse_and_unwritten_content() {
    let mut bytes = reference();
    let header = Header::read(&mut SliceSource::new(&bytes)).unwrap();
    let mut root = [0; 288];
    root[16] = 0x85;
    set16(&mut root, 18, 3);
    set16(&mut root, 20, 18);
    xad(&mut root, 32, 0, 100, 1, 0);
    let page = &mut bytes[100 * 4096..101 * 4096];
    page.fill(0);
    page[16] = 0x82;
    set16(page, 18, 4);
    set16(page, 20, 256);
    set32(page, 24, 1);
    set32(page, 28, 100);
    xad(page, 32, 0, 110, 2, 0);
    xad(page, 48, 10, 120, 1, 0);
    bytes[110 * 4096..112 * 4096].fill(0x5a);
    bytes[120 * 4096..121 * 4096].fill(0xc3);
    let mut source = SliceSource::new(&bytes);
    let map = xtree::Map::parse(&mut source, &header, &root).unwrap();
    let mut data = [0; 41];
    assert_eq!(
        map.read(&mut source, &header, 11 * 4096, 10 * 4096 + 7, &mut data)
            .unwrap(),
        41
    );
    assert_eq!(data, [0xc3; 41]);
    map.read(&mut source, &header, 11 * 4096, 2 * 4096 - 7, &mut data)
        .unwrap();
    assert_eq!(&data[..7], &[0x5a; 7]);
    assert_eq!(&data[7..], &[0; 34]);
    assert!(map
        .exact(&mut source, &header, 11 * 4096, 3 * 4096, &mut data)
        .is_err());
    bytes[100 * 4096 + 48] = 8;
    let mut source = SliceSource::new(&bytes);
    let map = xtree::Map::parse(&mut source, &header, &root).unwrap();
    map.read(&mut source, &header, 11 * 4096, 10 * 4096, &mut data)
        .unwrap();
    assert_eq!(data, [0; 41]);
    // Child self pointer and traversal topology must agree with the parent XAD.
    bytes[100 * 4096 + 28] = 99;
    assert!(xtree::Map::parse(&mut SliceSource::new(&bytes), &header, &root).is_err());
}

#[test]
fn external_jfsutils_volume_full_tree_content_and_persistent_directory_index() {
    let bytes = reference();
    let mut cache = BlockCache::new(0);
    let mut context = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut reader = Jfs::open(SliceSource::new(&bytes), &mut context).unwrap();
    let root = reader.read_dir(2, &mut context).unwrap();
    assert_eq!(
        root.iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        [
            "bigdir",
            "hello.txt",
            "large.bin",
            "link.txt",
            "subdir",
            "tiny.txt"
        ]
    );
    for (name, expected) in [
        ("hello.txt", b"Hello, JFS!".as_slice()),
        ("tiny.txt", b"tiny bytes"),
        ("link.txt", b"hello.txt"),
    ] {
        let node = reader.lookup(2, name, &mut context).unwrap().unwrap();
        let mut content = [0; 100];
        let count = reader.read_at(node, 0, &mut content, &mut context).unwrap();
        assert_eq!(&content[..count], expected);
    }
    let large = reader
        .lookup(2, "large.bin", &mut context)
        .unwrap()
        .unwrap();
    let expected: Vec<u8> = (0..24 * 1024)
        .map(|index| (index * 37 + 11) as u8)
        .collect();
    let mut actual = vec![0; expected.len()];
    assert_eq!(
        reader.read_at(large, 0, &mut actual, &mut context).unwrap(),
        expected.len()
    );
    assert_eq!(actual, expected);
    for offset in [0, 4091, 8191, 24572, 24576, 24587] {
        let mut actual = [0; 39];
        let count = reader
            .read_at(large, offset as u64, &mut actual, &mut context)
            .unwrap();
        assert_eq!(
            &actual[..count],
            &expected[offset.min(expected.len())..(offset + 39).min(expected.len())]
        );
    }
    let big = reader.lookup(2, "bigdir", &mut context).unwrap().unwrap();
    let children = reader.read_dir(big, &mut context).unwrap();
    assert_eq!(children.len(), 200);
    for (index, entry) in children.iter().enumerate() {
        assert_eq!(entry.name, format!("file_{:03}", index + 1));
        let mut actual = [0; 100];
        let count = reader
            .read_at(entry.node, 0, &mut actual, &mut context)
            .unwrap();
        assert_eq!(
            &actual[..count],
            format!("line {:03}", index + 1).as_bytes()
        );
    }
}

fn inflated(bytes: &[u8]) -> Vec<u8> {
    use std::io::Read;
    let mut result = Vec::new();
    flate2::read::ZlibDecoder::new(bytes)
        .read_to_end(&mut result)
        .unwrap();
    result
}
#[test]
fn independent_block_sizes_multiple_iags_legacy_names_links_and_nanoseconds() {
    for (fixture, block_size) in [
        (
            include_bytes!("../tests/fixtures/multi-iag-512-indexed.zlib").as_slice(),
            512,
        ),
        (
            include_bytes!("../tests/fixtures/multi-iag-512-legacy.zlib").as_slice(),
            512,
        ),
        (
            include_bytes!("../tests/fixtures/multi-iag-1024-indexed.zlib").as_slice(),
            1024,
        ),
        (
            include_bytes!("../tests/fixtures/multi-iag-1024-legacy.zlib").as_slice(),
            1024,
        ),
        (
            include_bytes!("../tests/fixtures/multi-iag-2048-indexed.zlib").as_slice(),
            2048,
        ),
        (
            include_bytes!("../tests/fixtures/multi-iag-2048-legacy.zlib").as_slice(),
            2048,
        ),
        (
            include_bytes!("../tests/fixtures/multi-iag-4096-indexed.zlib").as_slice(),
            4096,
        ),
        (
            include_bytes!("../tests/fixtures/multi-iag-4096-legacy.zlib").as_slice(),
            4096,
        ),
    ] {
        let bytes = inflated(fixture);
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        let mut reader = Jfs::open(SliceSource::new(&bytes), &mut context).unwrap();
        let children = reader.read_dir(2, &mut context).unwrap();
        assert_eq!(children.len(), 8);
        assert_eq!(reader.lookup(2, "big.bin", &mut context).unwrap(), Some(4));
        assert_eq!(
            reader.lookup(2, "alias.bin", &mut context).unwrap(),
            Some(4)
        );
        let mut expected = vec![0; 12 * block_size + 37];
        for (start, length, factor) in [
            (0, 2 * block_size, 17),
            (10 * block_size, block_size, 23),
            (12 * block_size, 37, 29),
        ] {
            for index in 0..length {
                expected[start + index] = (index * factor + 5) as u8;
            }
        }
        let mut actual = vec![0; expected.len() + 101];
        let count = reader.read_at(4, 0, &mut actual, &mut context).unwrap();
        assert_eq!(&actual[..count], expected);
        for offset in [
            0,
            2 * block_size - 7,
            6 * block_size - 7,
            10 * block_size - 7,
            12 * block_size - 7,
            expected.len() - 3,
            expected.len() + 9,
        ] {
            let count = reader
                .read_at(4, offset as u64, &mut actual[..91], &mut context)
                .unwrap();
            assert_eq!(
                &actual[..count],
                &expected[offset.min(expected.len())..(offset + 91).min(expected.len())]
            );
        }
        let times = reader.timestamps(4, &mut context).unwrap();
        assert_eq!(
            (times.accessed, times.modified),
            (Some((0, 0)), Some((4_000_000_000, 123456789)))
        );
        for (name, expected) in [
            ("link-short", b"big.bin".to_vec()),
            ("link-long", [b"./".as_slice(), &[b's'; 198]].concat()),
            ("link-external", [b"../".as_slice(), &[b'x'; 297]].concat()),
        ] {
            let node = reader.lookup(2, name, &mut context).unwrap().unwrap();
            assert_eq!(
                reader.read_link(node, &mut context).unwrap(),
                Some(expected)
            );
        }
        let sub = reader.lookup(2, "sub", &mut context).unwrap().unwrap();
        assert_eq!(sub, 4098);
        let nested = reader.read_dir(sub, &mut context).unwrap();
        assert_eq!(nested[0].node, 4099);
        let mut content = [0; 100];
        let count = reader.read_at(4099, 0, &mut content, &mut context).unwrap();
        assert_eq!(&content[..count], b"hello JFS");
        let long = format!("long_中文_{}\\tail", "x".repeat(180));
        assert_eq!(reader.lookup(2, &long, &mut context).unwrap(), Some(8));
    }
}

#[test]
fn corruption_cannot_turn_missing_allocations_or_names_into_successful_content() {
    let original = inflated(include_bytes!(
        "../tests/fixtures/multi-iag-4096-indexed.zlib"
    ));
    let mut cache = BlockCache::new(0);
    let mut context = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    // Root identity, slot partition, directory index, continuation and IAG
    // coverage are independently addressed on disk by the fixture's schema.
    let long_slot = usize::from(original[196608 + 124 * 32 + 6]);
    for (offset, value) in [
        (131072 + 2 * 512 + 244, 17),
        (196608 + 18, 1),
        (131072 + 2 * 512 + 128 + 2, 1),
        (196608 + long_slot * 32 + 4, long_slot as u8),
        (98304 + 4, 1),
    ] {
        let mut bytes = original.clone();
        bytes[offset] = value;
        let result = Jfs::open(SliceSource::new(&bytes), &mut context)
            .and_then(|mut reader| reader.read_dir(2, &mut context));
        assert!(result.is_err(), "metadata mutation at {offset}");
    }
    let file = 131072 + 4 * 512;
    for (offset, value) in [
        (file, 0),                      // wrong fileset stamp
        (file + 79, 0xff),              // invalid timestamp fraction
        (file + 256 + 8, 1),            // missing data allocation, nblocks unchanged
        (file + 256, 4),                // compressed XAD
        (file + 256 + 2 * 16 + 12, 64), // overlapping physical data runs
    ] {
        let mut bytes = original.clone();
        bytes[offset] = value;
        let result = Jfs::open(SliceSource::new(&bytes), &mut context)
            .and_then(|mut reader| reader.read_at(4, 0, &mut [0; 1], &mut context));
        assert!(result.is_err(), "file mutation at {offset}");
    }
    let mut bytes = original.clone();
    // Free the first data block, preserving the independent map counters and
    // working/persistent bitmap agreement. The explicit ownership check fails.
    for base in [2048, 3072] {
        let offset = 65536 + 4 * 4096 + base + (64 / 32) * 4;
        let word = le32(&bytes, offset).unwrap() & !0x80000000;
        set32(&mut bytes, offset, word);
    }
    for offset in [65536 + 8, 65536 + 4 * 4096 + 4] {
        let free = le32(&bytes, offset).unwrap();
        set32(&mut bytes, offset, free + 1);
    }
    let mut reader = Jfs::open(SliceSource::new(&bytes), &mut context).unwrap();
    assert!(reader.read_at(4, 0, &mut [0; 1], &mut context).is_err());
}

#[test]
fn source_failure_in_allocated_content_is_not_a_sparse_hole() {
    struct Faulty<'a>(&'a [u8]);
    impl lamfold::BlockSource for Faulty<'_> {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read_at(&mut self, offset: u64, bytes: &mut [u8]) -> lamfold::Result<()> {
            if (262144..270336).contains(&offset) {
                return Err(FoldError::Io("invented JFS payload read fault"));
            }
            lamfold::BlockSource::read_at(&mut SliceSource::new(self.0), offset, bytes)
        }
    }
    let bytes = inflated(include_bytes!(
        "../tests/fixtures/multi-iag-4096-indexed.zlib"
    ));
    let mut cache = BlockCache::new(0);
    let mut context = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut reader = Jfs::open(Faulty(&bytes), &mut context).unwrap();
    assert!(reader.read_at(4, 0, &mut [0; 100], &mut context).is_err());
}

#[test]
fn dmap_address_accounts_for_control_pages_above_32_gib() {
    use std::collections::BTreeMap;
    struct Sparse {
        length: u64,
        pages: BTreeMap<u64, [u8; 4096]>,
        reads: Vec<u64>,
    }
    impl lamfold::BlockSource for Sparse {
        fn len(&self) -> u64 {
            self.length
        }
        fn read_at(&mut self, offset: u64, bytes: &mut [u8]) -> lamfold::Result<()> {
            self.reads.push(offset);
            let page = self
                .pages
                .get(&offset)
                .ok_or(FoldError::Io("unexpected sparse metadata address"))?;
            require(bytes.len() == 4096, "sparse test page size")?;
            bytes.copy_from_slice(page);
            Ok(())
        }
    }
    let mut header = Header::read(&mut SliceSource::new(&reference())).unwrap();
    header.blocks = (1 << 23) + 8192;
    let mut control = [0; 4096];
    control[..8].copy_from_slice(&header.blocks.to_le_bytes());
    let mut first = [0; 4096];
    set32(&mut first, 0, 8192);
    first[2048..].fill(0xff);
    let mut distant = first;
    distant[8..16].copy_from_slice(&(1u64 << 23).to_le_bytes());
    let mut source = Sparse {
        length: header.mapped_bytes(),
        pages: BTreeMap::from([
            (512 * 4096, control),
            ((512 + 4) * 4096, first),
            ((512 + 1029) * 4096, distant),
        ]),
        reads: Vec::new(),
    };
    let map = xtree::Map {
        runs: vec![xtree::Run {
            logical: 0,
            physical: metadata::Physical {
                start: 512,
                count: 1030,
            },
            unwritten: false,
        }],
        pages: Vec::new(),
    };
    let mut allocation =
        allocation::Allocation::open(&mut source, &header, map, 1030 * 4096).unwrap();
    allocation
        .check(
            &mut source,
            &header,
            metadata::Physical {
                start: (1 << 23) + 7,
                count: 1,
            },
        )
        .unwrap();
    assert!(source.reads.contains(&((512 + 1029) * 4096)));
}
