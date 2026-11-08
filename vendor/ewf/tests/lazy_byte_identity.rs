//! Hard correctness gate for the lazy (paged) chunk table.
//!
//! The eager reader is the oracle: it is already validated byte-for-byte against
//! libewf's `ewfexport` (see `corpus_differential.rs`). This test proves the
//! lazy table is byte-identical to the eager one on REAL corpus images, across:
//!   - `chunk_count()` parity,
//!   - a full sequential sweep of the whole image in 1 MiB steps,
//!   - several hundred random-offset reads.
//!
//! If a corpus image is absent the per-image check skips cleanly (the bytes are
//! gitignored), so CI on a machine without the corpus still builds and runs.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ewf::EwfReader;
use std::path::Path;

const DATA_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");

/// A tiny deterministic xorshift PRNG — no external dep, reproducible offsets.
struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

fn assert_lazy_identical(e01_name: &str) {
    let path = format!("{DATA_DIR}/{e01_name}");
    if !Path::new(&path).exists() {
        eprintln!("skip {e01_name}: corpus image absent");
        return;
    }

    let eager = EwfReader::open(&path).expect("open eager");
    let lazy = EwfReader::open_lazy(&path).expect("open_lazy");

    assert!(lazy.is_lazy(), "open_lazy must build a lazy table");
    assert!(
        !eager.is_lazy(),
        "open must stay eager (zero behavior change)"
    );

    assert_eq!(
        eager.chunk_count(),
        lazy.chunk_count(),
        "chunk_count mismatch for {e01_name}"
    );
    assert_eq!(
        eager.total_size(),
        lazy.total_size(),
        "total_size mismatch for {e01_name}"
    );

    let size = eager.total_size();

    // 1) Full sequential sweep in 1 MiB windows — every byte of the image is
    //    compared between the two readers.
    let step = 1024 * 1024usize;
    let mut offset = 0u64;
    let mut eager_buf = vec![0u8; step];
    let mut lazy_buf = vec![0u8; step];
    while offset < size {
        let want = step.min((size - offset) as usize);
        let ne = eager
            .read_at(&mut eager_buf[..want], offset)
            .expect("eager read_at");
        let nl = lazy
            .read_at(&mut lazy_buf[..want], offset)
            .expect("lazy read_at");
        assert_eq!(
            ne, nl,
            "short-read length differs at {offset:#x} in {e01_name}"
        );
        assert_eq!(
            eager_buf[..ne],
            lazy_buf[..nl],
            "byte mismatch in [{offset:#x}, {:#x}) of {e01_name}",
            offset + ne as u64
        );
        offset += want as u64;
    }

    // 2) Random-offset reads with assorted lengths.
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ size);
    let mut e = vec![0u8; 256 * 1024];
    let mut l = vec![0u8; 256 * 1024];
    for _ in 0..400 {
        if size == 0 {
            break;
        }
        let off = rng.next_u64() % size;
        let max_len = (256 * 1024u64).min(size - off) as usize;
        let len = if max_len == 0 {
            1
        } else {
            1 + (rng.next_u64() as usize % max_len)
        };
        let ne = eager.read_at(&mut e[..len], off).expect("eager rnd");
        let nl = lazy.read_at(&mut l[..len], off).expect("lazy rnd");
        assert_eq!(
            ne, nl,
            "rnd short-read length differs at {off:#x} in {e01_name}"
        );
        assert_eq!(
            e[..ne],
            l[..nl],
            "rnd byte mismatch at {off:#x} (len {len}) in {e01_name}"
        );
    }
}

#[test]
fn lazy_reads_are_byte_identical_to_eager() {
    // Single-segment images of varied provenance (EnCase compressed, FTK
    // uncompressed) plus a real MULTI-SEGMENT image (.E01..E08) that exercises
    // multiple table sections across multiple segment files.
    for name in [
        "exfat1.E01",
        "imageformat_mmls_1.E01",
        "nps-2010-emails.E01",
        "ewfacquire_clean.E01",
        "ctf_file6.E01",
        "gpt_130_partitions.E01",
        "multiseg_v1.E01",
    ] {
        assert_lazy_identical(name);
    }
}

#[test]
fn lazy_chunk_meta_matches_eager_for_every_chunk() {
    // Per-chunk metadata identity: offset, size, compressed flag, segment must
    // match for EVERY chunk, not just the bytes they decode to.
    let path = format!("{DATA_DIR}/multiseg_v1.E01");
    if !Path::new(&path).exists() {
        eprintln!("skip: multiseg_v1.E01 absent");
        return;
    }
    let eager = EwfReader::open(&path).expect("open eager");
    let lazy = EwfReader::open_lazy(&path).expect("open_lazy");
    assert_eq!(eager.chunk_count(), lazy.chunk_count());

    // Reaching across the whole table also forces the lazy cache to evict and
    // re-parse sections, exercising the cache-miss path repeatedly.
    for i in 0..eager.chunk_count() {
        let ce = eager.debug_chunk(i).expect("eager chunk meta");
        let cl = lazy.debug_chunk(i).expect("lazy chunk meta");
        assert_eq!(ce, cl, "chunk meta mismatch at chunk {i}");
    }
}
