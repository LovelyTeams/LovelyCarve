use super::*;
use lamfold::{BlockCache, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};
use std::io::Read;

fn fixture(bytes: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();
    flate2::read::ZlibDecoder::new(bytes)
        .read_to_end(&mut result)
        .unwrap();
    result
}

#[test]
fn fragmented_trees_forks_macroman_local_time_and_ranges() {
    for (compressed, block_size) in [
        (
            include_bytes!("../tests/fixtures/fragmented-512.zlib").as_slice(),
            512,
        ),
        (
            include_bytes!("../tests/fixtures/fragmented-1024.zlib").as_slice(),
            1024,
        ),
        (
            include_bytes!("../tests/fixtures/fragmented-4096.zlib").as_slice(),
            4096,
        ),
    ] {
        let bytes = fixture(compressed);
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        let mut volume = Hfs::open(SliceSource::new(&bytes), &mut context).unwrap();
        assert_eq!(volume.header().block_size, block_size);
        let file = volume.lookup(2, "CAFÉ.BIN", &mut context).unwrap().unwrap();
        assert_eq!(volume.lookup(2, "Cafe.bin", &mut context).unwrap(), None);
        for (kind, tail, factor, shift) in
            [(ForkKind::Data, 37, 17, 3), (ForkKind::Resource, 19, 23, 5)]
        {
            let expected: Vec<u8> = (0..3 * block_size as usize + tail)
                .map(|index| (index * factor + shift) as u8)
                .collect();
            let mut content = vec![0; expected.len() + 17];
            let count = volume.read_fork_at(file, kind, 0, &mut content).unwrap();
            assert_eq!(&content[..count], expected);
            for offset in [
                0,
                block_size as usize - 7,
                3 * block_size as usize - 7,
                expected.len() - 3,
                expected.len(),
                expected.len() + 9,
            ] {
                let count = volume
                    .read_fork_at(file, kind, offset as u64, &mut content[..61])
                    .unwrap();
                assert_eq!(
                    &content[..count],
                    &expected[offset.min(expected.len())..(offset + 61).min(expected.len())]
                );
            }
        }
        let entries = volume.read_dir(2, &mut context).unwrap();
        for name in ["a:b", "a＼b", "．", "．．", "empty", "only-rsrc", "sub"] {
            let node = entries
                .iter()
                .find(|entry| entry.name == name)
                .unwrap()
                .node;
            assert_eq!(volume.lookup(2, name, &mut context).unwrap(), Some(node));
        }
        let resource_only = volume
            .lookup(2, "only-rsrc", &mut context)
            .unwrap()
            .unwrap();
        assert_eq!(volume.fork_size(resource_only, ForkKind::Data).unwrap(), 0);
        let mut content = [0; 10];
        assert_eq!(
            volume
                .read_fork_at(resource_only, ForkKind::Resource, 0, &mut content)
                .unwrap(),
            5
        );
        assert_eq!(&content[..5], b"RSRC!");
        assert_eq!(volume.raw_name(file).unwrap(), b"Caf\x8e.bin");
        assert_eq!(volume.backup_local(file).unwrap(), 3_800_000_052);
        assert_eq!(
            volume
                .local_timestamps(file, &mut context)
                .unwrap()
                .modified,
            Some((3_800_000_048 - 2_082_844_800, 0))
        );
    }
}
#[test]
fn hfsutils_external_volume() {
    let bytes =
        zstd::stream::decode_all(include_bytes!("../tests/fixtures/hfsutils.img.zst").as_slice())
            .unwrap();
    let mut cache = BlockCache::new(0);
    let mut context = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut volume = Hfs::open(SliceSource::new(&bytes), &mut context).unwrap();
    assert_eq!(volume.header().volume_name, "test_hfs");
    let children = volume.read_dir(2, &mut context).unwrap();
    let hello = children
        .iter()
        .find(|entry| entry.name == "hello.txt")
        .unwrap();
    let mut content = [0; 64];
    let count = volume
        .read_at(hello.node, 0, &mut content, &mut context)
        .unwrap();
    assert_eq!(&content[..count], b"Hello, hfs!");
    let sub = children
        .iter()
        .find(|entry| entry.name == "subdir")
        .unwrap();
    assert_eq!(
        volume.read_dir(sub.node, &mut context).unwrap()[0].name,
        "nested.txt"
    );
    assert!(volume
        .local_timestamps(hello.node, &mut context)
        .unwrap()
        .modified
        .is_some());
    assert!(volume
        .timestamps(hello.node, &mut context)
        .unwrap()
        .modified
        .is_none());
}

