#![allow(clippy::unwrap_used, clippy::expect_used)]
//! The bounded `open(path)` reader must return byte-identical reads to the
//! legacy whole-file `from_bytes(std::fs::read(path))` path.
//!
//! `open` reads only the small fixed structures + on-demand blocks (peak RSS no
//! longer scales with image size); `from_bytes` holds the whole image in RAM and
//! replays the log in place. They must agree byte-for-byte over the full virtual
//! disk and over hundreds of random reads — otherwise the bounded refactor
//! changed behaviour.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use vhdx::VhdxReader;

const DATA_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");

/// Deterministic xorshift — no external RNG dep, reproducible offsets/lengths.
struct XorShift(u64);
impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

fn read_full(reader: &mut VhdxReader, size: usize) -> Vec<u8> {
    let mut out = vec![0u8; size];
    reader.seek(SeekFrom::Start(0)).expect("seek 0");
    reader.read_exact(&mut out).expect("read full");
    out
}

fn assert_open_matches_from_bytes(path: &Path) {
    // Fixtures under tests/data/ are committed, so absence is a broken checkout.
    assert!(
        path.exists(),
        "committed fixture missing: {}",
        path.display()
    );

    // Oracle: legacy whole-file path.
    let bytes = std::fs::read(path).expect("read file bytes");
    let mut oracle = VhdxReader::from_bytes(bytes).expect("from_bytes oracle");
    let size = oracle.virtual_disk_size() as usize;

    // Bounded path under test.
    let mut bounded = VhdxReader::open(path).expect("bounded open");
    assert_eq!(
        bounded.virtual_disk_size() as usize,
        size,
        "virtual_disk_size must match for {}",
        path.display()
    );

    // 1) Full virtual disk byte-identical.
    let want = read_full(&mut oracle, size);
    let got = read_full(&mut bounded, size);
    assert!(
        want == got,
        "full-disk read mismatch for {}",
        path.display()
    );

    // 2) 400 random reads byte-identical (offsets + lengths spanning block and
    //    sector boundaries, including reads that run to the very end).
    let mut rng = XorShift(0x9E37_79B9_7F4A_7C15 ^ size as u64);
    for _ in 0..400 {
        let off = (rng.next() % size as u64) as usize;
        let max_len = (size - off).min(1 << 17); // up to 128 KiB
        let len = if max_len == 0 {
            0
        } else {
            (rng.next() as usize % max_len) + 1
        };

        let mut a = vec![0u8; len];
        oracle
            .seek(SeekFrom::Start(off as u64))
            .expect("oracle seek");
        oracle.read_exact(&mut a).expect("oracle read");

        let mut b = vec![0u8; len];
        bounded
            .seek(SeekFrom::Start(off as u64))
            .expect("bounded seek");
        bounded.read_exact(&mut b).expect("bounded read");

        assert!(
            a == b,
            "random read mismatch at off={off:#x} len={len} in {}",
            path.display()
        );
    }
}

#[test]
fn open_matches_from_bytes_ext2() {
    assert_open_matches_from_bytes(&PathBuf::from(format!("{DATA_DIR}/ext2.vhdx")));
}

#[test]
fn open_matches_from_bytes_qemu_fixed() {
    assert_open_matches_from_bytes(&PathBuf::from(format!("{DATA_DIR}/qemu_fixed.vhdx")));
}

#[test]
fn open_matches_from_bytes_qemu_empty_dynamic() {
    assert_open_matches_from_bytes(&PathBuf::from(format!(
        "{DATA_DIR}/qemu_empty_dynamic.vhdx"
    )));
}

#[test]
fn open_matches_from_bytes_fat_parent() {
    assert_open_matches_from_bytes(&PathBuf::from(format!("{DATA_DIR}/fat-parent.vhdx")));
}

#[test]
fn open_matches_from_bytes_dfvfs_ext2() {
    assert_open_matches_from_bytes(&PathBuf::from(format!("{DATA_DIR}/dfvfs_ext2.vhdx")));
}

// ── Dirty-image equivalence: in-place log replay (`from_bytes`) vs the bounded
//    log overlay (`open`). This is the riskiest path of the refactor — the two
//    must produce the same committed bytes. The image builder mirrors the proven
//    construction in `log_replay.rs` (a dirty log entry patching data block 0).

use vhdx::header::crc32c;
use vhdx::metadata::{GUID_FILE_PARAMETERS, GUID_LOGICAL_SECTOR_SIZE, GUID_VIRTUAL_DISK_SIZE};
use vhdx::region::{BAT_GUID, METADATA_GUID};

