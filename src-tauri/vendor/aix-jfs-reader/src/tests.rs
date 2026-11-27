use std::io::Read;

use flate2::read::ZlibDecoder;
use lamfold::{BlockCache, FoldError, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};

use crate::{AixJfs, Layout, PAGE};

const STANDARD: &[u8] = include_bytes!("../tests/fixtures/fsv3.zlib");
const FRAG_512: &[u8] = include_bytes!("../tests/fixtures/fsv3p-512.zlib");
const FRAG_1024: &[u8] = include_bytes!("../tests/fixtures/fsv3p-1024.zlib");
const FRAG_2048: &[u8] = include_bytes!("../tests/fixtures/fsv3p-2048.zlib");
const FRAG_4096: &[u8] = include_bytes!("../tests/fixtures/fsv3p-4096.zlib");

fn raw(compressed: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    ZlibDecoder::new(compressed)
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}

#[test]
fn both_layouts_preserve_fragments_inode_groups_and_indirection() {
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    for (compressed, layout, fragment) in [
        (STANDARD, Layout::Fsv3, 4096),
        (FRAG_512, Layout::Fsv3p, 512),
        (FRAG_1024, Layout::Fsv3p, 1024),
        (FRAG_2048, Layout::Fsv3p, 2048),
        (FRAG_4096, Layout::Fsv3p, 4096),
    ] {
        let bytes = raw(compressed);
        let mut fs = AixJfs::open(SliceSource::new(&bytes), &mut cx).unwrap();
        assert_eq!(
            (fs.header().layout, fs.header().fragment_size),
            (layout, fragment)
        );
        let root = fs.root();
        assert_eq!(fs.read_dir(root, &mut cx).unwrap().len(), 6);
        let tiny = fs.lookup(root, "tiny", &mut cx).unwrap().unwrap();
        let folder = fs.lookup(root, "dir", &mut cx).unwrap().unwrap();
        assert_eq!(fs.lookup(folder, "alias", &mut cx).unwrap(), Some(tiny));
        let mut data = [0; 32];
        let n = fs.read_at(tiny, 0, &mut data, &mut cx).unwrap();
        assert_eq!(&data[..n], b"aixjfs\n");
        assert_eq!(
            fs.timestamps(tiny, &mut cx).unwrap().modified,
            Some((-2, 85000))
        );
        let follow = fs.lookup(folder, "follow", &mut cx).unwrap().unwrap();
        let len = fs.read_at(follow, 0, &mut data, &mut cx).unwrap();
        assert_eq!(&data[..len], b"allocation group");
        assert_eq!(
            fs.read_at(7, 8 * PAGE - 2, &mut data[..5], &mut cx)
                .unwrap(),
            5
        );
        assert_eq!(&data[..5], b"\0\0ZZZ");
        assert_eq!(
            fs.read_at(8, 1025 * PAGE - 2, &mut data[..6], &mut cx)
                .unwrap(),
            6
        );
        assert_eq!(&data[..6], &[0, 0, 0, 1, 2, 3]);
        assert_eq!(fs.read_link(9, &mut cx).unwrap().unwrap(), b"tiny");
        assert_eq!(
            fs.read_link(10, &mut cx).unwrap().unwrap(),
            format!("{}tiny", "./".repeat(28)).as_bytes()
        );
    }
}

#[test]
fn backup_validation_and_short_allocations() {
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut bytes = raw(FRAG_512);
    bytes[4096 + 20..4096 + 22].fill(0);
    assert_eq!(
        AixJfs::open(SliceSource::new(&bytes), &mut cx)
            .unwrap()
            .header()
            .offset,
        31 * PAGE
    );

    let mut bytes = raw(FRAG_512);
    bytes[4096 + 16..4096 + 20].copy_from_slice(&2000u32.to_be_bytes());
    assert!(matches!(
        AixJfs::open(SliceSource::new(&bytes), &mut cx),
        Err(FoldError::Corrupt(_))
    ));

    let mut bytes = raw(FRAG_512);
    let size = 32 * PAGE as usize + 5 * 128 + 20;
    bytes[size..size + 4].copy_from_slice(&513u32.to_be_bytes());
    let mut fs = AixJfs::open(SliceSource::new(&bytes), &mut cx).unwrap();
    let mut data = [0; 64];
    assert!(matches!(
        fs.read_at(5, 0, &mut data, &mut cx),
        Err(FoldError::Corrupt(_))
    ));

    for pos in [4096, 31 * 4096] {
        bytes[pos + 7] = 1;
    }
    assert!(AixJfs::probe(&mut SliceSource::new(&bytes)).unwrap());
    assert!(matches!(
        AixJfs::open(SliceSource::new(&bytes), &mut cx),
        Err(FoldError::Unsupported(_))
    ));
}
