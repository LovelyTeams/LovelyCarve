//! Fixtures from `fixtures/regenerate-chunks.sh` (erofs-utils 1.9.4). Each
//! file's bytes are the formula regenerate.sh uses. lamfold-erofs 0.1.2
//! refused every chunk-based file, read an LZMA config at a fixed offset that
//! a superblock extension slot moves, and ignored Linux 7.2's hole bit.
#![cfg(feature = "lzma")]

use lamfold::{
    BlockCache, FileKind, FoldError, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx,
};
use lamfold_erofs::Erofs;

/// File `i` of length `len`: "lamfold-erofs fixture <i> line <k>\n" lines.
fn payload(i: usize, len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len + 64);
    let mut k = 0;
    while out.len() < len {
        out.extend_from_slice(format!("lamfold-erofs fixture {i} line {k}\n").as_bytes());
        k += 1;
    }
    out.truncate(len);
    out
}

const SIZES: [usize; 3] = [20000, 9000, 100];

fn read(img: &'static [u8], name: &str) -> Result<Vec<u8>, FoldError> {
    let mut cache = BlockCache::new(0);
    let nov = NoVerifier;
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };
    let mut fs = Erofs::open(SliceSource::new(img), &mut cx)?;
    let root = fs.root();
    let node = fs.lookup(root, name, &mut cx)?.expect(name);
    let md = fs.metadata(node, &mut cx)?;
    assert_eq!(md.kind, FileKind::Regular, "{name}");
    let mut buf = vec![0u8; md.size as usize];
    let n = fs.read_at(node, 0, &mut buf, &mut cx)?;
    assert_eq!(n, buf.len(), "{name}: short read");
    Ok(buf)
}

/// Chunk-based files with a 4-byte block map; the sparse file's empty chunks
/// are holes and read as zeros.
#[test]
fn chunk_based_files_read() {
    let img = include_bytes!("fixtures/chunks.erofs");
    for (i, &len) in SIZES.iter().enumerate() {
        assert!(
            read(img, &format!("f{i}.txt")).unwrap() == payload(i, len),
            "f{i}.txt"
        );
    }
    let sparse = read(img, "sparse.bin").unwrap();
    assert_eq!(sparse.len(), 262_144);
    let mut want = vec![0u8; 262_144];
    want[..22].copy_from_slice(b"head of a sparse file\n");
    want[65536..65543].copy_from_slice(b"middle\n");
    assert!(sparse == want, "sparse.bin");
}

/// The LZMA config follows the superblock's extension slot.
#[test]
fn compression_configs_follow_extension_slots() {
    let img = include_bytes!("fixtures/extslot_lzma.erofs");
    for (i, &len) in SIZES.iter().enumerate() {
        assert!(
            read(img, &format!("f{i}.txt")).unwrap() == payload(i, len),
            "f{i}.txt"
        );
    }
}

/// A full-index pcluster carrying Linux 7.2's hole bit is refused, not
/// decoded as if it held data.
#[test]
fn hole_pclusters_are_refused() {
    let img = include_bytes!("fixtures/hole_full.erofs");
    match read(img, "f0.txt") {
        Err(FoldError::Unsupported(why)) => assert_eq!(why, "erofs: hole pcluster not supported"),
        other => panic!(
            "expected the hole refusal, got {:?}",
            other.map(|v| v.len())
        ),
    }
    // The other files have no hole and still read.
    assert!(read(img, "f1.txt").unwrap() == payload(1, 9000));
}