const FILE_SIZE: usize = 0x0050_0000;
const LOG_OFFSET: u64 = 0x0010_0000;
const LOG_LENGTH: u32 = 0x0010_0000;
const META_OFFSET: u64 = 0x0020_0000;
const META_LENGTH: u32 = 0x0010_0000;
const BAT_OFFSET: u64 = 0x0030_0000;
const BAT_LENGTH: u32 = 0x0010_0000;
const DATA_OFFSET: u64 = 0x0040_0000;
const VIRTUAL_DISK_SIZE: u64 = 0x0010_0000;
const BLOCK_SIZE: u32 = 0x0010_0000;
const LOG_GUID: [u8; 16] = [
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10,
];

fn write_header(slot: &mut [u8], seq: u64, log_guid: [u8; 16], log_off: u64, log_len: u32) {
    let h = &mut slot[..4096];
    h[0..4].copy_from_slice(b"head");
    h[4..8].fill(0);
    h[8..16].copy_from_slice(&seq.to_le_bytes());
    h[48..64].copy_from_slice(&log_guid);
    h[64..66].copy_from_slice(&1u16.to_le_bytes());
    h[66..68].copy_from_slice(&1u16.to_le_bytes());
    h[68..72].copy_from_slice(&log_len.to_le_bytes());
    h[72..80].copy_from_slice(&log_off.to_le_bytes());
    let c = crc32c(h);
    h[4..8].copy_from_slice(&c.to_le_bytes());
}

fn write_region_table(rt: &mut [u8], bat_off: u64, bat_len: u32, meta_off: u64, meta_len: u32) {
    rt[0..4].copy_from_slice(b"regi");
    rt[4..8].fill(0);
    rt[8..12].copy_from_slice(&2u32.to_le_bytes());
    rt[12..16].fill(0);
    rt[16..32].copy_from_slice(&BAT_GUID);
    rt[32..40].copy_from_slice(&bat_off.to_le_bytes());
    rt[40..44].copy_from_slice(&bat_len.to_le_bytes());
    rt[44..48].copy_from_slice(&1u32.to_le_bytes());
    rt[48..64].copy_from_slice(&METADATA_GUID);
    rt[64..72].copy_from_slice(&meta_off.to_le_bytes());
    rt[72..76].copy_from_slice(&meta_len.to_le_bytes());
    rt[76..80].copy_from_slice(&1u32.to_le_bytes());
    let mut tmp = rt[..65536].to_vec();
    tmp[4..8].fill(0);
    let c = crc32c(&tmp);
    rt[4..8].copy_from_slice(&c.to_le_bytes());
}

fn write_metadata(region: &mut [u8]) {
    region[0..8].copy_from_slice(b"metadata");
    region[10..12].copy_from_slice(&3u16.to_le_bytes());
    const FP_OFF: u32 = 0x200;
    const VDS_OFF: u32 = 0x210;
    const LSS_OFF: u32 = 0x220;
    region[32..48].copy_from_slice(&GUID_FILE_PARAMETERS);
    region[48..52].copy_from_slice(&FP_OFF.to_le_bytes());
    region[52..56].copy_from_slice(&8u32.to_le_bytes());
    region[64..80].copy_from_slice(&GUID_VIRTUAL_DISK_SIZE);
    region[80..84].copy_from_slice(&VDS_OFF.to_le_bytes());
    region[84..88].copy_from_slice(&8u32.to_le_bytes());
    region[96..112].copy_from_slice(&GUID_LOGICAL_SECTOR_SIZE);
    region[112..116].copy_from_slice(&LSS_OFF.to_le_bytes());
    region[116..120].copy_from_slice(&4u32.to_le_bytes());
    region[FP_OFF as usize..FP_OFF as usize + 4].copy_from_slice(&BLOCK_SIZE.to_le_bytes());
    region[FP_OFF as usize + 4..FP_OFF as usize + 8].fill(0);
    region[VDS_OFF as usize..VDS_OFF as usize + 8]
        .copy_from_slice(&VIRTUAL_DISK_SIZE.to_le_bytes());
    region[LSS_OFF as usize..LSS_OFF as usize + 4].copy_from_slice(&512u32.to_le_bytes());
}

