//! Integration tests against committed VMDK real-image corpus.
//!
//! All fixtures are in `tests/data/` — provenance in `tests/data/README.md`.
//! Images that the reader doesn't support must return `Err`, not panic.

use std::io::{Cursor, Read, Seek, SeekFrom};

const DATA_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");

fn read_fixture(name: &str) -> Vec<u8> {
    let path = format!("{DATA_DIR}/{name}");
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {name}: {e}"))
}

// ── minimal.vmdk (monolithicSparse v1, 1 MiB virtual) ────────────────────────

#[test]
fn minimal_vmdk_virtual_disk_size() {
    let data = read_fixture("minimal.vmdk");
    let reader = vmdk::VmdkReader::open(Cursor::new(data)).expect("minimal.vmdk must open");
    assert_eq!(
        reader.virtual_disk_size(),
        1_048_576,
        "minimal.vmdk: 1 MiB virtual disk (README)"
    );
}

#[test]
fn minimal_vmdk_sector0_is_zeros() {
    let data = read_fixture("minimal.vmdk");
    let mut reader = vmdk::VmdkReader::open(Cursor::new(data)).expect("open");
    let mut buf = [0xFFu8; 512];
    reader.seek(SeekFrom::Start(0)).expect("seek");
    reader.read_exact(&mut buf).expect("read sector 0");
    assert_eq!(
        buf, [0u8; 512],
        "freshly-created sparse VMDK — sector 0 must be all zeros"
    );
}

// ── dfvfs_ext2.vmdk (dfvfs corpus, ext2 filesystem) ──────────────────────────

#[test]
fn dfvfs_ext2_vmdk_opens_and_has_nonzero_size() {
    let data = read_fixture("dfvfs_ext2.vmdk");
    let reader = vmdk::VmdkReader::open(Cursor::new(data)).expect("dfvfs_ext2.vmdk must open");
    assert!(
        reader.virtual_disk_size() > 0,
        "dfvfs_ext2.vmdk virtual_disk_size must be > 0"
    );
}

#[test]
fn dfvfs_ext2_vmdk_read_is_stable() {
    let data = read_fixture("dfvfs_ext2.vmdk");
    let mut reader = vmdk::VmdkReader::open(Cursor::new(data)).expect("open");
    let mut a = [0u8; 512];
    reader.seek(SeekFrom::Start(0)).expect("seek");
    reader.read_exact(&mut a).expect("first read");
    let mut b = [0u8; 512];
    reader.seek(SeekFrom::Start(0)).expect("seek");
    reader.read_exact(&mut b).expect("second read");
    assert_eq!(a, b, "repeated reads at offset 0 must be identical");
}

// ── disk_type from embedded descriptor ───────────────────────────────────────

#[test]
fn minimal_vmdk_disk_type_is_monolithic_sparse() {
    let data = read_fixture("minimal.vmdk");
    let reader = vmdk::VmdkReader::open(Cursor::new(data)).expect("open");
    assert_eq!(
        reader.disk_type(),
        "monolithicSparse",
        "minimal.vmdk must report createType monolithicSparse"
    );
}

#[test]
fn dfvfs_ext2_disk_type_is_monolithic_sparse() {
    let data = read_fixture("dfvfs_ext2.vmdk");
    let reader = vmdk::VmdkReader::open(Cursor::new(data)).expect("open");
    assert_eq!(
        reader.disk_type(),
        "monolithicSparse",
        "dfvfs_ext2.vmdk (VMware4 origin) must report createType monolithicSparse"
    );
}

// ── flat.vmdk (twoGbMaxExtentFlat) via open_path ─────────────────────────────

#[test]
fn flat_vmdk_opens_via_open_path() {
    let path = format!("{DATA_DIR}/flat.vmdk");
    let reader = vmdk::VmdkReader::open_path(std::path::Path::new(&path))
        .expect("flat.vmdk must open via open_path");
    assert_eq!(
        reader.virtual_disk_size(),
        1_048_576,
        "flat.vmdk: 2048 sectors * 512 = 1 MiB virtual disk"
    );
    assert_eq!(
        reader.disk_type(),
        "twoGbMaxExtentFlat",
        "flat.vmdk must report createType twoGbMaxExtentFlat"
    );
}

#[test]
fn flat_vmdk_reads_return_zeros_via_open_path() {
    let path = format!("{DATA_DIR}/flat.vmdk");
    let mut reader = vmdk::VmdkReader::open_path(std::path::Path::new(&path))
        .expect("open flat.vmdk via open_path");
    let mut buf = [0xFFu8; 512];
    reader.read_exact(&mut buf).expect("read sector 0");
    assert_eq!(buf, [0u8; 512], "flat-f001.vmdk is entirely zero");
}

