use super::*;
use lamfold::{BlockCache, FileKind, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};
use std::io::Read;

fn inflate(raw: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    flate2::read::ZlibDecoder::new(raw)
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}
fn context(cache: &mut BlockCache) -> SubstrateCtx<'_> {
    SubstrateCtx {
        cache,
        verifier: &NoVerifier,
    }
}
fn open(raw: &[u8]) -> Result<BeFs<SliceSource<'_>>> {
    BeFs::open(SliceSource::new(raw), &mut context(&mut BlockCache::new(0)))
}

#[test]
fn independent_variants_cover_tree_runs_attributes_names_dates_and_ranges() {
    for (packed, block_size, big, blocks) in [
        (
            include_bytes!("../tests/fixtures/le-1024-2560.zlib").as_slice(),
            1024,
            false,
            2560,
        ),
        (
            include_bytes!("../tests/fixtures/be-1024-2560.zlib").as_slice(),
            1024,
            true,
            2560,
        ),
        (
            include_bytes!("../tests/fixtures/le-2048-2560.zlib").as_slice(),
            2048,
            false,
            2560,
        ),
        (
            include_bytes!("../tests/fixtures/be-2048-2560.zlib").as_slice(),
            2048,
            true,
            2560,
        ),
        (
            include_bytes!("../tests/fixtures/le-4096-2560.zlib").as_slice(),
            4096,
            false,
            2560,
        ),
        (
            include_bytes!("../tests/fixtures/be-4096-2560.zlib").as_slice(),
            4096,
            true,
            2560,
        ),
        (
            include_bytes!("../tests/fixtures/le-8192-2560.zlib").as_slice(),
            8192,
            false,
            2560,
        ),
        (
            include_bytes!("../tests/fixtures/be-8192-2560.zlib").as_slice(),
            8192,
            true,
            2560,
        ),
        (
            include_bytes!("../tests/fixtures/le-1024-10000.zlib").as_slice(),
            1024,
            false,
            10000,
        ),
        (
            include_bytes!("../tests/fixtures/be-1024-100000.zlib").as_slice(),
            1024,
            true,
            100000,
        ),
    ] {
        let raw = inflate(packed);
        assert!(BeFs::probe(&mut SliceSource::new(&raw)).unwrap());
        let mut reader = open(&raw).unwrap_or_else(|error| {
            panic!("block={block_size}, big={big}, blocks={blocks}: {error}")
        });
        assert_eq!(
            (
                reader.header().block_size,
                reader.header().blocks,
                reader.header().superblock_offset
            ),
            (block_size, blocks, if big { 0 } else { 512 })
        );
        let mut cache = BlockCache::new(0);
        let mut cx = context(&mut cache);
        let root = reader.root();
        let directory = reader.read_dir(root, &mut cx).unwrap();
        assert_eq!(directory.len(), 9);
        let node = reader.lookup(root, "big.bin", &mut cx).unwrap().unwrap();
        assert_eq!(
            reader.lookup(root, "alias.bin", &mut cx).unwrap(),
            Some(node)
        );
        assert!(reader.lookup(root, "BIG.BIN", &mut cx).unwrap().is_none());
        let expected: Vec<_> = (0..26 * block_size as usize - 37)
            .map(|index| (index * 17 + 5) as u8)
            .collect();
        let mut actual = vec![0; expected.len()];
        assert_eq!(
            reader.read_at(node, 0, &mut actual, &mut cx).unwrap(),
            actual.len()
        );
        assert_eq!(actual, expected);
        for offset in [
            0,
            block_size as usize - 7,
            12 * block_size as usize - 9,
            18 * block_size as usize - 11,
            expected.len() - 3,
            expected.len(),
            usize::MAX,
        ] {
            let mut actual = [0; 73];
            let count = reader
                .read_at(node, offset as u64, &mut actual, &mut cx)
                .unwrap();
            let start = offset.min(expected.len());
            assert_eq!(
                &actual[..count],
                &expected[start..start.saturating_add(73).min(expected.len())]
            );
        }
        let attributes = reader.attributes(node).unwrap();
        assert_eq!(
            attributes
                .iter()
                .map(|attribute| (attribute.name.as_str(), attribute.size, attribute.resident))
                .collect::<Vec<_>>(),
            vec![
                ("BEOS:TYPE", 11, true),
                ("big-attribute", 12003, false),
                ("custom", 9, true)
            ]
        );
        assert!(attributes.iter().all(
            |attribute| attribute.type_code == 0x4d494d53 || attribute.type_code == 0x52415754
        ));
        let mut value = [0; 20];
        let length = reader
            .read_attribute_at(node, "custom", 0, &mut value)
            .unwrap();
        assert_eq!(&value[..length], b"\0\xffinline\x80");
        let length = reader
            .read_attribute_at(node, "big-attribute", 4090, &mut value)
            .unwrap();
        assert_eq!(
            value[..length],
            (4090..4110)
                .map(|index| (index * 23 + 7) as u8)
                .collect::<Vec<_>>()
        );
        let times = reader.timestamps(node, &mut cx).unwrap();
        assert_eq!(
            (times.created, times.modified, times.changed, times.accessed),
            (Some((-315619200, 0)), Some((1700000000, 0)), None, None)
        );
        assert_eq!(
            reader.recorded_times(node).unwrap().created & 0xffff,
            0x2345
        );
        let names = reader
            .lookup(root, "中文\\evidence", &mut cx)
            .unwrap()
            .unwrap();
        let length = reader.read_at(names, 0, &mut value, &mut cx).unwrap();
        assert_eq!(&value[..length], b"names");
        let long = reader
            .lookup(root, &format!("{}.bin", "é".repeat(100)), &mut cx)
            .unwrap()
            .unwrap();
        let length = reader.read_at(long, 0, &mut value, &mut cx).unwrap();
        assert_eq!(&value[..length], b"long filename");
        let sub = reader.lookup(root, "sub", &mut cx).unwrap().unwrap();
        assert_eq!(reader.read_dir(sub, &mut cx).unwrap()[0].name, "nested.txt");
        let short = reader.lookup(root, "link", &mut cx).unwrap().unwrap();
        assert_eq!(
            reader.read_link(short, &mut cx).unwrap().unwrap(),
            b"sub/nested.txt"
        );
        let long = reader.lookup(root, "longlink", &mut cx).unwrap().unwrap();
        assert_eq!(
            reader.read_link(long, &mut cx).unwrap().unwrap(),
            [b"relative/".as_slice(), &[b'x'; 200]].concat()
        );
        let pipe = reader.lookup(root, "pipe", &mut cx).unwrap().unwrap();
        assert_eq!(
            reader.metadata(pipe, &mut cx).unwrap().kind,
            FileKind::Other
        );
        assert!(reader.read_at(pipe, 0, &mut value, &mut cx).is_err());
        let empty = reader.lookup(root, "empty", &mut cx).unwrap().unwrap();
        assert_eq!(reader.read_at(empty, 0, &mut value, &mut cx).unwrap(), 0);
    }
}

