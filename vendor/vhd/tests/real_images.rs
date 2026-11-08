#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration tests against committed VHD real-image corpus.
//!
//! All fixtures are in `tests/data/` — provenance in `tests/data/README.md`.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const DATA_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");

// ── minimal.vhd (dynamic, ~1 MiB virtual) ────────────────────────────────────

#[test]
fn minimal_vhd_virtual_disk_size() {
    let path = format!("{DATA_DIR}/minimal.vhd");
    let reader = vhd::VhdReader::open(Path::new(&path)).expect("minimal.vhd must open");
    // qemu-img rounds up to a VHD cylinder boundary; the resulting virtual size
    // is 1,079,296 bytes (2108 sectors) rather than the nominal 1 MiB (2048 sectors).
    assert_eq!(
        reader.virtual_disk_size(),
        1_079_296,
        "minimal.vhd: qemu-img rounds to CHS boundary → 1_079_296 bytes"
    );
}

#[test]
fn minimal_vhd_sector0_is_zeros() {
    let path = format!("{DATA_DIR}/minimal.vhd");
    let mut reader = vhd::VhdReader::open(Path::new(&path)).expect("open");
    let mut buf = [0xFFu8; 512];
    reader.seek(SeekFrom::Start(0)).expect("seek");
    reader.read_exact(&mut buf).expect("read sector 0");
    assert_eq!(
        buf, [0u8; 512],
        "empty dynamic VHD — sector 0 must be all zeros"
    );
}

// ── fixed.vhd (fixed, ~1 MiB virtual) ────────────────────────────────────────

#[test]
fn fixed_vhd_virtual_disk_size() {
    let path = format!("{DATA_DIR}/fixed.vhd");
    let reader = vhd::VhdReader::open(Path::new(&path)).expect("fixed.vhd must open");
    // Same CHS rounding as minimal.vhd — both were created with qemu-img 1M.
    assert_eq!(
        reader.virtual_disk_size(),
        1_079_296,
        "fixed.vhd: qemu-img rounds to CHS boundary → 1_079_296 bytes"
    );
}

#[test]
fn fixed_vhd_sector0_is_zeros() {
    let path = format!("{DATA_DIR}/fixed.vhd");
    let mut reader = vhd::VhdReader::open(Path::new(&path)).expect("open");
    let mut buf = [0xFFu8; 512];
    reader.seek(SeekFrom::Start(0)).expect("seek");
    reader.read_exact(&mut buf).expect("read sector 0");
    assert_eq!(
        buf, [0u8; 512],
        "fixed VHD with no filesystem — sector 0 must be all zeros"
    );
}

// ── ntfs_fixed.vhd (fixed, 4 MiB, NTFS filesystem) ───────────────────────────

#[test]
fn ntfs_fixed_vhd_opens_and_has_nonzero_size() {
    let path = format!("{DATA_DIR}/ntfs_fixed.vhd");
    let reader = vhd::VhdReader::open(Path::new(&path)).expect("ntfs_fixed.vhd must open");
    assert!(
        reader.virtual_disk_size() > 0,
        "ntfs_fixed.vhd virtual_disk_size must be > 0"
    );
}

#[test]
fn ntfs_fixed_vhd_seek_and_read_stable() {
    let path = format!("{DATA_DIR}/ntfs_fixed.vhd");
    let mut reader = vhd::VhdReader::open(Path::new(&path)).expect("open");
    let mut a = [0u8; 512];
    reader.seek(SeekFrom::Start(0)).expect("seek");
    reader.read_exact(&mut a).expect("first read");
    let mut b = [0u8; 512];
    reader.seek(SeekFrom::Start(0)).expect("seek");
    reader.read_exact(&mut b).expect("second read");
    assert_eq!(a, b, "repeated reads at offset 0 must be identical");
}

// ── original-size-mismatch.vhd (dynamic; OriginalSize@40 ≠ CurrentSize@48) ────
//
// A real qemu-img dynamic VHD whose *CurrentSize* (offset 48) is untouched at
// 2,123,776 (qemu-img reports exactly this), with only *OriginalSize* (offset 40)
// injected to a distinct 1,061,888 — the shape a resized disk has. The reader must
// return the CURRENT (virtual) size from offset 48, matching qemu-img, NOT the
// original from offset 40. qemu-img is the independent oracle. See tests/data/README.md.

#[test]
fn current_size_read_from_offset_48_not_40() {
    let path = format!("{DATA_DIR}/original-size-mismatch.vhd");
    let reader = vhd::VhdReader::open(Path::new(&path)).expect("mismatch.vhd must open");
    assert_eq!(
        reader.virtual_disk_size(),
        2_123_776,
        "current_size must come from footer offset 48 (qemu-img's virtual size), \
         not offset 40 (OriginalSize)"
    );
}

// ── ntfs_dynamic.vhd (dfvfs; third-party real Windows dynamic VHD) — TIER 1 ───
//
// A real Windows-authored (creator "win ", Win10/Hyper-V) dynamic NTFS VHD from
// log2timeline/dfvfs (Apache-2.0). Virtual size 4,194,304 B as reported by
// `qemu-img info -f vpc` — an INDEPENDENT reference oracle we did not author.
// Third-party real-world artifact + independent oracle = Tier-1. Dynamic VHDs read
// current_size cleanly (no fixed-VHD footer-as-sector ambiguity). tests/data/README.md.
#[test]
fn dfvfs_dynamic_vhd_matches_reference_oracle() {
    let path = format!("{DATA_DIR}/ntfs_dynamic.vhd");
    let reader = vhd::VhdReader::open(Path::new(&path)).expect("open dfvfs dynamic vhd");
    assert_eq!(
        reader.virtual_disk_size(),
        4_194_304,
        "must match qemu-img -f vpc (independent oracle) on the third-party dfvfs artifact"
    );
    assert_eq!(reader.disk_type(), vhd::DiskType::Dynamic);
}

// Expose BOTH sizes: current (readable capacity, offset 48) and original (creation
// size, offset 40). Each is validated by a different reference oracle — qemu-img
// reports current 2,123,776; libvhdi reports original 1,061,888. Original != current
// is a forensic resize signal.
#[test]
fn exposes_both_current_and_original_size() {
    let path = format!("{DATA_DIR}/original-size-mismatch.vhd");
    let r = vhd::VhdReader::open(Path::new(&path)).expect("open");
    assert_eq!(
        r.virtual_disk_size(),
        2_123_776,
        "current size (offset 48, qemu oracle)"
    );
    assert_eq!(
        r.original_size(),
        1_061_888,
        "original size (offset 40, libvhdi oracle)"
    );
}

#[test]
fn real_vhd_original_equals_current() {
    // A real (un-resized) VHD has original == current; both reference tools agree.
    let path = format!("{DATA_DIR}/ntfs_dynamic.vhd");
    let r = vhd::VhdReader::open(Path::new(&path)).expect("open");
    assert_eq!(r.original_size(), r.virtual_disk_size());
    assert_eq!(r.original_size(), 4_194_304);
}