// ── Unsupported formats — must return Err, never panic ────────────────────────

// stream_opt.vmdk was previously rejected with UnsupportedVersion(3).
// After adding v3 support it must open successfully and return zeros for
// the all-sparse empty 1 MiB disk.

#[test]
fn stream_opt_vmdk_opens_and_has_correct_size() {
    let data = read_fixture("stream_opt.vmdk");
    let reader =
        vmdk::VmdkReader::open(Cursor::new(data)).expect("streamOptimized v3 must open via open()");
    assert_eq!(
        reader.virtual_disk_size(),
        1_048_576,
        "stream_opt.vmdk: 2048 sectors * 512 = 1 MiB"
    );
    assert_eq!(
        reader.disk_type(),
        "streamOptimized",
        "stream_opt.vmdk must report createType streamOptimized"
    );
}

#[test]
fn stream_opt_vmdk_reads_return_zeros() {
    let data = read_fixture("stream_opt.vmdk");
    let mut reader = vmdk::VmdkReader::open(Cursor::new(data)).expect("open stream_opt.vmdk");
    let mut buf = [0xFFu8; 512];
    reader.read_exact(&mut buf).expect("read sector 0");
    assert_eq!(
        buf, [0u8; 512],
        "all-sparse streamOptimized VMDK must read as zeros"
    );
}

// ── plaso_image.vmdk (VMware Workstation 4, monolithicSparse, real data) ─────
//
// Generated by VMware (not QEMU): virtualHWVersion=4, adapterType=ide.
// 200 sectors (102,400 bytes) capacity, grain_size=128 sectors.
// Contains real non-zero data starting at virtual offset 1024 (sector 2).
// Source: log2timeline/plaso test_data corpus (Apache 2.0).

#[test]
fn plaso_image_vmdk_opens_and_has_correct_size() {
    let data = read_fixture("plaso_image.vmdk");
    let reader = vmdk::VmdkReader::open(Cursor::new(data)).expect("plaso_image.vmdk must open");
    assert_eq!(
        reader.virtual_disk_size(),
        102_400,
        "plaso_image.vmdk: 200 sectors * 512 = 102,400 bytes"
    );
    assert_eq!(
        reader.disk_type(),
        "monolithicSparse",
        "plaso_image.vmdk must report createType monolithicSparse"
    );
}

#[test]
fn plaso_image_vmdk_read_is_stable() {
    let data = read_fixture("plaso_image.vmdk");
    let mut reader = vmdk::VmdkReader::open(Cursor::new(data)).expect("open");
    let mut a = [0u8; 512];
    reader.seek(SeekFrom::Start(0)).expect("seek");
    reader.read_exact(&mut a).expect("first read");
    let mut b = [0u8; 512];
    reader.seek(SeekFrom::Start(0)).expect("seek");
    reader.read_exact(&mut b).expect("second read");
    assert_eq!(a, b, "repeated reads at offset 0 must be identical");
}

#[test]
fn plaso_image_vmdk_has_real_data_at_offset_1024() {
    // Virtual offset 1024 is the first non-zero location in this image.
    // These 16 bytes are stable properties of the plaso corpus fixture.
    let data = read_fixture("plaso_image.vmdk");
    let mut reader = vmdk::VmdkReader::open(Cursor::new(data)).expect("open");
    reader.seek(SeekFrom::Start(1024)).expect("seek to 1024");
    let mut buf = [0u8; 16];
    reader
        .read_exact(&mut buf)
        .expect("read 16 bytes at offset 1024");
    assert_eq!(
        buf,
        [
            0x10, 0x00, 0x00, 0x00, 0x64, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x4a, 0x00,
            0x00, 0x00
        ],
        "plaso_image.vmdk: known data at virtual offset 1024 must match corpus"
    );
}

// ── ms3-win.vmdk (Metasploitable3 Windows 2008, twoGbMaxExtentSparse) ────────
//
// Descriptor-only file (1 KB) from the Rapid7 Metasploitable3 VMware Vagrant
// box (virtualHWVersion=13, built with Packer vmware-iso). References 16 ×
// disk-sNNN.vmdk SPARSE extents which are not committed (total ~60 GB).
// Source: vagrantcloud.com/rapid7/metasploitable3-win2k8, vmware_desktop provider.
//
// open()      → Err (text descriptor, no VMDK binary header → BadMagic)
// open_path() → Err (extent files not present → Io(NotFound) on disk-s001.vmdk)

#[test]
fn ms3_win_descriptor_open_returns_err() {
    let data = read_fixture("ms3-win.vmdk");
    let result = vmdk::VmdkReader::open(Cursor::new(data));
    assert!(
        result.is_err(),
        "twoGbMaxExtentSparse text descriptor opened via open() must return Err"
    );
}

