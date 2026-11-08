//! Robustness tests: every Paranoid-Gatekeeper guard must fire (loud error,
//! never a panic or silent wrong output) on crafted malformed input.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    clippy::identity_op
)]

use ad1::testfix;
use ad1::{Ad1Error, Ad1Reader};
use std::path::PathBuf;

const MARGIN: usize = 512;

fn write_one(bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.ad1");
    std::fs::write(&path, bytes).unwrap();
    (dir, path)
}

fn rd_u64(b: &[u8], phys: usize) -> u64 {
    u64::from_le_bytes(b[phys..phys + 8].try_into().unwrap())
}
fn put_u64(b: &mut [u8], phys: usize, v: u64) {
    b[phys..phys + 8].copy_from_slice(&v.to_le_bytes());
}
fn put_u32(b: &mut [u8], phys: usize, v: u32) {
    b[phys..phys + 4].copy_from_slice(&v.to_le_bytes());
}

/// Physical offset of a logical address (single segment).
fn phys(logical: u64) -> usize {
    MARGIN + logical as usize
}

/// Walk the tree to find an item's logical offset by name.
fn item_offset(bytes: &[u8], name: &str) -> u64 {
    let first = rd_u64(bytes, phys(0) + 0x24); // logical header first_item_addr
    let mut stack = vec![first];
    while let Some(addr) = stack.pop() {
        if addr == 0 {
            continue;
        }
        let base = phys(addr);
        let name_len =
            u32::from_le_bytes(bytes[base + 0x2c..base + 0x30].try_into().unwrap()) as usize;
        let nm = String::from_utf8_lossy(&bytes[base + 0x30..base + 0x30 + name_len]);
        if nm == name {
            return addr;
        }
        let next = rd_u64(bytes, base + 0x00);
        let child = rd_u64(bytes, base + 0x08);
        stack.push(next);
        stack.push(child);
    }
    panic!("item {name} not found");
}

#[test]
fn zero_fragments_size_is_rejected() {
    let built = testfix::build(testfix::sample_tree());
    let mut bytes = built.bytes;
    put_u32(&mut bytes, 0x22, 0); // segment header fragments_size
    let (_d, p) = write_one(&bytes);
    assert!(matches!(Ad1Reader::open(&p), Err(Ad1Error::Malformed(_))));
}

#[test]
fn zero_chunk_size_is_rejected() {
    let built = testfix::build(testfix::sample_tree());
    let mut bytes = built.bytes;
    put_u32(&mut bytes, 0x218, 0); // logical header zlib_chunk_size
    let (_d, p) = write_one(&bytes);
    assert!(matches!(Ad1Reader::open(&p), Err(Ad1Error::Malformed(_))));
}

#[test]
fn implausible_chunk_size_is_rejected() {
    let built = testfix::build(testfix::sample_tree());
    let mut bytes = built.bytes;
    put_u32(&mut bytes, 0x218, u32::MAX); // > MAX_CHUNK_SIZE
    let (_d, p) = write_one(&bytes);
    assert!(matches!(Ad1Reader::open(&p), Err(Ad1Error::Malformed(_))));
}

#[test]
fn truncated_header_is_not_ad1() {
    let (_d, p) = write_one(b"AD"); // far shorter than the marker
    assert!(matches!(Ad1Reader::open(&p), Err(Ad1Error::NotAd1(_))));
}

#[test]
fn oversize_item_name_is_rejected() {
    let built = testfix::build(testfix::sample_tree());
    let off = item_offset(&built.bytes, "hello.txt");
    let mut bytes = built.bytes;
    put_u32(&mut bytes, phys(off) + 0x2c, 100_000); // name_len > MAX_NAME_LEN
    let (_d, p) = write_one(&bytes);
    assert!(matches!(Ad1Reader::open(&p), Err(Ad1Error::Malformed(_))));
}

#[test]
fn tree_cycle_is_detected() {
    let built = testfix::build(testfix::sample_tree());
    let first = rd_u64(&built.bytes, phys(0) + 0x24);
    let mut bytes = built.bytes;
    // Point the root's next-sibling at itself.
    put_u64(&mut bytes, phys(first) + 0x00, first);
    let (_d, p) = write_one(&bytes);
    match Ad1Reader::open(&p) {
        Err(Ad1Error::Malformed(m)) => assert!(m.contains("cycle"), "{m}"),
        other => panic!("expected cycle Malformed, got {other:?}"),
    }
}

