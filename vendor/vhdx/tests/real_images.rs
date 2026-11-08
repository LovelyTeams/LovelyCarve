#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration tests against the committed VHDX/VHD real-image corpus.
//!
//! All fixtures are in `tests/data/` — provenance in `tests/data/SOURCES.md`.
//! Virtual disk sizes are verified against the SOURCES.md specifications.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const DATA_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");

// ── qemu_empty_dynamic.vhdx (QEMU v11, 16 MiB virtual, dynamic) ──────────────

#[test]
fn qemu_empty_dynamic_virtual_disk_size() {
    let path = format!("{DATA_DIR}/qemu_empty_dynamic.vhdx");
    let reader =
        vhdx::VhdxReader::open(Path::new(&path)).expect("qemu_empty_dynamic.vhdx must open");
    assert_eq!(
        reader.virtual_disk_size(),
        16_777_216,
        "qemu_empty_dynamic: 16 MiB virtual disk (SOURCES.md)"
    );
}

#[test]
fn qemu_empty_dynamic_sector0_is_zeros() {
    let path = format!("{DATA_DIR}/qemu_empty_dynamic.vhdx");
    let mut reader = vhdx::VhdxReader::open(Path::new(&path)).expect("open");
    let mut buf = [0xFFu8; 512];
    reader.seek(SeekFrom::Start(0)).expect("seek");
    reader.read_exact(&mut buf).expect("read sector 0");
    assert_eq!(
        buf, [0u8; 512],
        "empty dynamic VHDX — sector 0 must read as zeros"
    );
}

// ── qemu_fixed.vhdx (QEMU v11, 8 MiB virtual, fixed provisioning) ────────────

#[test]
fn qemu_fixed_virtual_disk_size() {
    let path = format!("{DATA_DIR}/qemu_fixed.vhdx");
    let reader = vhdx::VhdxReader::open(Path::new(&path)).expect("qemu_fixed.vhdx must open");
    assert_eq!(
        reader.virtual_disk_size(),
        8_388_608,
        "qemu_fixed: 8 MiB virtual disk (SOURCES.md)"
    );
}

#[test]
fn qemu_fixed_sector0_is_zeros() {
    let path = format!("{DATA_DIR}/qemu_fixed.vhdx");
    let mut reader = vhdx::VhdxReader::open(Path::new(&path)).expect("open");
    let mut buf = [0xFFu8; 512];
    reader.seek(SeekFrom::Start(0)).expect("seek");
    reader.read_exact(&mut buf).expect("read sector 0");
    assert_eq!(
        buf, [0u8; 512],
        "fixed VHDX with no filesystem — sector 0 must be zeros"
    );
}

// ── ext2.vhdx (log2timeline/dfvfs, 4 MiB virtual ext2 filesystem) ─────────────

#[test]
fn ext2_vhdx_virtual_disk_size() {
    let path = format!("{DATA_DIR}/ext2.vhdx");
    let reader = vhdx::VhdxReader::open(Path::new(&path)).expect("ext2.vhdx must open");
    assert_eq!(
        reader.virtual_disk_size(),
        4_194_304,
        "ext2.vhdx: 4 MiB virtual disk (SOURCES.md)"
    );
}

#[test]
fn ext2_vhdx_seek_and_read_are_stable() {
    let path = format!("{DATA_DIR}/ext2.vhdx");
    let mut reader = vhdx::VhdxReader::open(Path::new(&path)).expect("open");
    let mut a = [0u8; 512];
    reader.seek(SeekFrom::Start(0)).expect("seek");
    reader.read_exact(&mut a).expect("first read");
    let mut b = [0u8; 512];
    reader.seek(SeekFrom::Start(0)).expect("seek");
    reader.read_exact(&mut b).expect("second read");
    assert_eq!(a, b, "repeated reads at offset 0 must be identical");
}

// ── dfvfs_ext2.vhdx ──────────────────────────────────────────────────────────

#[test]
fn dfvfs_ext2_vhdx_opens_and_has_nonzero_size() {
    let path = format!("{DATA_DIR}/dfvfs_ext2.vhdx");
    let reader = vhdx::VhdxReader::open(Path::new(&path)).expect("dfvfs_ext2.vhdx must open");
    assert!(
        reader.virtual_disk_size() > 0,
        "dfvfs_ext2.vhdx virtual_disk_size must be > 0"
    );
}

// ── fat-parent.vhdx (dfvfs, FAT filesystem, standalone parent) ───────────────

#[test]
fn fat_parent_vhdx_opens_and_has_nonzero_size() {
    let path = format!("{DATA_DIR}/fat-parent.vhdx");
    let reader = vhdx::VhdxReader::open(Path::new(&path)).expect("fat-parent.vhdx must open");
    assert!(
        reader.virtual_disk_size() > 0,
        "fat-parent.vhdx virtual_disk_size must be > 0"
    );
}

// ── fat-differential.vhdx (dfvfs, differencing disk) ─────────────────────────
//
// A differencing VHDX references a parent disk. VhdxReader may or may not
// support differencing chains. The test documents the current behaviour and
// guards against panic on either path.

#[test]
fn fat_differential_vhdx_does_not_panic() {
    let path = format!("{DATA_DIR}/fat-differential.vhdx");
    let result = vhdx::VhdxReader::open(Path::new(&path));
    if let Ok(reader) = &result {
        assert!(
            reader.virtual_disk_size() > 0,
            "if fat-differential opens, it must report nonzero size"
        );
    } else {
        // Returning Err for a differencing disk is acceptable;
        // panicking is not.
    }
}

// ── ext2.vhd — must be rejected by VhdxReader (legacy VHD format) ────────────

#[test]
fn ext2_vhd_is_rejected_by_vhdx_reader() {
    let path = format!("{DATA_DIR}/ext2.vhd");
    let result = vhdx::VhdxReader::open(Path::new(&path));
    assert!(
        result.is_err(),
        "ext2.vhd is a legacy VHD file — VhdxReader must reject it with Err, not panic"
    );
}
