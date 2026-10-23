//! Integration test against a real `mkfs.erofs -zlz4 -E legacy-compress` image
//! (datalayout 1, `COMPRESSED_FULL`, LZ4 head).
//!
//! Fixture `fixtures/comp.erofs` (4 KiB blocks, extended inodes):
//!   /comp.bin   131072 B, partially compressible → a mix of PLAIN, HEAD1 and
//!               NONHEAD lclusters (the cross-pcluster LZ4 dictionary is
//!               exercised). The exact source bytes are `fixtures/comp.payload`.
//!   /note.txt   "erofs lz4-full fixture\n" (inline)
//!
//! `comp.payload` is the ground truth: the reader's decode must reproduce it
//! byte-for-byte.

use lamfold::{BlockCache, FileKind, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};
use lamfold_erofs::Erofs;

const IMG: &[u8] = include_bytes!("fixtures/comp.erofs");
const PAYLOAD: &[u8] = include_bytes!("fixtures/comp.payload");

fn open() -> (Erofs<SliceSource<'static>>, BlockCache, NoVerifier) {
    let mut cache = BlockCache::new(0);
    let nov = NoVerifier;
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };
    let fs = Erofs::open(SliceSource::new(IMG), &mut cx).unwrap();
    (fs, cache, nov)
}

#[test]
fn full_decode_matches_source_byte_for_byte() {
    let (mut fs, mut cache, nov) = open();
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };
    let root = fs.root();

    let names: Vec<String> = fs
        .read_dir(root, &mut cx)
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(names.contains(&"comp.bin".to_string()), "got {names:?}");
    assert!(names.contains(&"note.txt".to_string()), "got {names:?}");

    let comp = fs.lookup(root, "comp.bin", &mut cx).unwrap().unwrap();
    let md = fs.metadata(comp, &mut cx).unwrap();
    assert_eq!(md.kind, FileKind::Regular);
    assert_eq!(md.size as usize, PAYLOAD.len());

    let mut buf = vec![0u8; PAYLOAD.len()];
    let n = fs.read_at(comp, 0, &mut buf, &mut cx).unwrap();
    assert_eq!(n, PAYLOAD.len());
    assert!(buf == PAYLOAD, "decoded bytes diverge from source");

    // Reading past EOF yields nothing.
    assert_eq!(fs.read_at(comp, md.size, &mut buf, &mut cx).unwrap(), 0);
}

#[test]
fn partial_reads_straddling_pclusters_match() {
    let (mut fs, mut cache, nov) = open();
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };
    let root = fs.root();
    let comp = fs.lookup(root, "comp.bin", &mut cx).unwrap().unwrap();

    // A window that spans several 4 KiB pcluster boundaries, so the read exercises
    // the decode cache across multiple decompressed clusters.
    for &(off, len) in &[
        (0usize, 100usize),
        (4090, 4108),
        (70_001, 9000),
        (131_000, 72),
    ] {
        let mut got = vec![0u8; len];
        let n = fs.read_at(comp, off as u64, &mut got, &mut cx).unwrap();
        assert_eq!(n, len, "short read at off={off}");
        assert_eq!(&got[..n], &PAYLOAD[off..off + len], "mismatch at off={off}");
    }
}

#[test]
fn inline_sibling_still_reads() {
    let (mut fs, mut cache, nov) = open();
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };
    let root = fs.root();
    let note = fs.lookup(root, "note.txt", &mut cx).unwrap().unwrap();
    let mut buf = vec![0u8; fs.metadata(note, &mut cx).unwrap().size as usize];
    fs.read_at(note, 0, &mut buf, &mut cx).unwrap();
    assert_eq!(&buf, b"erofs lz4-full fixture\n");
}