#[test]
fn oversize_metadata_payload_is_rejected() {
    let built = testfix::build(testfix::sample_tree());
    let off = item_offset(&built.bytes, "hello.txt");
    let meta_addr = rd_u64(&built.bytes, phys(off) + 0x10);
    let mut bytes = built.bytes;
    // Inflate the first metadata record's data_length beyond MAX_META_DATA.
    put_u32(&mut bytes, phys(meta_addr) + 0x10, 10_000_000);
    let (_d, p) = write_one(&bytes);
    assert!(matches!(Ad1Reader::open(&p), Err(Ad1Error::Malformed(_))));
}

#[test]
fn zero_chunk_count_on_a_sized_file_is_rejected() {
    let built = testfix::build(testfix::sample_tree());
    let off = item_offset(&built.bytes, "a.bin");
    let zlib_addr = rd_u64(&built.bytes, phys(off) + 0x18);
    let mut bytes = built.bytes;
    put_u64(&mut bytes, phys(zlib_addr), 0); // chunk_count = 0
    let (_d, p) = write_one(&bytes);
    let r = Ad1Reader::open(&p).unwrap();
    let entry = r
        .entries()
        .iter()
        .find(|e| e.path == "root/sub/a.bin")
        .unwrap();
    let mut buf = vec![0u8; 64];
    assert!(matches!(
        r.read_at(entry, 0, &mut buf),
        Err(Ad1Error::Malformed(_))
    ));
}

#[test]
fn max_chunk_count_does_not_overflow() {
    // Regression: a fuzzer-found add-overflow — chunk_count = u64::MAX made
    // `count + 1` overflow before the bound check. Must reject, not panic.
    let built = testfix::build(testfix::sample_tree());
    let off = item_offset(&built.bytes, "a.bin");
    let zlib_addr = rd_u64(&built.bytes, phys(off) + 0x18);
    let mut bytes = built.bytes;
    put_u64(&mut bytes, phys(zlib_addr), u64::MAX);
    let (_d, p) = write_one(&bytes);
    let r = Ad1Reader::open(&p).unwrap();
    let entry = r
        .entries()
        .iter()
        .find(|e| e.path == "root/sub/a.bin")
        .unwrap();
    let mut buf = vec![0u8; 64];
    assert!(matches!(
        r.read_at(entry, 0, &mut buf),
        Err(Ad1Error::Malformed(_))
    ));
}

#[test]
fn corrupt_compressed_chunk_errors_on_read() {
    let built = testfix::build(testfix::sample_tree());
    let off = item_offset(&built.bytes, "a.bin");
    let zlib_addr = rd_u64(&built.bytes, phys(off) + 0x18);
    let chunk0 = rd_u64(&built.bytes, phys(zlib_addr) + 0x08); // addresses[0]
    let mut bytes = built.bytes;
    // Destroy the zlib header of chunk 0.
    bytes[phys(chunk0)] = 0xff;
    bytes[phys(chunk0) + 1] = 0xff;
    let (_d, p) = write_one(&bytes);
    let r = Ad1Reader::open(&p).unwrap();
    let entry = r
        .entries()
        .iter()
        .find(|e| e.path == "root/sub/a.bin")
        .unwrap();
    let mut buf = vec![0u8; 1024];
    assert!(r.read_at(entry, 0, &mut buf).is_err());
}

#[test]
fn non_monotonic_chunk_addresses_are_rejected() {
    let built = testfix::build(testfix::sample_tree());
    let off = item_offset(&built.bytes, "a.bin");
    let zlib_addr = rd_u64(&built.bytes, phys(off) + 0x18);
    let mut bytes = built.bytes;
    // addresses[1] < addresses[0] -> negative chunk length.
    put_u64(&mut bytes, phys(zlib_addr) + 0x10, 0);
    let (_d, p) = write_one(&bytes);
    let r = Ad1Reader::open(&p).unwrap();
    let entry = r
        .entries()
        .iter()
        .find(|e| e.path == "root/sub/a.bin")
        .unwrap();
    let mut buf = vec![0u8; 1024];
    assert!(matches!(
        r.read_at(entry, 0, &mut buf),
        Err(Ad1Error::Malformed(_))
    ));
}

#[test]
fn sized_file_without_chunk_table_is_rejected() {
    let built = testfix::build(testfix::sample_tree());
    let off = item_offset(&built.bytes, "a.bin");
    let mut bytes = built.bytes;
    put_u64(&mut bytes, phys(off) + 0x18, 0); // zlib_metadata_addr = 0 but size > 0
    let (_d, p) = write_one(&bytes);
    let r = Ad1Reader::open(&p).unwrap();
    let entry = r
        .entries()
        .iter()
        .find(|e| e.path == "root/sub/a.bin")
        .unwrap();
    let mut buf = vec![0u8; 64];
    assert!(matches!(
        r.read_at(entry, 0, &mut buf),
        Err(Ad1Error::Malformed(_))
    ));
}

