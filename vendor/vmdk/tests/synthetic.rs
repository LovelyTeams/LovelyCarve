//! Cursor-based synthetic tests for `VmdkReader` — no disk I/O.
//!
//! All tests drive `VmdkReader::open(Cursor::new(img))`.  Because the public
//! API is generic (`VmdkReader<R: Read + Seek>`), no files are touched; a
//! Cursor<Vec<u8>> round-trips the same code paths as a real File.
//!
//! Format constants are duplicated here (from the private `testutil` /
//! `header` modules) because integration tests cannot reach private items.

use std::io::{Cursor, Read, Seek, SeekFrom};
use vmdk::VmdkReader;

// ── Format constants ──────────────────────────────────────────────────────────

const MAGIC: u32 = 0x564D_444B;
const SECTOR_SIZE: usize = 512;
const GRAIN_SIZE_SECTORS: u64 = 8;
const GRAIN_SIZE_BYTES: usize = GRAIN_SIZE_SECTORS as usize * SECTOR_SIZE;
const NUM_GTES_PER_GT: u32 = 512;

// Layout (sectors)
const DESCRIPTOR_OFFSET: u64 = 1;
const DESCRIPTOR_SECTORS: u64 = 20;
const GD_SECTOR: u64 = DESCRIPTOR_OFFSET + DESCRIPTOR_SECTORS; // 21
const RGD_SECTOR: u64 = GD_SECTOR + 1; // 22
const GT_SECTOR: u64 = RGD_SECTOR + 1; // 23
const GT_SECTORS: u64 = 4;
const GRAIN_SECTOR: u64 = GT_SECTOR + GT_SECTORS; // 27

// ── Builder ───────────────────────────────────────────────────────────────────

