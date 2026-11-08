//! qemu-img differential tests — env-gated (skip when qemu-img is absent).
//!
//! Relocated out of the lib unit-test module so the strict production coverage
//! gate (which excludes `tests/`) is not dragged down by env-skip branches and
//! assert-failure message args. Exercises only the public `VhdReader` API.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::Command;
use vhd::VhdReader;

/// Resolve a usable `qemu-img`, or `None` (the differential then skips).
///
/// `PATH` is probed first, so any install location works — including ones a
/// fixed list cannot anticipate: another package manager's prefix, or a
/// hand-built install. The absolute
/// candidates are the fallback for a stripped `PATH`: Homebrew on Apple silicon
/// and Intel, then `/usr/bin`, where the Linux CI runner's `qemu-utils` package
/// lands it. `QEMU_IMG_BIN` overrides both.
///
/// The hardcoded Homebrew path this replaces resolved only on a macOS-arm64 dev
/// machine, so every qemu differential here skipped silently on the Linux CI
/// runner despite CI installing `qemu-utils` for exactly these tests.
fn qemu_img_bin() -> Option<String> {
    if let Ok(explicit) = std::env::var("QEMU_IMG_BIN") {
        return usable(&explicit);
    }
    [
        "qemu-img",
        "/opt/homebrew/bin/qemu-img",
        "/usr/local/bin/qemu-img",
        "/usr/bin/qemu-img",
    ]
    .into_iter()
    .find_map(usable)
}

/// A candidate counts only if it actually executes: a successful `--version`
/// proves both that the name resolved and that the binary runs on this host,
/// which a bare `Path::exists()` check does not.
fn usable(candidate: &str) -> Option<String> {
    Command::new(candidate)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| candidate.to_string())
}

// VHD uses CHS geometry, so the virtual size gets rounded from the source.
// Strategy: raw → VHD → raw_reference (both via qemu-img), then compare
// our reader output against raw_reference. qemu-img is authoritative for
// both conversions; we need not predict the CHS-rounded size ourselves.

#[test]
fn reads_match_qemu_raw_convert() {
    let Some(qemu_img) = qemu_img_bin() else {
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");

    // 1 MiB source with a deterministic non-trivial pattern.
    let size: usize = 1 << 20;
    let source_data: Vec<u8> = (0..size).map(|i| (i ^ (i >> 8)) as u8).collect();
    let raw_path = tmp.path().join("source.raw");
    std::fs::write(&raw_path, &source_data).expect("write raw");

    // raw → VHD (dynamic, qemu default VPC format).
    let vhd_path = tmp.path().join("test.vhd");
    let ok = Command::new(&qemu_img)
        .args([
            "convert",
            "-O",
            "vpc",
            raw_path.to_str().unwrap(),
            vhd_path.to_str().unwrap(),
        ])
        .status()
        .expect("spawn qemu-img")
        .success();
    assert!(ok, "qemu-img raw→vpc failed");

    // VHD → reference raw (qemu-img resolves CHS rounding authoritatively).
    let ref_path = tmp.path().join("reference.raw");
    let ok = Command::new(&qemu_img)
        .args([
            "convert",
            "-O",
            "raw",
            vhd_path.to_str().unwrap(),
            ref_path.to_str().unwrap(),
        ])
        .status()
        .expect("spawn qemu-img")
        .success();
    assert!(ok, "qemu-img vpc→raw failed");
    let ref_data = std::fs::read(&ref_path).expect("read reference raw");

    let mut reader = VhdReader::open(&vhd_path).expect("open vhd");
    let vhd_size = reader.virtual_disk_size() as usize;
    assert_eq!(
        vhd_size,
        ref_data.len(),
        "virtual_disk_size must match qemu-img reference raw size"
    );

    // Sample every 64 KiB (covers block boundaries) plus near-end.
    let step = 65536usize;
    let mut offset = 0usize;
    while offset < vhd_size {
        let len = 512.min(vhd_size - offset);
        let mut buf = vec![0u8; len];
        reader.seek(SeekFrom::Start(offset as u64)).expect("seek");
        reader.read_exact(&mut buf).expect("read");
        assert_eq!(
            buf,
            ref_data[offset..offset + len],
            "byte mismatch at offset {offset:#x}",
        );
        offset += step;
    }
    if vhd_size >= 512 {
        let end = vhd_size - 512;
        let mut buf = vec![0u8; 512];
        reader
            .seek(SeekFrom::Start(end as u64))
            .expect("seek near-end");
        reader.read_exact(&mut buf).expect("read near-end");
        assert_eq!(buf, ref_data[end..end + 512], "byte mismatch near end");
    }
}

// ── Corpus differential tests: real qemu-img generated VHDs ──────────────

fn corpus_vhd_matches_raw(corpus: &Path) {
    let Some(qemu_img) = qemu_img_bin() else {
        return;
    };
    if !corpus.exists() {
        return;
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    let ref_path = tmp.path().join("reference.raw");
    let ok = Command::new(&qemu_img)
        .args([
            "convert",
            "-O",
            "raw",
            corpus.to_str().unwrap(),
            ref_path.to_str().unwrap(),
        ])
        .status()
        .expect("spawn qemu-img")
        .success();
    assert!(ok, "qemu-img convert failed for {}", corpus.display());
    let ref_data = std::fs::read(&ref_path).expect("read raw");

    let mut reader = VhdReader::open(corpus).expect("open");
    let vhd_size = reader.virtual_disk_size() as usize;
    // qemu's vpc driver computes total_sectors = file_size / 512 for fixed
    // VHDs, treating the 512-byte footer as a trailing data sector.
    // Our reader follows the spec: virtual_disk_size = current_size (data only).
    // Accept ref_data being exactly vhd_size OR vhd_size + 512.
    assert!(
        ref_data.len() == vhd_size || ref_data.len() == vhd_size + 512,
        "unexpected ref raw size {} vs vhd_size {} for {}",
        ref_data.len(),
        vhd_size,
        corpus.display(),
    );
    let cmp_size = vhd_size; // never compare footer bytes

    let step = 65536usize;
    let mut offset = 0usize;
    while offset < cmp_size {
        let len = 512.min(cmp_size - offset);
        let mut buf = vec![0u8; len];
        reader.seek(SeekFrom::Start(offset as u64)).expect("seek");
        reader.read_exact(&mut buf).expect("read");
        assert_eq!(
            buf,
            ref_data[offset..offset + len],
            "byte mismatch at {offset:#x} in {}",
            corpus.display()
        );
        offset += step;
    }
}

#[test]
fn corpus_minimal_vhd_reads_match_qemu_raw_convert() {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/minimal.vhd");
    corpus_vhd_matches_raw(&p);
}

#[test]
fn corpus_fixed_vhd_reads_match_qemu_raw_convert() {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/fixed.vhd");
    corpus_vhd_matches_raw(&p);
}

#[test]
fn corpus_dfvfs_ntfs_fixed_vhd_reads_match_qemu_raw_convert() {
    // ntfs_fixed.vhd was created by Windows (creator_app = "win ", version 0x000A0000
    // = Windows 10 / Hyper-V). It is a fixed-type VHD with an NTFS filesystem,
    // sourced from the dfvfs project (Apache-2.0):
    // https://github.com/log2timeline/dfvfs/raw/main/test_data/ntfs-fixed.vhd
    // SHA-256: 797a3a1ffb1966b634bef79bf4b1e93641545cce8560b1f81d8d2c3f84b00de2
    // This is NOT QEMU-generated, providing independent cross-implementation validation.
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/ntfs_fixed.vhd");
    corpus_vhd_matches_raw(&p);
}
