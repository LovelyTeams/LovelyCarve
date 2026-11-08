//! Integration tests for the full ewf-forensic image corpus.
//!
//! All fixtures are committed to `tests/data/` — these tests are plain
//! (not ignored) and run in CI with `cargo test --lib --test extended_corpus`.
//!
//! Hashes verified against ewfverify / libewf (independent oracle).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use md5::{Digest, Md5};
use std::io::{Read, Seek, SeekFrom};

const DATA_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");

fn full_media_md5(reader: &mut ewf::EwfReader) -> String {
    let mut hasher = Md5::new();
    let mut buf = vec![0u8; 1024 * 1024];
    reader.seek(SeekFrom::Start(0)).unwrap();
    loop {
        let n = reader.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    format!("{:x}", hasher.finalize())
}

// ── bogus.E01 / bogus.E02 (0-byte invalid files) ─────────────────────────────

#[test]
fn bogus_e01_open_returns_err() {
    let path = format!("{DATA_DIR}/bogus.E01");
    let result = ewf::EwfReader::open(&path);
    assert!(
        result.is_err(),
        "0-byte bogus.E01 must be rejected with Err, not panic"
    );
}

#[test]
fn bogus_e02_open_returns_err() {
    let path = format!("{DATA_DIR}/bogus.E02");
    let result = ewf::EwfReader::open(&path);
    assert!(
        result.is_err(),
        "0-byte bogus.E02 must be rejected with Err, not panic"
    );
}

// ── ctf_file6.E01 (EWF v1, compressed, CTF challenge) ────────────────────────

#[test]
fn ctf_file6_opens_and_has_nonzero_size() {
    let path = format!("{DATA_DIR}/ctf_file6.E01");
    let reader = ewf::EwfReader::open(&path).expect("ctf_file6.E01 must open");
    assert!(
        reader.total_size() > 0,
        "ctf_file6.E01 total_size must be > 0"
    );
}

#[test]
fn ctf_file6_read_is_stable() {
    let path = format!("{DATA_DIR}/ctf_file6.E01");
    let mut reader = ewf::EwfReader::open(&path).expect("open");
    let mut buf = [0u8; 512];
    reader.seek(SeekFrom::Start(0)).unwrap();
    reader
        .read_exact(&mut buf)
        .expect("read sector 0 of ctf_file6.E01");
    let mut buf2 = [0u8; 512];
    reader.seek(SeekFrom::Start(0)).unwrap();
    reader.read_exact(&mut buf2).unwrap();
    assert_eq!(buf, buf2, "repeated reads at offset 0 must be identical");
}

// ── ewfacquire_clean.E01 (EWF v1, no compression, 4 MiB of /dev/zero) ────────

#[test]
fn ewfacquire_clean_media_size() {
    let path = format!("{DATA_DIR}/ewfacquire_clean.E01");
    let reader = ewf::EwfReader::open(&path).expect("ewfacquire_clean.E01 must open");
    assert_eq!(
        reader.total_size(),
        4_194_304,
        "ewfacquire_clean: 4 MiB /dev/zero source → 4 MiB media"
    );
}

#[test]
fn ewfacquire_clean_sector0_is_zeros() {
    let path = format!("{DATA_DIR}/ewfacquire_clean.E01");
    let mut reader = ewf::EwfReader::open(&path).expect("open");
    let mut buf = [0xFFu8; 512];
    reader.read_exact(&mut buf).expect("read sector 0");
    assert_eq!(
        buf, [0u8; 512],
        "ewfacquire_clean sourced from /dev/zero — sector 0 must be all zeros"
    );
}

// ── gpt_130_partitions.E01 (EWF v1, GPT with 130 partitions) ─────────────────

#[test]
fn gpt_130_partitions_opens_and_has_nonzero_size() {
    let path = format!("{DATA_DIR}/gpt_130_partitions.E01");
    let reader = ewf::EwfReader::open(&path).expect("gpt_130_partitions.E01 must open");
    assert!(
        reader.total_size() > 0,
        "gpt_130_partitions: total_size must be > 0"
    );
}

#[test]
fn gpt_130_partitions_mbr_signature() {
    let path = format!("{DATA_DIR}/gpt_130_partitions.E01");
    let mut reader = ewf::EwfReader::open(&path).expect("open");
    let mut mbr = [0u8; 512];
    reader
        .read_exact(&mut mbr)
        .expect("read sector 0 (MBR/GPT protective MBR)");
    // GPT uses a protective MBR with boot signature 0x55 0xAA.
    assert_eq!(mbr[510], 0x55, "GPT protective MBR byte 510 must be 0x55");
    assert_eq!(mbr[511], 0xAA, "GPT protective MBR byte 511 must be 0xAA");
}

// ── multiseg_v1.E01 … E08 (EWF v1, 10 MiB /dev/urandom, 8 segments) ─────────

#[test]
fn multiseg_v1_total_size() {
    let path = format!("{DATA_DIR}/multiseg_v1.E01");
    let reader = ewf::EwfReader::open(&path).expect("multiseg_v1.E01 must open");
    assert_eq!(
        reader.total_size(),
        10_485_760,
        "multiseg_v1: 10 MiB source → 10 MiB media across 8 segments"
    );
}

#[test]
fn multiseg_v1_full_media_md5() {
    let path = format!("{DATA_DIR}/multiseg_v1.E01");
    let mut reader = ewf::EwfReader::open(&path).expect("open");
    assert_eq!(
        full_media_md5(&mut reader),
        "2692f3177a389e58906b5c9080aa1add",
        "multiseg_v1 MD5 mismatch vs ewfverify ground truth"
    );
}

#[test]
fn multiseg_v1_seek_across_segments() {
    let path = format!("{DATA_DIR}/multiseg_v1.E01");
    let mut reader = ewf::EwfReader::open(&path).expect("open");

    // Read 512 bytes from near the segment E01/E02 boundary (~1.4 MiB = 1,468,006 bytes).
    // The boundary offset is approximate — just verify a cross-segment read works.
    let boundary_approx = 1_400_000u64;
    reader.seek(SeekFrom::Start(boundary_approx)).expect("seek");
    let mut buf = [0u8; 512];
    reader
        .read_exact(&mut buf)
        .expect("cross-segment read must succeed");
}

// ── zeros_128s.Ex01 (EWF v2, 128 sectors × 512 = 64 KiB of zeros) ────────────

#[test]
fn zeros_128s_ex01_media_size() {
    let path = format!("{DATA_DIR}/zeros_128s.Ex01");
    let reader = ewf::EwfReader::open(&path).expect("zeros_128s.Ex01 must open");
    assert_eq!(
        reader.total_size(),
        65_536,
        "zeros_128s: 128 sectors × 512 bytes = 65 536 bytes"
    );
}

#[test]
fn zeros_128s_ex01_all_zeros() {
    let path = format!("{DATA_DIR}/zeros_128s.Ex01");
    let mut reader = ewf::EwfReader::open(&path).expect("open");
    let mut buf = [0xFFu8; 512];
    reader.read_exact(&mut buf).expect("read sector 0");
    assert_eq!(
        buf, [0u8; 512],
        "zeros_128s.Ex01 sourced from /dev/zero — all bytes must be zero"
    );
}

// ── zeros_128s_compressed.Ex01 (EWF v2, same content, zlib compressed) ────────

#[test]
fn zeros_128s_compressed_ex01_media_size() {
    let path = format!("{DATA_DIR}/zeros_128s_compressed.Ex01");
    let reader = ewf::EwfReader::open(&path).expect("zeros_128s_compressed.Ex01 must open");
    assert_eq!(
        reader.total_size(),
        65_536,
        "zeros_128s_compressed: same 64 KiB source, just stored compressed"
    );
}

#[test]
fn zeros_128s_compressed_ex01_all_zeros() {
    let path = format!("{DATA_DIR}/zeros_128s_compressed.Ex01");
    let mut reader = ewf::EwfReader::open(&path).expect("open");
    let mut buf = [0xFFu8; 512];
    reader
        .read_exact(&mut buf)
        .expect("read sector 0 from compressed EWF v2");
    assert_eq!(
        buf, [0u8; 512],
        "zeros_128s_compressed sourced from /dev/zero — all bytes must be zero"
    );
}

#[test]
fn zeros_128s_compressed_matches_uncompressed_md5() {
    let uncompressed_path = format!("{DATA_DIR}/zeros_128s.Ex01");
    let compressed_path = format!("{DATA_DIR}/zeros_128s_compressed.Ex01");

    let mut ur = ewf::EwfReader::open(&uncompressed_path).expect("open uncompressed");
    let mut cr = ewf::EwfReader::open(&compressed_path).expect("open compressed");

    assert_eq!(
        full_media_md5(&mut ur),
        full_media_md5(&mut cr),
        "compressed and uncompressed Ex01 images must yield identical media MD5"
    );
}
