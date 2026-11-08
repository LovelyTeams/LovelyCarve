//! Adversarial robustness tests for the EWF v1 reader.
//!
//! Each test crafts a minimal EWF byte sequence with a poisoned field and
//! verifies that the reader returns an error rather than panicking.
//!
//! RED: tests fail (panics in debug mode or wrong error returned).
//! GREEN: tests pass after overflow-safe arithmetic and validation are added.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use tempfile::NamedTempFile;

use ewf::{EwfError, EwfReader};

// ── Minimal EWF v1 byte builder ───────────────────────────────────────────────

/// Magic header for EWF v1 segment files.
const EVF_MAGIC: [u8; 8] = [0x45, 0x56, 0x46, 0x09, 0x0d, 0x0a, 0xff, 0x00];

fn file_header(segment: u16) -> [u8; 13] {
    let mut h = [0u8; 13];
    h[0..8].copy_from_slice(&EVF_MAGIC);
    h[8] = 0x01;
    h[9..11].copy_from_slice(&segment.to_le_bytes());
    h[11..13].copy_from_slice(&0u16.to_le_bytes());
    h
}

/// Build a 76-byte section descriptor with the given fields (checksum ignored).
fn section_desc(section_type: &[u8], next: u64, section_size: u64) -> [u8; 76] {
    let mut d = [0u8; 76];
    let copy_len = section_type.len().min(16);
    d[..copy_len].copy_from_slice(&section_type[..copy_len]);
    d[16..24].copy_from_slice(&next.to_le_bytes());
    d[24..32].copy_from_slice(&section_size.to_le_bytes());
    d
}

/// Build a 94-byte volume/disk section payload.
fn volume_data(chunk_count: u32, sectors_per_chunk: u32, bytes_per_sector: u32) -> [u8; 94] {
    let mut v = [0u8; 94];
    // Offset 4: chunk_count, 8: sectors_per_chunk, 12: bytes_per_sector, 16: sector_count.
    v[4..8].copy_from_slice(&chunk_count.to_le_bytes());
    v[8..12].copy_from_slice(&sectors_per_chunk.to_le_bytes());
    v[12..16].copy_from_slice(&bytes_per_sector.to_le_bytes());
    v[16..24].copy_from_slice(&u64::from(chunk_count).to_le_bytes()); // sector_count
    v
}

/// Write bytes to a `NamedTempFile` with an .E01 extension. Returns (file, path).
fn write_temp_e01(content: &[u8]) -> (NamedTempFile, std::path::PathBuf) {
    let mut f = NamedTempFile::with_suffix(".E01").unwrap();
    f.write_all(content).unwrap();
    let path = f.path().to_path_buf();
    (f, path)
}

// Layout constants (byte offsets within the test images):
const FHDR: usize = 13; // file header end / first section descriptor start
const VOL_SECTION_SIZE: u64 = 76 + 94; // descriptor + volume data
const VOL_DATA_END: usize = FHDR + 76 + 94; // end of volume section data = 183
const SECTORS_DESC_OFF: usize = VOL_DATA_END; // sectors section starts here = 183
const DONE_DESC_OFF: usize = SECTORS_DESC_OFF + 76; // done section = 259

// ── Test 1: next = u64::MAX must not panic ────────────────────────────────────
//
// When the traversal advances desc_offset to u64::MAX, the loop's bounds
// check `desc_offset + SECTION_DESCRIPTOR_SIZE > file_len` overflows in
// debug mode → panic. After fix: checked_add → break cleanly.

#[test]
fn section_next_max_does_not_panic() {
    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(&file_header(1));
    // One "done" descriptor with next = u64::MAX (normally next=0 terminates chain).
    buf.extend_from_slice(&section_desc(b"done", u64::MAX, 76));
    let (_f, path) = write_temp_e01(&buf);
    // Must return Ok or Err — not panic.
    let _ = EwfReader::open(&path);
}