#[test]
fn timestamp_interpretation_requires_provenance_and_preserves_all_words() {
    let raw = RecordedTimes {
        created: (-315619200i64 << 16) | 0x2345,
        modified: (1700000000i64 << 16) | 0xf123,
        status_or_reserved: 0x123456789,
    };
    for encoding in [TimestampEncoding::Unknown, TimestampEncoding::BeOs] {
        let times = raw.interpret(encoding).unwrap();
        assert_eq!(
            (times.created, times.modified, times.changed),
            (Some((-315619200, 0)), Some((1700000000, 0)), None)
        );
    }
    let times = raw.interpret(TimestampEncoding::Haiku).unwrap();
    assert_eq!(
        (times.created, times.modified, times.changed),
        (
            Some((-315619200, 147849216)),
            Some((1700000000, 0)),
            Some((74565, 434110464))
        )
    );
    let invalid = RecordedTimes {
        created: 0xeff0,
        modified: 0,
        status_or_reserved: 0,
    };
    assert!(invalid.interpret(TimestampEncoding::Haiku).is_err());
    assert!(invalid.interpret(TimestampEncoding::Unknown).is_ok());
}

fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn probe_distinguishes_a_nonmatching_format_from_a_source_read_failure() {
    struct Broken;
    impl BlockSource for Broken {
        fn len(&self) -> u64 {
            4096
        }
        fn read_at(&mut self, _offset: u64, _bytes: &mut [u8]) -> Result<()> {
            Err(FoldError::Io("injected header read failure"))
        }
    }
    assert!(matches!(BeFs::probe(&mut Broken), Err(FoldError::Io(_))));
    assert!(!BeFs::probe(&mut SliceSource::new(&[0; 4096])).unwrap());
}
fn put64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn dirty_truncated_and_invalid_geometry_refuse_without_replaying() {
    let original = inflate(include_bytes!("../tests/fixtures/le-1024-2560.zlib"));
    for (offset, value) in [(512 + 84, 0x44495254), (512 + 96, 3)] {
        let mut raw = original.clone();
        put32(&mut raw, offset, value);
        let geometry = Header::inspect(&mut SliceSource::new(&raw)).unwrap();
        assert!(!geometry.clean);
        assert!(matches!(open(&raw), Err(FoldError::Unsupported(_))));
    }
    for (offset, value) in [
        (512 + 44, 31),
        (512 + 72, 0),
        (512 + 76, 32),
        (512 + 80, 2),
        (512 + 96, 10000),
    ] {
        let mut raw = original.clone();
        put32(&mut raw, offset, value);
        assert!(Header::inspect(&mut SliceSource::new(&raw)).is_err());
    }
    assert!(open(&original[..original.len() - 1]).is_err());
    let mut raw = original;
    raw[1024] &= !1;
    assert!(open(&raw).is_err());
}

