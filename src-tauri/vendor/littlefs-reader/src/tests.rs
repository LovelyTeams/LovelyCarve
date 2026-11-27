use std::io::Read;

use flate2::read::ZlibDecoder;
use lamfold::{BlockCache, FoldError, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};

use crate::{Content, LittleFs, ROOT, metadata::Block};

const SMALL: &[u8] = include_bytes!("../tests/fixtures/128-2.0.zlib");
const MEDIUM: &[u8] = include_bytes!("../tests/fixtures/512-2.1.zlib");
const LARGE: &[u8] = include_bytes!("../tests/fixtures/4096-2.1.zlib");
const INTERRUPTED: &[u8] = include_bytes!("../tests/fixtures/interrupted-move.zlib");

fn decode(zlib: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    ZlibDecoder::new(zlib).read_to_end(&mut bytes).unwrap();
    bytes
}

#[test]
fn native_versioned_images_read_intact() {
    let mut cache = BlockCache::new(0);
    let mut ctx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let expected: Vec<_> = (0..19473).map(|n| (n * 21 + n / 49) as u8).collect();
    for (fixture, block_size, version) in [
        (SMALL, 128, 0x20000),
        (MEDIUM, 512, 0x20001),
        (LARGE, 4096, 0x20001),
    ] {
        let bytes = decode(fixture);
        let mut fs = LittleFs::open(SliceSource::new(&bytes), &mut ctx).unwrap();
        assert_eq!(fs.geometry.superblock.block_size, block_size);
        assert_eq!(fs.geometry.superblock.version, version);
        assert_eq!(
            fs.read_dir(ROOT, &mut ctx)
                .unwrap()
                .into_iter()
                .map(|e| e.name)
                .collect::<Vec<_>>(),
            ["Logs", "Split", "tiny"]
        );
        let tiny = fs.lookup(ROOT, "tiny", &mut ctx).unwrap().unwrap();
        let mut inline = [0; 8];
        assert_eq!(fs.read_at(tiny, 0, &mut inline, &mut ctx).unwrap(), 8);
        assert_eq!(&inline, b"init-end");
        let log = fs.lookup(ROOT, "Logs", &mut ctx).unwrap().unwrap();
        assert!(fs.lookup(ROOT, "logs", &mut ctx).unwrap().is_none());
        assert_eq!(
            fs.read_dir(log, &mut ctx)
                .unwrap()
                .into_iter()
                .map(|e| e.name)
                .collect::<Vec<_>>(),
            ["Nested", "event.bin", "odd-%FF", "with%5Cbackslash%25"]
        );
        let nested = fs.lookup(log, "Nested", &mut ctx).unwrap().unwrap();
        let empty = fs.lookup(nested, "EmptyLater", &mut ctx).unwrap().unwrap();
        assert_eq!(fs.metadata(empty, &mut ctx).unwrap().size, 0);
        let event = fs.lookup(log, "event.bin", &mut ctx).unwrap().unwrap();
        let mut body = vec![0; expected.len() + 30];
        assert_eq!(
            fs.read_at(event, 0, &mut body, &mut ctx).unwrap(),
            expected.len()
        );
        assert_eq!(&body[..expected.len()], expected);
        assert_eq!(
            fs.read_at(event, 4291, &mut body[..6987], &mut ctx)
                .unwrap(),
            6987
        );
        assert_eq!(&body[..6987], &expected[4291..11278]);
        let split = fs.lookup(ROOT, "Split", &mut ctx).unwrap().unwrap();
        let found = fs.read_dir(split, &mut ctx).unwrap();
        assert_eq!(found.len(), 20);
        assert_eq!(found[19].name, "entry-19-long-file-name-for-split");
    }
}

#[test]
fn corrupt_metadata_and_ctz_pointers_fail_safely() {
    let mut cache = BlockCache::new(0);
    let mut ctx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut bytes = decode(MEDIUM);
    let head = {
        let mut fs = LittleFs::open(SliceSource::new(&bytes), &mut ctx).unwrap();
        let log = fs.lookup(ROOT, "Logs", &mut ctx).unwrap().unwrap();
        let event = fs.lookup(log, "event.bin", &mut ctx).unwrap().unwrap();
        let Content::Ctz(chain) = fs.node(event).unwrap() else {
            panic!("expected CTZ file")
        };
        chain.head_block as usize
    };
    bytes[head * 512..head * 512 + 32].fill(0xff);
    let mut fs = LittleFs::open(SliceSource::new(&bytes), &mut ctx).unwrap();
    let log = fs.lookup(ROOT, "Logs", &mut ctx).unwrap().unwrap();
    let event = fs.lookup(log, "event.bin", &mut ctx).unwrap().unwrap();
    assert!(matches!(
        fs.read_at(event, 0, &mut [0; 13], &mut ctx),
        Err(FoldError::Corrupt(_))
    ));
    bytes[..1024].fill(0xff);
    assert!(!LittleFs::probe(&mut SliceSource::new(&bytes)).unwrap());
    let mut short_crc = [0; 8];
    short_crc[4..].copy_from_slice(&(u32::MAX ^ (0x500 << 20)).to_be_bytes());
    assert!(crate::metadata::Block::committed(&short_crc).is_none());
}

#[test]
fn honors_a_committed_move_without_replaying_writes() {
    let mut cache = BlockCache::new(0);
    let mut ctx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let bytes = decode(INTERRUPTED);
    let mut fs = LittleFs::open(SliceSource::new(&bytes), &mut ctx).unwrap();
    assert!(fs.geometry.pending_move.is_some());
    assert!(fs.lookup(ROOT, "move", &mut ctx).unwrap().is_none());
    let logs = fs.lookup(ROOT, "Logs", &mut ctx).unwrap().unwrap();
    let nested = fs.lookup(logs, "Nested", &mut ctx).unwrap().unwrap();
    let moved = fs.lookup(nested, "EmptyLater", &mut ctx).unwrap().unwrap();
    let mut data = [0; 10];
    assert_eq!(fs.read_at(moved, 0, &mut data, &mut ctx).unwrap(), 10);
    assert_eq!(&data, b"after move");
}

#[test]
fn finds_valid_large_alternate_when_primary_has_no_magic() {
    let mut bytes = decode(LARGE);
    let (active, _) = Block::select(&bytes[..4096], &bytes[4096..8192]).unwrap();
    if active == 0 {
        bytes.copy_within(..4096, 4096);
    }
    bytes[..4096].fill(0xff);
    let header = LittleFs::header(&mut SliceSource::new(&bytes))
        .unwrap()
        .unwrap();
    assert_eq!(header.anchor_offset, 4096);
}