// ── Test 2: section_size = u64::MAX on "sectors" must not panic ───────────────
//
// `sectors_data_end = d.offset + d.section_size` overflows when
// section_size = u64::MAX. After fix: saturating_add.

#[test]
fn section_size_max_does_not_panic() {
    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(&file_header(1));

    // Volume descriptor → volume data → sectors descriptor (section_size=u64::MAX) → done.
    let sectors_off = SECTORS_DESC_OFF as u64;
    let done_off = DONE_DESC_OFF as u64;

    buf.extend_from_slice(&section_desc(b"volume", sectors_off, VOL_SECTION_SIZE));
    buf.extend_from_slice(&volume_data(1, 64, 512)); // chunk_size=32768, valid
    buf.extend_from_slice(&section_desc(b"sectors", done_off, u64::MAX)); // poisoned size
    buf.extend_from_slice(&section_desc(b"done", 0, 76));

    let (_f, path) = write_temp_e01(&buf);
    // Must return Ok or Err — not panic.
    let _ = EwfReader::open(&path);
}

// ── Test 3: chunk_size above MAX_CHUNK_SIZE is rejected at parse time ─────────
//
// Currently no MAX_CHUNK_SIZE guard; a huge value reaches read_chunk() where
// `vec![0u8; chunk_size]` would OOM. After fix: returns Err(InvalidChunkSize)
// at parse time.

#[test]
fn chunk_size_too_large_is_rejected() {
    // sectors_per_chunk=32769, bytes_per_sector=4096 → chunk_size ≈ 134 MB > 128 MB limit.
    let sectors_per_chunk: u32 = 32769;
    let bytes_per_sector: u32 = 4096;
    let done_off = (FHDR + 76 + 94 + 76) as u64;

    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(&file_header(1));
    buf.extend_from_slice(&section_desc(b"volume", done_off, VOL_SECTION_SIZE));
    buf.extend_from_slice(&volume_data(1, sectors_per_chunk, bytes_per_sector));
    buf.extend_from_slice(&section_desc(b"done", 0, 76));

    let (_f, path) = write_temp_e01(&buf);
    let err = EwfReader::open(&path).expect_err("huge chunk_size must be rejected");
    assert!(
        matches!(err, EwfError::InvalidChunkSize(_)),
        "expected InvalidChunkSize, got {err:?}"
    );
}

// ── Test 4: a large declared chunk_count is BOUNDED, not rejected
//
// The volume header's chunk_count is an UNTRUSTED hint, not a hard limit — a real
// 2 TB image has ~67M chunks. The DoS here is the OLD `chunks.reserve(chunk_count)`
// (u32::MAX would reserve ~137 GB); the fix bounds the reservation by file size
// (>= 4 B/chunk) instead of capping the count, so a huge count opens safely rather
// than rejecting every image over 128 GB (the former 4M ceiling). Regression guard
// against re-introducing an image-size ceiling.

#[test]
fn large_declared_chunk_count_is_bounded_not_rejected() {
    // 4_000_001 — one past the former 4M ceiling — with NO table section, so the
    // real chunk count is zero. Must open (count accepted) without OOMing.
    let huge_chunk_count: u32 = 4_000_001;
    let done_off = (FHDR + 76 + 94 + 76) as u64;

    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(&file_header(1));
    buf.extend_from_slice(&section_desc(b"volume", done_off, VOL_SECTION_SIZE));
    buf.extend_from_slice(&volume_data(huge_chunk_count, 64, 512));
    buf.extend_from_slice(&section_desc(b"done", 0, 76));

    let (_f, path) = write_temp_e01(&buf);
    let reader = EwfReader::open(&path)
        .expect("a large declared chunk_count must be accepted (bounded), not rejected");
    // No table section -> the inflated header count is ignored; real chunks = 0.
    assert_eq!(reader.chunk_count(), 0);
}