#[test]
fn ms3_win_two_gb_max_extent_sparse_open_path_returns_err() {
    let path = format!("{DATA_DIR}/ms3-win.vmdk");
    let result = vmdk::VmdkReader::open_path(std::path::Path::new(&path));
    assert!(
        result.is_err(),
        "twoGbMaxExtentSparse with missing extent files must return Err(Io(NotFound))"
    );
}

#[test]
fn flat_vmdk_descriptor_returns_err() {
    let data = read_fixture("flat.vmdk");
    let result = vmdk::VmdkReader::open(Cursor::new(data));
    assert!(
        result.is_err(),
        "monolithic flat VMDK descriptor must be rejected with Err, not panic"
    );
}

#[test]
fn flat_f001_vmdk_returns_err() {
    // flat-f001.vmdk is the raw extent data file for flat.vmdk.
    // Its first 4 bytes are 0x00000000 — valid file size but zero magic.
    // Must be rejected as BadMagic, never panic.
    let data = read_fixture("flat-f001.vmdk");
    let result = vmdk::VmdkReader::open(Cursor::new(data));
    assert!(
        result.is_err(),
        "flat extent data (zero magic) must be rejected with Err, not panic"
    );
}

// ── monolithicFlat (twoGbMaxExtentFlat sibling, same FLAT extent format) ──────
//
// mono_flat.vmdk + mono_flat-flat.vmdk generated by:
//   qemu-img create -f vmdk -o subformat=monolithicFlat tests/data/mono_flat.vmdk 1M
// Descriptor: createType="monolithicFlat", RW 2048 FLAT "mono_flat-flat.vmdk" 0
// Flat file: raw 1 MiB of zeros.

#[test]
fn mono_flat_vmdk_opens_via_open_path() {
    let path = format!("{DATA_DIR}/mono_flat.vmdk");
    let reader = vmdk::VmdkReader::open_path(std::path::Path::new(&path))
        .expect("monolithicFlat must open via open_path");
    assert_eq!(reader.virtual_disk_size(), 1_048_576, "1 MiB virtual disk");
    assert_eq!(reader.disk_type(), "monolithicFlat");
}

#[test]
fn mono_flat_vmdk_reads_return_zeros() {
    let path = format!("{DATA_DIR}/mono_flat.vmdk");
    let mut reader =
        vmdk::VmdkReader::open_path(std::path::Path::new(&path)).expect("open mono_flat.vmdk");
    let mut buf = [0xFFu8; 512];
    reader.read_exact(&mut buf).expect("read sector 0");
    assert_eq!(buf, [0u8; 512], "mono_flat-flat.vmdk is entirely zero");
}

// ── twoGbMaxExtentSparse: multi-file sparse VMDKs ────────────────────────────
//
// tw_sparse.vmdk + tw_sparse-s001.vmdk (all-sparse, 4 MiB, generated by):
//   qemu-img create -f vmdk -o subformat=twoGbMaxExtentSparse tests/data/tw_sparse.vmdk 4M
//
// tw_sparse_data.vmdk + tw_sparse_data-s001.vmdk (real grain data, 4 MiB):
//   qemu-img convert -f raw -O vmdk -o subformat=twoGbMaxExtentSparse pat4m.raw ...
//   Source: bytes(i%256 for i in range(4*1024*1024))

#[test]
fn tw_sparse_vmdk_opens_via_open_path() {
    let path = format!("{DATA_DIR}/tw_sparse.vmdk");
    let reader = vmdk::VmdkReader::open_path(std::path::Path::new(&path))
        .expect("twoGbMaxExtentSparse must open via open_path");
    assert_eq!(reader.virtual_disk_size(), 4_194_304, "4 MiB virtual disk");
    assert_eq!(reader.disk_type(), "twoGbMaxExtentSparse");
}

#[test]
fn tw_sparse_vmdk_reads_return_zeros() {
    let path = format!("{DATA_DIR}/tw_sparse.vmdk");
    let mut reader =
        vmdk::VmdkReader::open_path(std::path::Path::new(&path)).expect("open tw_sparse.vmdk");
    let mut buf = [0xFFu8; 512];
    reader.read_exact(&mut buf).expect("read sector 0");
    assert_eq!(
        buf, [0u8; 512],
        "all-sparse twoGbMaxExtentSparse must read as zeros"
    );
}

