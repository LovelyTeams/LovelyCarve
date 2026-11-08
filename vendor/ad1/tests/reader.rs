//! Integration tests for `ad1-core` against spec-faithful crafted fixtures.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation
)]
use std::fmt::Write as _;

use ad1::testfix as common;
use ad1::{Ad1Error, Ad1Reader};
use md5::{Digest as _, Md5};
use std::path::PathBuf;

fn write_one(bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("image.ad1");
    std::fs::write(&path, bytes).unwrap();
    (dir, path)
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

// ---- detection + headers (build step 1/2) -------------------------------

#[test]
fn rejects_non_ad1_and_shows_the_bytes() {
    let garbage = b"\x00\x00\x00\xc1\x0e not an ad1 file at all..........";
    let (_d, p) = write_one(garbage);
    match Ad1Reader::open(&p) {
        Err(Ad1Error::NotAd1(msg)) => {
            // Show-the-bytes: the diagnostic must include the offending signature.
            assert!(
                msg.contains("00") && msg.to_lowercase().contains("c1"),
                "msg={msg}"
            );
        }
        other => panic!("expected NotAd1, got {other:?}"),
    }
}

#[test]
fn refuses_adcrypt_encrypted_images() {
    let mut bytes = vec![0u8; 512];
    bytes[0..8].copy_from_slice(b"ADCRYPT\0");
    let (_d, p) = write_one(&bytes);
    match Ad1Reader::open(&p) {
        Err(Ad1Error::Unsupported(msg)) => {
            assert!(
                msg.to_lowercase().contains("encrypt") || msg.contains("ADCRYPT"),
                "msg={msg}"
            );
        }
        other => panic!("expected Unsupported(ADCRYPT), got {other:?}"),
    }
}

#[test]
fn opens_valid_image_and_parses_headers() {
    let built = common::build(common::sample_tree());
    let (_d, p) = write_one(&built.bytes);
    let r = Ad1Reader::open(&p).expect("should open crafted AD1");
    assert_eq!(r.segment_count(), 1);
    assert_eq!(r.image_version(), 4);
    assert_eq!(r.chunk_size(), common::CHUNK_SIZE);
}

// ---- file tree walk (build step 3) --------------------------------------

#[test]
fn entries_match_the_logical_tree_in_dfs_order() {
    let built = common::build(common::sample_tree());
    let (_d, p) = write_one(&built.bytes);
    let r = Ad1Reader::open(&p).unwrap();
    let got: Vec<_> = r
        .entries()
        .iter()
        .map(|e| (e.path.clone(), e.is_dir, e.size))
        .collect();
    let want: Vec<_> = built
        .expected
        .iter()
        .map(|e| (e.path.clone(), e.is_dir, e.size))
        .collect();
    assert_eq!(got, want);
}

#[test]
fn stored_hashes_are_exposed_for_files() {
    let built = common::build(common::sample_tree());
    let (_d, p) = write_one(&built.bytes);
    let r = Ad1Reader::open(&p).unwrap();
    for (entry, exp) in r.entries().iter().zip(built.expected.iter()) {
        assert_eq!(entry.md5, exp.md5, "md5 for {}", exp.path);
        assert_eq!(entry.sha1, exp.sha1, "sha1 for {}", exp.path);
    }
}

// ---- positioned decompression (build step 4) ----------------------------

fn find<'a>(r: &'a Ad1Reader, path: &str) -> &'a ad1::Ad1Entry {
    r.entries().iter().find(|e| e.path == path).expect("entry")
}

#[test]
fn read_at_whole_file_matches_data_and_stored_hash() {
    let built = common::build(common::sample_tree());
    let (_d, p) = write_one(&built.bytes);
    let r = Ad1Reader::open(&p).unwrap();
    let entry = find(&r, "root/sub/a.bin");
    let mut buf = vec![0u8; entry.size as usize];
    let n = r.read_at(entry, 0, &mut buf).unwrap();
    assert_eq!(n, entry.size as usize);
    let exp = built
        .expected
        .iter()
        .find(|e| e.path == "root/sub/a.bin")
        .unwrap();
    assert_eq!(&buf, exp.data.as_ref().unwrap());
    // Independent oracle: recomputed md5 equals the AD1-stored md5.
    assert_eq!(Some(hex(&Md5::digest(&buf))), entry.md5);
}

