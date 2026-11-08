//! Images from erofs-utils 1.9.4 that lamfold-erofs 0.1.1 misread or failed on,
//! built by `fixtures/regenerate.sh`. Each file's bytes are a formula the test
//! recomputes (see the script), except `lz4_zeropad`'s, which is checked by its
//! SHA-256.
//!
//! Run with the codecs on: `cargo test -p lamfold-erofs --features deflate,zstd,lzma`.
#![cfg(all(feature = "zstd", feature = "lzma"))]

use lamfold::{
    BlockCache, FileKind, FoldError, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx,
};
use lamfold_erofs::Erofs;
use sha2::{Digest, Sha256};

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

fn open(img: &'static [u8]) -> Result<Erofs<SliceSource<'static>>, FoldError> {
    let mut cache = BlockCache::new(0);
    let nov = NoVerifier;
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };
    Erofs::open(SliceSource::new(img), &mut cx)
}

/// Read `/name` whole.
fn read(fs: &mut Erofs<SliceSource<'static>>, name: &str) -> Vec<u8> {
    let mut cache = BlockCache::new(0);
    let nov = NoVerifier;
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };
    let root = fs.root();
    let node = fs
        .lookup(root, name, &mut cx)
        .unwrap_or_else(|e| panic!("lookup {name}: {e:?}"))
        .unwrap_or_else(|| panic!("{name} missing"));
    let md = fs.metadata(node, &mut cx).unwrap();
    assert_eq!(md.kind, FileKind::Regular, "{name}");
    let mut buf = vec![0u8; usize::try_from(md.size).unwrap()];
    let n = fs
        .read_at(node, 0, &mut buf, &mut cx)
        .unwrap_or_else(|e| panic!("read {name}: {e:?}"));
    assert_eq!(n, buf.len(), "{name}: short read");
    buf
}

/// Every file `f<i>.txt` of a formula tree reads back exactly.
fn check_tree(img: &'static [u8], count: usize, base: usize) {
    let mut fs = open(img).unwrap_or_else(|e| panic!("open: {e:?}"));
    for i in 0..count {
        let name = format!("f{i}.txt");
        assert!(
            read(&mut fs, &name) == payload(i, base + 1237 * i),
            "{name}: decoded bytes differ from the source"
        );
    }
}

/// An lz4hc pcluster with one byte of zero padding before its stream. 0.1.1
/// tried offset 0 first, which decodes "successfully" into wrong bytes.
#[test]
fn lz4_stream_starts_after_zero_padding() {
    let mut fs = open(include_bytes!("fixtures/lz4_zeropad.erofs")).unwrap();
    let data = read(&mut fs, "lz4pad.bin");
    let want = include_str!("fixtures/lz4_zeropad.sha256").trim();
    let got: String = Sha256::digest(&data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(got, want, "lz4pad.bin decoded wrong");
}

/// A 56-byte inline xattr area puts each file's compact index base on a 32-byte
/// boundary. 0.1.1 then placed every big pcluster one block too far.
#[test]
fn big_pcluster_index_on_a_32_byte_boundary() {
    check_tree(include_bytes!("fixtures/xattr_zstd.erofs"), 4, 40000);
    check_tree(include_bytes!("fixtures/xattr_lzma.erofs"), 4, 40000);
}

/// erofs-utils 1.9's custom LZMA lc/lp/pb, stored inverted in each stream's
/// first byte. 0.1.1 assumed the default properties.
#[test]
fn lzma_properties_come_from_the_stream() {
    check_tree(include_bytes!("fixtures/lzma_props.erofs"), 3, 30000);
}

/// Logical clusters follow the block size. 0.1.1 assumed 4 KiB.
#[test]
fn blocks_smaller_than_4k() {
    check_tree(include_bytes!("fixtures/blk1024_zstd.erofs"), 3, 30000);
    check_tree(include_bytes!("fixtures/blk2048_lz4hc.erofs"), 3, 30000);
}

fn refused(img: &'static [u8]) -> &'static str {
    match open(img) {
        Err(FoldError::Unsupported(why)) => why,
        Err(e) => panic!("expected Unsupported, got {e:?}"),
        Ok(_) => panic!("opened"),
    }
}

/// Layouts this reader can't read are refused at open. 0.1.1 returned wrong
/// file contents for fragments and an empty tree for compressed directories,
/// with no error.
#[test]
fn unsupported_layouts_are_refused_at_open() {
    assert!(refused(include_bytes!("fixtures/refuse_fragments.erofs")).contains("fragments"));
    assert!(refused(include_bytes!("fixtures/refuse_dircompress.erofs")).contains("fragments"));
    assert!(refused(include_bytes!("fixtures/refuse_48bit.erofs")).contains("48-bit"));
    assert!(refused(include_bytes!("fixtures/refuse_metabox.erofs")).contains("metabox"));
}
