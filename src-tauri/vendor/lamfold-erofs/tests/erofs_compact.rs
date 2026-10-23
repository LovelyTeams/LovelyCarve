//! COMPRESSED_COMPACT (datalayout 3) integration tests against real
//! `mkfs.erofs 1.8.6` images — the bit-packed default index plus the deflate,
//! zstd, and MicroLZMA head algorithms (each over big-pcluster, mkfs's default
//! for those codecs).
//!
//! Each `fixtures/comp_<codec>.erofs` carries `/payload.bin` (28 lclusters of
//! mixed-compressibility data: a repeating phrase + random per 4 KiB, so the
//! image exercises the 4B-initial, 2B-pack, and 4B-final index zones, PLAIN
//! lclusters, multi-block pclusters, and the right-aligned partial-final
//! pcluster). The decoded bytes must reproduce `fixtures/comp_multi.payload`
//! byte-for-byte. Regenerate with `mkfs.erofs -z<codec> -b4096`.
//!
//! Codec arms are behind their cargo feature; run e.g.
//! `cargo test -p lamfold-erofs --features deflate,zstd,lzma`.

use lamfold::{BlockCache, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};
use lamfold_erofs::Erofs;

const PAYLOAD: &[u8] = include_bytes!("fixtures/comp_multi.payload");

fn decode_whole(img: &'static [u8]) {
    decode_against(img, PAYLOAD);
}

fn decode_against(img: &'static [u8], payload: &[u8]) {
    let mut cache = BlockCache::new(0);
    let nov = NoVerifier;
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };
    let mut fs = Erofs::open(SliceSource::new(img), &mut cx).unwrap();
    let root = fs.root();
    let file = fs.lookup(root, "payload.bin", &mut cx).unwrap().unwrap();

    let md = fs.metadata(file, &mut cx).unwrap();
    assert_eq!(md.size as usize, payload.len(), "size mismatch");

    let mut buf = vec![0u8; payload.len()];
    let n = fs.read_at(file, 0, &mut buf, &mut cx).unwrap();
    assert_eq!(n, payload.len(), "short read");
    assert!(buf == payload, "decoded bytes diverge from source");

    // Partial reads straddling pcluster / lcluster boundaries.
    for &(off, len) in &[
        (0usize, 100usize),
        (4090, 4108.min(payload.len() - 4090)),
        (payload.len() - 37, 37),
    ] {
        let mut got = vec![0u8; len];
        let r = fs.read_at(file, off as u64, &mut got, &mut cx).unwrap();
        assert_eq!(r, len, "short straddling read at {off}");
        assert_eq!(&got[..r], &payload[off..off + len], "mismatch at {off}");
    }

    // Reading at EOF yields nothing.
    let mut tail = [0u8; 16];
    assert_eq!(fs.read_at(file, md.size, &mut tail, &mut cx).unwrap(), 0);
}

/// The compact (datalayout 3) **index** itself, isolated from a novel codec:
/// lz4 over the validated decode engine but through the bit-packed index (advise
/// `0x0001`, non-big-pcluster). A failure here localizes to the index unpack.
#[test]
fn compact_lz4_index_decodes() {
    decode_whole(include_bytes!("fixtures/comp_lz4.erofs"));
}

#[cfg(feature = "deflate")]
#[test]
fn compact_deflate_big_pcluster_decodes() {
    decode_whole(include_bytes!("fixtures/comp_deflate.erofs"));
}

#[cfg(feature = "zstd")]
#[test]
fn compact_zstd_big_pcluster_decodes() {
    decode_whole(include_bytes!("fixtures/comp_zstd.erofs"));
}

#[cfg(feature = "lzma")]
#[test]
fn compact_microlzma_big_pcluster_decodes() {
    decode_whole(include_bytes!("fixtures/comp_lzma.erofs"));
}

/// Regression: a compact lz4 image whose pcluster stream is RIGHT-ALIGNED within
/// its block (leading zero pad), not left-aligned. Decoding from block offset 0
/// fails on the zero pad; the reader must find the true stream start. (mkfs emits
/// this for short/partial lz4 pclusters.)
#[test]
fn compact_lz4_right_aligned_stream_decodes() {
    decode_against(
        include_bytes!("fixtures/comp_ra_lz4.erofs"),
        include_bytes!("fixtures/comp_ra_lz4.payload"),
    );
}

/// Regression: an image whose index walk yields a final HEAD/PLAIN lcluster with a
/// zero-length extent (its logical start lands exactly at i_size) — its blkaddr can
/// point one block past the compressed data (EOF), so the decoder must skip it
/// rather than read an out-of-range span.
#[cfg(feature = "deflate")]
#[test]
fn compact_trailing_zero_length_head_decodes() {
    decode_against(
        include_bytes!("fixtures/comp_zerohead.erofs"),
        include_bytes!("fixtures/comp_zerohead.payload"),
    );
}

/// An INLINE_PCLUSTER (ztailpacking) image must refuse cleanly — its tail data is
/// packed after the index, not where the pcluster blkaddr points, so decoding it
/// as a normal pcluster would surface wrong bytes. The gate returns `Unsupported`,
/// never a panic or a silent miscompare.
#[cfg(feature = "deflate")]
#[test]
fn ztailpacking_is_gated_unsupported() {
    let img: &[u8] = include_bytes!("fixtures/comp_ztail.erofs");
    let mut cache = BlockCache::new(0);
    let nov = NoVerifier;
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };
    let mut fs = Erofs::open(SliceSource::new(img), &mut cx).unwrap();
    let root = fs.root();
    let file = fs.lookup(root, "payload.bin", &mut cx).unwrap().unwrap();
    let mut buf = vec![0u8; PAYLOAD.len()];
    let r = fs.read_at(file, 0, &mut buf, &mut cx);
    assert!(
        matches!(r, Err(lamfold::FoldError::Unsupported(_))),
        "ztailpacking must surface Unsupported, got {r:?}"
    );
}

/// Hostile media must never panic or over-read: truncations and byte flips across
/// a valid compact image (superblock, inode, and the bit-packed index) must each
/// resolve to an `Ok`/`Err` — never a panic. Mirrors the substrate's
/// `garbage_input_never_panics` discipline for the new compact-index parser.
#[test]
fn hostile_compact_image_never_panics() {
    let base: &[u8] = include_bytes!("fixtures/comp_lz4.erofs");

    let probe = |img: &[u8]| {
        let mut cache = BlockCache::new(0);
        let nov = NoVerifier;
        let mut cx = SubstrateCtx {
            cache: &mut cache,
            verifier: &nov,
        };
        if let Ok(mut fs) = Erofs::open(SliceSource::new(img), &mut cx) {
            let root = fs.root();
            if let Ok(Some(file)) = fs.lookup(root, "payload.bin", &mut cx) {
                let mut buf = vec![0u8; 16384];
                let _ = fs.read_at(file, 0, &mut buf, &mut cx);
            }
        }
    };

    for cut in [64usize, 200, 1400, 2048, base.len() / 2, base.len() - 1] {
        probe(&base[..cut]);
    }
    for i in (0..base.len()).step_by(251) {
        let mut img = base.to_vec();
        img[i] ^= 0xff;
        probe(&img);
    }
}