/// Build a minimal valid monolithic sparse VMDK with `sector_data` in grain 0.
fn minimal_vmdk(sector_data: &[u8]) -> Vec<u8> {
    let mut grain = vec![0u8; GRAIN_SIZE_BYTES];
    let n = sector_data.len().min(GRAIN_SIZE_BYTES);
    grain[..n].copy_from_slice(&sector_data[..n]);

    let mut hdr = vec![0u8; SECTOR_SIZE];
    hdr[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    hdr[4..8].copy_from_slice(&1u32.to_le_bytes()); // version
    hdr[8..12].copy_from_slice(&0u32.to_le_bytes()); // flags
    hdr[12..20].copy_from_slice(&GRAIN_SIZE_SECTORS.to_le_bytes()); // capacity (= 1 grain)
    hdr[20..28].copy_from_slice(&GRAIN_SIZE_SECTORS.to_le_bytes()); // grainSize
    hdr[28..36].copy_from_slice(&DESCRIPTOR_OFFSET.to_le_bytes());
    hdr[36..44].copy_from_slice(&DESCRIPTOR_SECTORS.to_le_bytes());
    hdr[44..48].copy_from_slice(&NUM_GTES_PER_GT.to_le_bytes());
    hdr[48..56].copy_from_slice(&RGD_SECTOR.to_le_bytes()); // rgdOffset
    hdr[56..64].copy_from_slice(&GD_SECTOR.to_le_bytes()); // gdOffset
    hdr[64..72].copy_from_slice(&GRAIN_SECTOR.to_le_bytes()); // overHead
    hdr[72] = 0; // uncleanShutdown
    hdr[73] = b'\n';
    hdr[74] = b' ';
    hdr[75] = b'\r';
    hdr[76] = b'\n';
    hdr[77..79].copy_from_slice(&0u16.to_le_bytes()); // compressAlgorithm = 0

    let mut desc = vec![0u8; DESCRIPTOR_SECTORS as usize * SECTOR_SIZE];
    let s = "# Disk DescriptorFile\nversion=1\nCID=fffffffe\nparentCID=ffffffff\ncreateType=\"monolithicSparse\"\n";
    let copy = s.len().min(desc.len());
    desc[..copy].copy_from_slice(&s.as_bytes()[..copy]);

    let mut gd = vec![0u8; SECTOR_SIZE];
    gd[0..4].copy_from_slice(&(GT_SECTOR as u32).to_le_bytes());
    let rgd = gd.clone();

    let mut gt = vec![0u8; GT_SECTORS as usize * SECTOR_SIZE];
    gt[0..4].copy_from_slice(&(GRAIN_SECTOR as u32).to_le_bytes());

    let mut out = Vec::new();
    out.extend_from_slice(&hdr);
    out.extend_from_slice(&desc);
    out.extend_from_slice(&gd);
    out.extend_from_slice(&rgd);
    out.extend_from_slice(&gt);
    out.extend_from_slice(&grain);
    out
}

// ── open ─────────────────────────────────────────────────────────────────────

#[test]
fn open_with_cursor_returns_ok() {
    let img = minimal_vmdk(&[0u8; SECTOR_SIZE]);
    let _ = VmdkReader::open(Cursor::new(img)).expect("Cursor open must succeed");
}

// ── error cases ───────────────────────────────────────────────────────────────

#[test]
fn open_empty_returns_err() {
    assert!(
        VmdkReader::open(Cursor::new(vec![])).is_err(),
        "empty bytes must return Err"
    );
}

#[test]
fn open_truncated_header_returns_err() {
    // Valid magic + version 1, then nothing — incomplete 512-byte header.
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    buf[4..8].copy_from_slice(&1u32.to_le_bytes());
    assert!(
        VmdkReader::open(Cursor::new(buf)).is_err(),
        "truncated archive must return Err"
    );
}

#[test]
fn open_bad_magic_returns_err() {
    let mut img = minimal_vmdk(&[0u8; SECTOR_SIZE]);
    img[0..4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
    assert!(
        VmdkReader::open(Cursor::new(img)).is_err(),
        "wrong magic must return Err"
    );
}

// ── virtual_disk_size ─────────────────────────────────────────────────────────

#[test]
fn virtual_disk_size_matches_header() {
    let img = minimal_vmdk(&[0u8; SECTOR_SIZE]);
    let reader = VmdkReader::open(Cursor::new(img)).expect("open");
    assert_eq!(reader.virtual_disk_size(), GRAIN_SIZE_BYTES as u64);
}

// ── read ─────────────────────────────────────────────────────────────────────

#[test]
fn read_grain0_returns_sector_data() {
    let mut data = vec![0u8; SECTOR_SIZE];
    data[42] = 0xDE;
    data[43] = 0xAD;
    let img = minimal_vmdk(&data);
    let mut reader = VmdkReader::open(Cursor::new(img)).expect("open");
    let mut buf = vec![0u8; SECTOR_SIZE];
    reader.read_exact(&mut buf).expect("read");
    assert_eq!(buf[42], 0xDE);
    assert_eq!(buf[43], 0xAD);
}

#[test]
fn read_at_eof_returns_zero_bytes() {
    let img = minimal_vmdk(&[0u8; GRAIN_SIZE_BYTES]);
    let mut reader = VmdkReader::open(Cursor::new(img)).expect("open");
    reader.seek(SeekFrom::End(0)).expect("seek to end");
    let mut buf = [0u8; 4];
    let n = reader.read(&mut buf).expect("read at EOF");
    assert_eq!(n, 0, "read past EOF must return 0");
}

// ── seek ─────────────────────────────────────────────────────────────────────

#[test]
fn seek_and_read_at_byte_offset_with_cursor() {
    let mut data = vec![0u8; GRAIN_SIZE_BYTES];
    data[100] = 0xBE;
    data[101] = 0xEF;
    let img = minimal_vmdk(&data);
    let mut reader = VmdkReader::open(Cursor::new(img)).expect("open");
    reader.seek(SeekFrom::Start(100)).expect("seek");
    let mut buf = [0u8; 2];
    reader.read_exact(&mut buf).expect("read");
    assert_eq!(buf, [0xBE, 0xEF]);
}

#[test]
fn seek_from_end_lands_at_virtual_disk_size() {
    let img = minimal_vmdk(&[0u8; GRAIN_SIZE_BYTES]);
    let mut reader = VmdkReader::open(Cursor::new(img)).expect("open");
    let pos = reader.seek(SeekFrom::End(0)).expect("seek to end");
    assert_eq!(pos, GRAIN_SIZE_BYTES as u64);
}

#[test]
fn seek_before_start_returns_err() {
    let img = minimal_vmdk(&[0u8; GRAIN_SIZE_BYTES]);
    let mut reader = VmdkReader::open(Cursor::new(img)).expect("open");
    assert!(
        reader.seek(SeekFrom::Current(-1)).is_err(),
        "seek before start must return Err"
    );
}

// ── Send bound ────────────────────────────────────────────────────────────────

#[test]
fn vmdk_reader_cursor_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<VmdkReader<Cursor<Vec<u8>>>>();
}

// ── disk_type ─────────────────────────────────────────────────────────────────

#[test]
fn disk_type_is_monolithic_sparse_for_minimal_vmdk() {
    let img = minimal_vmdk(&[0u8; SECTOR_SIZE]);
    let reader = VmdkReader::open(Cursor::new(img)).expect("open");
    assert_eq!(
        reader.disk_type(),
        "monolithicSparse",
        "embedded descriptor createType must be monolithicSparse"
    );
}

#[test]
fn disk_type_is_empty_when_no_descriptor() {
    // vmdk_header_bytes sets descriptor_offset=0, descriptor_size=0 → no descriptor.
    // We need a minimal VMDK that actually has a valid GD so open() succeeds.
    // Use minimal_vmdk but zero out the descriptor fields in the header.
    let mut img = minimal_vmdk(&[0u8; SECTOR_SIZE]);
    // bytes 28..36 = descriptor_offset → set to 0
    img[28..36].copy_from_slice(&0u64.to_le_bytes());
    // bytes 36..44 = descriptor_size → set to 0
    img[36..44].copy_from_slice(&0u64.to_le_bytes());
    let reader = VmdkReader::open(Cursor::new(img)).expect("open");
    assert_eq!(
        reader.disk_type(),
        "",
        "disk_type must be empty string when descriptor_offset=0"
    );
}