#[test]
fn corrupt_mdb_bitmap_btree_and_incomplete_forks_fail_explicitly() {
    let original = fixture(include_bytes!("../tests/fixtures/fragmented-1024.zlib"));
    let mut cache = BlockCache::new(0);
    let mut context = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    // Every mutation targets a distinct on-disk invariant, not parser internals.
    for (offset, replacement) in [
        (1024 + 20, 0xff),                        // impossible allocation geometry
        (1024 + 124, 0x48),                       // HFS+ wrapper (complete below)
        (1536, 0),                                // metadata references free block
        (2048 + 512 + 8, 0),                      // leaf pretending to be index
        (2048 + 512 + 9, 2),                      // wrong leaf height
        (2048 + 512 + 3, 1),                      // cyclic leaf link
        (2048 + 512 + 510, 0xff),                 // record offset inside offset table
        (2048 + 248, 0),                          // header map says used nodes free
        (2048 + 8 * 1024 + 512 + 14 + 38 + 3, 1), // index points to itself
        (2048 + 10 * 1024 + 512 + 7, 0),          // broken leaf backward link
    ] {
        let mut bytes = original.clone();
        bytes[offset] = replacement;
        if offset == 1024 + 124 {
            bytes[offset + 1] = 0x2b;
        }
        assert!(
            Hfs::open(SliceSource::new(&bytes), &mut context).is_err(),
            "offset {offset}"
        );
    }
    for missing in [1, 511, 1024, original.len() - 1535] {
        assert!(Hfs::open(
            SliceSource::new(&original[..original.len() - missing]),
            &mut context
        )
        .is_err());
    }
    // Fork mapping is validated even when asking for only its first byte or EOF.
    for (offset, replacement) in [
        (1536 + 16 / 8, 0),                         // allocated data missing from bitmap
        (2048 + 512 + 14 + 2 * 20 + 7, 4),          // missing data-overflow start=3
        (2048 + 12 * 1024 + 512 + 30 + 74 + 9, 16), // overlapping inline data extent
        (2048 + 12 * 1024 + 512 + 30 + 33, 1),      // physical EOF no longer matches extents
    ] {
        let mut bytes = original.clone();
        bytes[offset] = replacement;
        let mut volume = Hfs::open(SliceSource::new(&bytes), &mut context).unwrap();
        assert!(
            volume
                .read_fork_at(16, ForkKind::Data, 0, &mut [0; 1])
                .is_err(),
            "offset {offset}"
        );
    }
}

#[test]
fn name_collation_preserves_accents_case_and_nbsp() {
    assert!(names::compare(b"Caf\x8e", b"CAF\x83").is_eq());
    assert!(!names::compare(b"Caf\x8e", b"Cafe").is_eq());
    assert!(names::compare(b"a b", b"A\xcab").is_eq());
    assert!(names::compare(b"B\x8ezier", b"Bind").is_lt());
    for byte in 1..=255 {
        if byte == b':' {
            continue;
        }
        assert_eq!(names::encode(&names::display(&[byte])).unwrap(), [byte]);
    }
}

#[test]
fn source_fault_during_fork_read_is_not_synthesized_as_zeroes() {
    struct FaultySource<'a>(&'a [u8]);
    impl lamfold::BlockSource for FaultySource<'_> {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read_at(&mut self, offset: u64, bytes: &mut [u8]) -> lamfold::Result<()> {
            if (2048 + 16 * 1024..2048 + 17 * 1024).contains(&offset) {
                return Err(lamfold::FoldError::Io("invented source read fault"));
            }
            SliceSource::new(self.0).read_at(offset, bytes)
        }
    }
    let bytes = fixture(include_bytes!("../tests/fixtures/fragmented-1024.zlib"));
    let mut cache = BlockCache::new(0);
    let mut context = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut volume = Hfs::open(FaultySource(&bytes), &mut context).unwrap();
    assert!(volume
        .read_fork_at(16, ForkKind::Data, 0, &mut [0; 100])
        .is_err());
}