#[test]
fn missing_free_duplicate_outside_and_recursive_maps_never_return_complete_data() {
    let original = inflate(include_bytes!("../tests/fixtures/le-1024-2560.zlib"));
    for mutation in 0..8 {
        let mut raw = original.clone();
        let stream = 67 * 1024 + 72;
        match mutation {
            0 => {
                put64(&mut raw, stream + 136, 26 * 1024 + 1);
            }
            1 => {
                let at = 1024 + (300 / 32) * 4;
                let word = u32::from_le_bytes(raw[at..at + 4].try_into().unwrap());
                put32(&mut raw, at, word & !(1 << (300 % 32)));
            }
            2 => {
                raw[stream + 8..stream + 16].copy_from_slice(&original[stream..stream + 8]);
            }
            3 => put32(&mut raw, stream, 2),
            4 => {
                raw[stream + 4..stream + 6].copy_from_slice(&64u16.to_le_bytes());
            }
            5 => put64(&mut raw, stream + 112, 18 * 1024 + 1),
            6 => {
                raw[114 * 1024 + 6..114 * 1024 + 8].copy_from_slice(&3u16.to_le_bytes());
            }
            7 => {
                raw[110 * 1024 + 4..110 * 1024 + 6].copy_from_slice(&110u16.to_le_bytes());
            }
            _ => unreachable!(),
        }
        let mut reader = open(&raw).unwrap();
        let mut cache = BlockCache::new(0);
        let mut cx = context(&mut cache);
        assert!(
            reader.read_at(67, 0, &mut [0; 17], &mut cx).is_err(),
            "mutation={mutation}"
        );
    }
}

#[test]
fn tree_cross_references_names_free_chain_and_inode_identity_are_checked() {
    let original = inflate(include_bytes!("../tests/fixtures/le-1024-2560.zlib"));
    for mutation in 0..7 {
        let mut raw = original.clone();
        let tree = 150 * 1024;
        match mutation {
            0 => put64(&mut raw, tree + 1024 + 16, 1024),
            1 => put64(&mut raw, tree + 2048 + 8, 2048),
            2 => put64(&mut raw, tree + 4096, 4096),
            3 => put32(&mut raw, tree + 32, u32::MAX),
            4 => {
                let name = "中文\\evidence".as_bytes();
                let at = raw
                    .windows(name.len())
                    .position(|window| window == name)
                    .unwrap();
                raw[at + 2] = 0xff;
            }
            5 => put32(&mut raw, 65 * 1024 + 4, 1),
            6 => put32(&mut raw, 65 * 1024 + 24, 0x11),
            _ => unreachable!(),
        }
        assert!(open(&raw).is_err(), "mutation={mutation}");
    }
}

#[test]
fn cached_ranges_seek_directly_and_propagate_source_faults() {
    use std::{cell::RefCell, rc::Rc};
    struct Audit<'a> {
        bytes: &'a [u8],
        reads: Rc<RefCell<Vec<(u64, usize)>>>,
        fault: Rc<RefCell<Option<u64>>>,
    }
    impl BlockSource for Audit<'_> {
        fn len(&self) -> u64 {
            self.bytes.len() as u64
        }
        fn read_at(&mut self, offset: u64, bytes: &mut [u8]) -> Result<()> {
            if self
                .fault
                .borrow()
                .is_some_and(|fault| offset <= fault && offset + bytes.len() as u64 > fault)
            {
                return Err(FoldError::Io("injected BFS evidence fault"));
            }
            self.reads.borrow_mut().push((offset, bytes.len()));
            let start = offset as usize;
            bytes.copy_from_slice(&self.bytes[start..start + bytes.len()]);
            Ok(())
        }
    }
    let raw = inflate(include_bytes!("../tests/fixtures/le-1024-2560.zlib"));
    let reads = Rc::new(RefCell::new(Vec::new()));
    let fault = Rc::new(RefCell::new(None));
    let mut cache = BlockCache::new(0);
    let mut cx = context(&mut cache);
    let mut reader = BeFs::open(
        Audit {
            bytes: &raw,
            reads: reads.clone(),
            fault: fault.clone(),
        },
        &mut cx,
    )
    .unwrap();
    reader.read_at(67, 0, &mut [0; 1], &mut cx).unwrap();
    reads.borrow_mut().clear();
    let mut range = [0; 97];
    reader
        .read_at(67, 18 * 1024 + 7, &mut range, &mut cx)
        .unwrap();
    assert_eq!(reads.borrow().as_slice(), &[(500 * 1024 + 7, 97)]);
    assert_eq!(
        range.as_slice(),
        (18 * 1024 + 7..18 * 1024 + 104)
            .map(|index| (index * 17 + 5) as u8)
            .collect::<Vec<_>>()
    );
    *fault.borrow_mut() = Some(500 * 1024 + 7);
    assert!(matches!(
        reader.read_at(67, 18 * 1024, &mut range, &mut cx),
        Err(FoldError::Io(_))
    ));
}