/// Build a 5 MB dirty-log VHDX whose log entry patches `data[DATA_OFFSET]` to
/// `patch_byte`. `descriptor` selects the descriptor kind: `b"desc"` writes a
/// full sector (first byte = `patch_byte`); `b"zero"` zero-fills one sector.
fn build_dirty_log_vhdx(descriptor: [u8; 4], patch_byte: u8, predata: u8) -> Vec<u8> {
    let mut buf = vec![0u8; FILE_SIZE];
    buf[0..8].copy_from_slice(b"vhdxfile");
    write_header(
        &mut buf[0x10000..0x20000],
        1,
        LOG_GUID,
        LOG_OFFSET,
        LOG_LENGTH,
    );
    write_header(&mut buf[0x20000..0x30000], 0, [0u8; 16], 0, 0);
    write_region_table(
        &mut buf[0x30000..0x40000],
        BAT_OFFSET,
        BAT_LENGTH,
        META_OFFSET,
        META_LENGTH,
    );
    write_region_table(
        &mut buf[0x40000..0x50000],
        BAT_OFFSET,
        BAT_LENGTH,
        META_OFFSET,
        META_LENGTH,
    );
    write_metadata(&mut buf[META_OFFSET as usize..(META_OFFSET + u64::from(META_LENGTH)) as usize]);

    let bat_entry: u64 = (DATA_OFFSET >> 20) << 20 | 6;
    buf[BAT_OFFSET as usize..BAT_OFFSET as usize + 8].copy_from_slice(&bat_entry.to_le_bytes());

    // Pre-replay data block content (so `zero` descriptors are observable).
    buf[DATA_OFFSET as usize..DATA_OFFSET as usize + BLOCK_SIZE as usize].fill(predata);

    const ENTRY_LEN: usize = 64 + 32 + 4096;
    let log_base = LOG_OFFSET as usize;
    {
        let e = &mut buf[log_base..log_base + ENTRY_LEN];
        e[0..4].copy_from_slice(b"loge");
        e[8..12].copy_from_slice(&(ENTRY_LEN as u32).to_le_bytes());
        e[16..24].copy_from_slice(&1u64.to_le_bytes());
        e[24..28].copy_from_slice(&1u32.to_le_bytes());
        e[32..48].copy_from_slice(&LOG_GUID);
        let file_size = FILE_SIZE as u64;
        e[48..56].copy_from_slice(&file_size.to_le_bytes());
        e[56..64].copy_from_slice(&file_size.to_le_bytes());
        e[64..68].copy_from_slice(&descriptor);
        if &descriptor == b"zero" {
            // Zero descriptor: ZeroLength @8, FileOffset @16 (one sector).
            e[64 + 8..64 + 16].copy_from_slice(&4096u64.to_le_bytes());
            e[64 + 16..64 + 24].copy_from_slice(&DATA_OFFSET.to_le_bytes());
        } else {
            // Data descriptor: FileOffset @24; payload sector at entry+96.
            e[64 + 16..64 + 24].copy_from_slice(&1u64.to_le_bytes());
            e[64 + 24..64 + 32].copy_from_slice(&DATA_OFFSET.to_le_bytes());
            e[96] = patch_byte;
        }
        let c = crc32c(e);
        e[4..8].copy_from_slice(&c.to_le_bytes());
    }
    buf
}

fn assert_dirty_open_matches_from_bytes(image: Vec<u8>) {
    let tmp = tempfile::NamedTempFile::new().expect("tmpfile");
    std::fs::write(tmp.path(), &image).expect("write dirty image");

    let mut oracle = VhdxReader::from_bytes(image).expect("from_bytes dirty");
    let size = oracle.virtual_disk_size() as usize;
    let want = read_full(&mut oracle, size);

    let mut bounded = VhdxReader::open(tmp.path()).expect("open dirty");
    let got = read_full(&mut bounded, size);

    assert!(
        want == got,
        "dirty-image overlay (open) must match in-place replay (from_bytes)"
    );
    // The first virtual byte reflects the log replay (not the pre-replay value).
    assert_eq!(want[0], got[0], "patched byte must agree");
}

#[test]
fn dirty_data_descriptor_open_matches_from_bytes() {
    // Pre-replay byte 0x00, log writes 0xAB.
    let img = build_dirty_log_vhdx(*b"desc", 0xAB, 0x00);

    // The bounded overlay path must read back the replayed value (0xAB), not the
    // pre-replay 0x00 that sits on disk in the data block.
    let tmp = tempfile::NamedTempFile::new().expect("tmpfile");
    std::fs::write(tmp.path(), &img).expect("write");
    let mut r = VhdxReader::open(tmp.path()).expect("open dirty");
    let mut b = [0u8; 1];
    r.read_exact(&mut b).expect("read byte 0");
    assert_eq!(b[0], 0xAB, "data descriptor must replay 0xAB via overlay");

    assert_dirty_open_matches_from_bytes(img);
}

#[test]
fn dirty_zero_descriptor_open_matches_from_bytes() {
    // Pre-replay byte 0xFF, log zero-fills the sector.
    let img = build_dirty_log_vhdx(*b"zero", 0, 0xFF);
    assert_dirty_open_matches_from_bytes(img);
}