#[test]
fn tw_sparse_data_vmdk_reads_correct_pattern() {
    // GTE[0]=128 in tw_sparse_data-s001.vmdk → grain 0 at byte 65536.
    // Source pattern: bytes(i%256 for i in range(4MiB)).
    // First 16 bytes of grain 0 = [0, 1, 2, ..., 15].
    let path = format!("{DATA_DIR}/tw_sparse_data.vmdk");
    let mut reader =
        vmdk::VmdkReader::open_path(std::path::Path::new(&path)).expect("open tw_sparse_data.vmdk");
    let mut buf = [0u8; 16];
    reader.seek(SeekFrom::Start(0)).expect("seek");
    reader.read_exact(&mut buf).expect("read 16 bytes");
    assert_eq!(
        buf,
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
        "tw_sparse_data: grain 0 must contain source pattern (verified empirically)"
    );
}

// ── streamOptimized: allocated compressed grain must decompress correctly ─────
//
// compressed_stream_opt.vmdk generated by:
//   qemu-img convert -f raw -O vmdk -o subformat=streamOptimized pat64k.raw ...
//   Source: bytes(i%64 for i in range(65536))
// GTE[0]=128 → GrainMarker at byte 65536 (LBA=0, dataSize=280) + 280 bytes zlib.
// Decompressed grain: 65536 bytes, first 16 = [0, 1, 2, ..., 15].

#[test]
fn compressed_stream_opt_opens_and_has_correct_size() {
    let data = read_fixture("compressed_stream_opt.vmdk");
    let reader =
        vmdk::VmdkReader::open(Cursor::new(data)).expect("compressed_stream_opt.vmdk must open");
    assert_eq!(
        reader.virtual_disk_size(),
        65_536,
        "128 sectors * 512 = 64 KiB"
    );
    assert_eq!(reader.disk_type(), "streamOptimized");
}

#[test]
fn compressed_stream_opt_reads_correct_data() {
    // The decompressed grain at virtual offset 0 must match the source pattern.
    // First 16 bytes: [0, 1, 2, ..., 15] (verified with Python zlib.decompress).
    let data = read_fixture("compressed_stream_opt.vmdk");
    let mut reader =
        vmdk::VmdkReader::open(Cursor::new(data)).expect("open compressed_stream_opt.vmdk");
    let mut buf = [0u8; 16];
    reader.read_exact(&mut buf).expect("read 16 bytes");
    assert_eq!(
        buf,
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
        "decompressed grain must match source pattern bytes(i%64 for i in range(65536))"
    );
}

// ── extent_dependencies against real QEMU/VMware descriptors ─────────────────

#[test]
fn extent_dependencies_real_flat_vmdk() {
    // Real twoGbMaxExtentFlat descriptor references flat-f001.vmdk (a committed file).
    let path = std::path::Path::new(DATA_DIR).join("flat.vmdk");
    let deps = vmdk::VmdkFileReader::extent_dependencies(&path).expect("deps");
    assert_eq!(deps.len(), 1, "flat.vmdk has one companion extent");
    assert_eq!(
        deps[0].file_name().unwrap().to_string_lossy(),
        "flat-f001.vmdk"
    );
    assert!(
        deps[0].exists(),
        "the reported companion file must exist on disk"
    );
}

#[test]
fn extent_dependencies_real_two_gb_sparse_vmdk() {
    // Real twoGbMaxExtentSparse descriptor references tw_sparse-s001.vmdk.
    let path = std::path::Path::new(DATA_DIR).join("tw_sparse.vmdk");
    let deps = vmdk::VmdkFileReader::extent_dependencies(&path).expect("deps");
    assert!(
        deps.iter()
            .any(|p| p.file_name().unwrap() == "tw_sparse-s001.vmdk"),
        "must list the SPARSE companion, got: {deps:?}"
    );
    for d in &deps {
        assert!(d.exists(), "companion {d:?} must exist");
    }
}

#[test]
fn extent_dependencies_real_monolithic_sparse_is_empty() {
    // dfvfs_ext2.vmdk is a self-contained binary monolithicSparse — no companions.
    let path = std::path::Path::new(DATA_DIR).join("dfvfs_ext2.vmdk");
    let deps = vmdk::VmdkFileReader::extent_dependencies(&path).expect("deps");
    assert!(
        deps.is_empty(),
        "self-contained binary VMDK has no deps, got: {deps:?}"
    );
}

#[test]
fn compressed_streamoptimized_reads_fully() {
    // compressed_stream_opt.vmdk has DEFLATE grains — reading to the end exercises the
    // compressed-grain decode path.
    let data = read_fixture("compressed_stream_opt.vmdk");
    let mut reader = vmdk::VmdkReader::open(Cursor::new(data)).expect("open compressed");
    let mut buf = vec![0u8; 65536];
    let mut total = 0u64;
    loop {
        match std::io::Read::read(&mut reader, &mut buf) {
            Ok(0) => break,
            Ok(n) => total += n as u64,
            Err(e) => panic!("compressed read failed: {e}"),
        }
    }
    assert_eq!(total, reader.virtual_disk_size());
}
