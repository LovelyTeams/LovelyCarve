//! Concurrency contract for the EWF reader (intra-image parallelism).
//!
//! The reader must serve **positioned reads** through a shared `&self` so many
//! threads can decompress different chunks at once — the enabler for parallel
//! full-image hashing/verify/carve across the fleet. This test is the
//! differential oracle: every thread's positioned read must be byte-identical
//! to the single-threaded `Read`+`Seek` baseline. It fails to compile until
//! `EwfReader::read_at(&self, ..)` exists and the reader is `Sync`.

use std::io::Read;
use std::path::PathBuf;

use ewf::EwfReader;

fn data(name: &str) -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data")).join(name)
}

/// Many threads each read a disjoint region via the shared `&self` positioned
/// API; every region must equal the serial baseline.
#[test]
fn concurrent_positioned_reads_match_serial() {
    let path = data("nps-2010-emails.E01");

    // Serial baseline: whole image via the cursor-based Read+Seek path.
    let mut serial = EwfReader::open(&path).expect("open serial");
    let size = serial.total_size() as usize;
    let mut baseline = vec![0u8; size];
    serial.read_exact(&mut baseline).expect("serial read_exact");

    // Concurrent: scoped threads share `&reader` and each reads its own slice.
    let reader = EwfReader::open(&path).expect("open concurrent");
    let nthreads = 8usize;
    let block = size.div_ceil(nthreads);

    std::thread::scope(|s| {
        let baseline = &baseline;
        let reader = &reader; // requires EwfReader: Sync
        for t in 0..nthreads {
            s.spawn(move || {
                let start = t * block;
                if start >= size {
                    return;
                }
                let len = block.min(size - start);
                let mut buf = vec![0u8; len];
                // The new shared-borrow positioned read.
                let n = reader.read_at(&mut buf, start as u64).expect("read_at");
                assert_eq!(n, len, "thread {t}: short read");
                assert_eq!(
                    &buf[..],
                    &baseline[start..start + len],
                    "thread {t}: concurrent read differs from serial baseline"
                );
            });
        }
    });
}

/// Hammer a SINGLE hot chunk from many threads to exercise the shared cache
/// path (every thread reads the same first 4 KiB) — must stay byte-stable.
#[test]
fn concurrent_reads_of_same_chunk_are_stable() {
    let path = data("nps-2010-emails.E01");

    let mut serial = EwfReader::open(&path).expect("open serial");
    let want_len = 4096usize.min(serial.total_size() as usize);
    let mut want = vec![0u8; want_len];
    serial.read_exact(&mut want).expect("serial read");

    let reader = EwfReader::open(&path).expect("open concurrent");
    std::thread::scope(|s| {
        let want = &want;
        let reader = &reader;
        for _ in 0..16 {
            s.spawn(move || {
                let mut buf = vec![0u8; want_len];
                let n = reader.read_at(&mut buf, 0).expect("read_at");
                assert_eq!(n, want_len);
                assert_eq!(&buf[..], &want[..], "same-chunk concurrent read diverged");
            });
        }
    });
}

/// `verify()` must run through a shared `&self` so it can decompress chunks in
/// PARALLEL (MD5/SHA1 are serial, but zlib decompression — the CPU cost — is
/// not). A non-mut binding forces the `&self` signature; the computed MD5 must
/// still match the stored hash, i.e. parallel decompression reproduces the
/// exact serial byte stream. exfat1.E01 ships a stored MD5.
#[test]
fn verify_runs_on_shared_ref_and_matches_stored_md5() {
    let reader = EwfReader::open(data("exfat1.E01")).expect("open");
    let result = reader.verify().expect("verify");
    assert_eq!(
        result.md5_match,
        Some(true),
        "parallel verify's computed MD5 must match the stored MD5"
    );
}