#[test]
fn metadata_chain_cycle_is_tolerated() {
    let built = testfix::build(testfix::sample_tree());
    let off = item_offset(&built.bytes, "hello.txt");
    let meta_addr = rd_u64(&built.bytes, phys(off) + 0x10);
    let mut bytes = built.bytes;
    // First metadata record points at itself: the walker must break, not loop.
    put_u64(&mut bytes, phys(meta_addr) + 0x00, meta_addr);
    let (_d, p) = write_one(&bytes);
    // Opens cleanly (a metadata cycle is degraded, not fatal).
    assert!(Ad1Reader::open(&p).is_ok());
}

#[test]
fn chunk_length_exceeding_image_size_is_rejected() {
    let built = testfix::build(testfix::sample_tree());
    let off = item_offset(&built.bytes, "a.bin");
    let zlib_addr = rd_u64(&built.bytes, phys(off) + 0x18);
    let mut bytes = built.bytes;
    // addresses[1] far beyond the image -> chunk length > capacity.
    put_u64(&mut bytes, phys(zlib_addr) + 0x10, u64::MAX / 2);
    let (_d, p) = write_one(&bytes);
    let r = Ad1Reader::open(&p).unwrap();
    let entry = r
        .entries()
        .iter()
        .find(|e| e.path == "root/sub/a.bin")
        .unwrap();
    let mut buf = vec![0u8; 64];
    assert!(matches!(
        r.read_at(entry, 0, &mut buf),
        Err(Ad1Error::Malformed(_))
    ));
}

#[test]
fn absurd_segment_count_is_rejected_not_allocated() {
    // Regression: a fuzzer-found OOM — a ~2.25e9 segment_number drove
    // Vec::with_capacity(segment_count) to a 72 GB allocation. The count must be
    // capped and rejected loud, never pre-allocated.
    let built = testfix::build(testfix::sample_tree());
    let mut bytes = built.bytes;
    put_u32(&mut bytes, 0x1c, 100_000); // segment_number well past the cap
    let (_d, p) = write_one(&bytes);
    assert!(matches!(Ad1Reader::open(&p), Err(Ad1Error::Malformed(_))));
}

#[test]
fn short_non_last_chunk_does_not_panic() {
    // Regression: a fuzzer-found subtract-overflow — a non-last chunk that
    // inflates to fewer than chunk_size bytes leaves a hole, and the next chunk's
    // `cur - chunk_base` underflowed. read_at must stop with a short read.
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use std::io::Write as _;

    let built = testfix::build(testfix::sample_tree());
    let off = item_offset(&built.bytes, "a.bin");
    let zlib_addr = rd_u64(&built.bytes, phys(off) + 0x18);
    let chunk0 = rd_u64(&built.bytes, phys(zlib_addr) + 0x08);
    // A valid zlib stream that inflates to only 16 bytes (« 64 KiB chunk size).
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
    enc.write_all(&[0u8; 16]).unwrap();
    let short = enc.finish().unwrap();

    let mut bytes = built.bytes;
    bytes[phys(chunk0)..phys(chunk0) + short.len()].copy_from_slice(&short);
    let (_d, p) = write_one(&bytes);
    let r = Ad1Reader::open(&p).unwrap();
    let entry = r
        .entries()
        .iter()
        .find(|e| e.path == "root/sub/a.bin")
        .unwrap();
    let mut buf = vec![0u8; entry.size as usize];
    let n = r.read_at(entry, 0, &mut buf).unwrap(); // must not panic
    assert!(
        n < entry.size as usize,
        "short chunk should yield a short read"
    );
}

#[test]
fn read_at_directory_and_empty_file_yield_zero() {
    let built = testfix::build(testfix::sample_tree());
    let (_d, p) = write_one(&built.bytes);
    let r = Ad1Reader::open(&p).unwrap();
    let dir = r.entries().iter().find(|e| e.path == "root/sub").unwrap();
    let empty = r
        .entries()
        .iter()
        .find(|e| e.path == "root/sub/empty.dat")
        .unwrap();
    let mut buf = [0u8; 8];
    assert_eq!(r.read_at(dir, 0, &mut buf).unwrap(), 0);
    assert_eq!(r.read_at(empty, 0, &mut buf).unwrap(), 0);
    assert_eq!(r.read_at(empty, 0, &mut []).unwrap(), 0); // empty buf
}