#[test]
fn read_at_partial_across_chunk_boundary() {
    let built = common::build(common::sample_tree());
    let (_d, p) = write_one(&built.bytes);
    let r = Ad1Reader::open(&p).unwrap();
    let entry = find(&r, "root/sub/a.bin");
    let exp = built
        .expected
        .iter()
        .find(|e| e.path == "root/sub/a.bin")
        .unwrap();
    let full = exp.data.as_ref().unwrap();
    // Read 100 bytes straddling the 64 KiB chunk boundary.
    let off = 65_500u64;
    let mut buf = vec![0u8; 100];
    let n = r.read_at(entry, off, &mut buf).unwrap();
    assert_eq!(n, 100);
    assert_eq!(&buf[..], &full[off as usize..off as usize + 100]);
}

#[test]
fn read_at_past_end_returns_zero() {
    let built = common::build(common::sample_tree());
    let (_d, p) = write_one(&built.bytes);
    let r = Ad1Reader::open(&p).unwrap();
    let entry = find(&r, "root/hello.txt");
    let mut buf = [0u8; 16];
    let n = r.read_at(entry, 9999, &mut buf).unwrap();
    assert_eq!(n, 0);
}

// ---- multi-segment chaining (build step 5) ------------------------------

#[test]
fn reads_file_data_spanning_multiple_segments() {
    let built = common::build(common::sample_tree());
    let logical_len = built.bytes.len() - common::MARGIN;
    // fragments_size = 2 -> stride ≈ 127 KiB, so the 200 KB file crosses segments.
    let fragments_size = 2u32;
    let stride = (fragments_size as usize) * 0x1_0000 - common::MARGIN;
    let n = logical_len.div_ceil(stride) as u32;
    assert!(n >= 2, "fixture should split into >= 2 segments, got {n}");
    let segs = common::split(&built.bytes, n, fragments_size);

    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("multi.ad1");
    for (i, seg) in segs.iter().enumerate() {
        let name = if i == 0 {
            "multi.ad1".to_string()
        } else {
            format!("multi.ad{}", i + 1)
        };
        std::fs::write(dir.path().join(name), seg).unwrap();
    }

    let r = Ad1Reader::open(&first).unwrap();
    assert_eq!(r.segment_count(), n);
    let entry = find(&r, "root/sub/a.bin");
    let mut buf = vec![0u8; entry.size as usize];
    let read = r.read_at(entry, 0, &mut buf).unwrap();
    assert_eq!(read, entry.size as usize);
    let exp = built
        .expected
        .iter()
        .find(|e| e.path == "root/sub/a.bin")
        .unwrap();
    assert_eq!(&buf, exp.data.as_ref().unwrap());
    assert_eq!(Some(hex(&Md5::digest(&buf))), entry.md5);
}

#[test]
fn missing_later_segment_errors_when_data_needs_it() {
    let built = common::build(common::sample_tree());
    let logical_len = built.bytes.len() - common::MARGIN;
    let fragments_size = 2u32;
    let stride = (fragments_size as usize) * 0x1_0000 - common::MARGIN;
    let n = logical_len.div_ceil(stride) as u32;
    let segs = common::split(&built.bytes, n, fragments_size);

    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("gap.ad1");
    // Write ONLY the first segment; the rest are missing.
    std::fs::write(&first, &segs[0]).unwrap();

    let r = Ad1Reader::open(&first).unwrap();
    // The absent later segment is reported (for the AD1-SEGMENT-MISSING audit).
    assert_eq!(r.missing_segments(), vec![2]);
    let entry = find(&r, "root/sub/a.bin");
    let mut buf = vec![0u8; entry.size as usize];
    // a.bin spans into a missing segment -> loud error, not silent truncation.
    assert!(matches!(
        r.read_at(entry, 0, &mut buf),
        Err(Ad1Error::Malformed(_))
    ));
}

#[test]
fn read_at_small_file_exact() {
    let built = common::build(common::sample_tree());
    let (_d, p) = write_one(&built.bytes);
    let r = Ad1Reader::open(&p).unwrap();
    let entry = find(&r, "root/hello.txt");
    let mut buf = vec![0u8; entry.size as usize];
    let n = r.read_at(entry, 0, &mut buf).unwrap();
    assert_eq!(&buf[..n], b"Hello, AD1!\n");
}
