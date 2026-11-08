//! Hard correctness gate for the `SegmentSource` abstraction.
//!
//! `open(path)` (loose `File` segments) is the oracle — it is already validated
//! byte-for-byte against libewf's `ewfexport`. This proves that opening the SAME
//! image bytes through the other two source kinds yields a byte-identical reader.
//!
//! `Mem`: each segment file slurped into a `Vec<u8>` -> `SegmentSource::Mem`.
//! `Sub`: each segment's bytes embedded at a NON-ZERO offset inside a larger temp
//! file, addressed via `SegmentSource::sub(base, len)` (proves the offset math).
//!
//! Each is run through BOTH the eager (`open_from_sources`) and lazy
//! (`open_lazy_from_sources`) constructors. Segment-number reordering of the
//! provided sources is exercised on the real multi-segment image.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use ewf::{EwfReader, SegmentSource};

const DATA_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");

/// Deterministic xorshift PRNG (no external dep) for reproducible random reads.
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

/// Discover the segment files for a first-segment path, in extension order.
fn segment_paths(first: &str) -> Vec<std::path::PathBuf> {
    let p = Path::new(first);
    let stem = p.file_stem().unwrap().to_str().unwrap();
    let dir = p.parent().unwrap();
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|q| {
            q.file_stem().and_then(|s| s.to_str()) == Some(stem)
                && q.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.len() == 3 && e.starts_with(['E', 'L']))
        })
        .collect();
    v.sort();
    v
}

/// Read each segment fully into RAM as a `SegmentSource::Mem`.
fn mem_sources(paths: &[std::path::PathBuf]) -> Vec<SegmentSource> {
    paths
        .iter()
        .map(|p| SegmentSource::from_bytes(std::fs::read(p).unwrap()))
        .collect()
}

/// Embed each segment's bytes at a non-zero offset inside its OWN larger temp
/// file and address it via `Sub { base, len }`. A distinct, non-trivial pad per
/// segment proves the base offset is honoured (not silently treated as 0).
fn sub_sources(paths: &[std::path::PathBuf]) -> (Vec<tempfile::NamedTempFile>, Vec<SegmentSource>) {
    let mut keepalive = Vec::new();
    let mut srcs = Vec::new();
    for (i, p) in paths.iter().enumerate() {
        let bytes = std::fs::read(p).unwrap();
        let base = 0x1000u64 + (i as u64) * 0x37; // distinct non-zero base per segment
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(&vec![0xCDu8; base as usize]).unwrap(); // leading pad
        tmp.write_all(&bytes).unwrap();
        tmp.write_all(&[0xABu8; 64]).unwrap(); // trailing pad (must not be read)
        tmp.flush().unwrap();
        let file = Arc::new(File::open(tmp.path()).unwrap());
        srcs.push(SegmentSource::sub(file, base, bytes.len() as u64));
        keepalive.push(tmp);
    }
    (keepalive, srcs)
}

fn assert_identical(reference: &EwfReader, other: &EwfReader, label: &str) {
    assert_eq!(
        reference.chunk_count(),
        other.chunk_count(),
        "chunk_count mismatch ({label})"
    );
    assert_eq!(
        reference.total_size(),
        other.total_size(),
        "total_size mismatch ({label})"
    );

    let size = reference.total_size();

    // Full sequential sweep in 1 MiB windows.
    let step = 1024 * 1024usize;
    let mut a = vec![0u8; step];
    let mut b = vec![0u8; step];
    let mut off = 0u64;
    while off < size {
        let want = step.min((size - off) as usize);
        let na = reference.read_at(&mut a[..want], off).unwrap();
        let nb = other.read_at(&mut b[..want], off).unwrap();
        assert_eq!(
            na, nb,
            "seq short-read length differs at {off:#x} ({label})"
        );
        assert_eq!(a[..na], b[..nb], "seq byte mismatch at {off:#x} ({label})");
        off += want as u64;
    }

    // 400 random-offset reads of assorted lengths.
    let mut rng = Rng(0xDEAD_BEEF_F00D_1234 ^ size);
    let mut ra = vec![0u8; 256 * 1024];
    let mut rb = vec![0u8; 256 * 1024];
    for _ in 0..400 {
        if size == 0 {
            break;
        }
        let o = rng.next_u64() % size;
        let max_len = (256 * 1024u64).min(size - o) as usize;
        let len = if max_len == 0 {
            1
        } else {
            1 + (rng.next_u64() as usize % max_len)
        };
        let na = reference.read_at(&mut ra[..len], o).unwrap();
        let nb = other.read_at(&mut rb[..len], o).unwrap();
        assert_eq!(na, nb, "rnd short-read length differs at {o:#x} ({label})");
        assert_eq!(ra[..na], rb[..nb], "rnd byte mismatch at {o:#x} ({label})");
    }
}

fn run_for_image(first: &str) {
    let path = format!("{DATA_DIR}/{first}");
    if !Path::new(&path).exists() {
        eprintln!("skip {first}: corpus image absent");
        return;
    }
    let paths = segment_paths(&path);
    assert!(!paths.is_empty(), "no segments found for {first}");

    let reference = EwfReader::open(&path).expect("open(path) oracle");

    // Mem, eager + lazy.
    let eager_mem =
        EwfReader::open_from_sources(mem_sources(&paths)).expect("open_from_sources Mem");
    assert_identical(&reference, &eager_mem, "Mem/eager");
    let lazy_mem =
        EwfReader::open_lazy_from_sources(mem_sources(&paths)).expect("open_lazy_from_sources Mem");
    assert!(lazy_mem.is_lazy());
    assert_identical(&reference, &lazy_mem, "Mem/lazy");

    // Sub, eager + lazy (keepalive holds the temp files open).
    let (_ka1, sub1) = sub_sources(&paths);
    let eager_sub = EwfReader::open_from_sources(sub1).expect("open_from_sources Sub");
    assert_identical(&reference, &eager_sub, "Sub/eager");
    let (_ka2, sub2) = sub_sources(&paths);
    let lazy_sub = EwfReader::open_lazy_from_sources(sub2).expect("open_lazy_from_sources Sub");
    assert!(lazy_sub.is_lazy());
    assert_identical(&reference, &lazy_sub, "Sub/lazy");
}

#[test]
fn open_from_memory_matches_open_from_path() {
    // Single-segment images of varied provenance plus a real multi-segment image
    // (.E01..E08) — the latter exercises Sub/Mem segment-number reordering.
    for name in [
        "exfat1.E01",
        "imageformat_mmls_1.E01",
        "nps-2010-emails.E01",
        "multiseg_v1.E01",
    ] {
        run_for_image(name);
    }
}

#[test]
fn sources_are_reordered_by_segment_number() {
    // Provide the multi-segment image's Mem sources in REVERSE order; the reader
    // must reorder by parsed EWF segment_number and still read byte-identically.
    let path = format!("{DATA_DIR}/multiseg_v1.E01");
    if !Path::new(&path).exists() {
        eprintln!("skip: multiseg_v1.E01 absent");
        return;
    }
    let mut paths = segment_paths(&path);
    let reference = EwfReader::open(&path).expect("oracle");

    paths.reverse();
    let shuffled = mem_sources(&paths);
    let reordered = EwfReader::open_from_sources(shuffled).expect("open_from_sources reversed");
    assert_identical(&reference, &reordered, "reversed Mem");
}

#[test]
fn ewf2_source_is_rejected_cleanly() {
    // An Ex01 (EWF2) image fed as a SegmentSource must fail loud, not silently
    // misparse — the v2 path is path/File-only for now.
    let path = format!("{DATA_DIR}/zeros_128s.Ex01");
    if !Path::new(&path).exists() {
        eprintln!("skip: zeros_128s.Ex01 absent");
        return;
    }
    let bytes = std::fs::read(&path).unwrap();
    let err = EwfReader::open_from_sources(vec![SegmentSource::from_bytes(bytes)]);
    assert!(err.is_err(), "EWF2 source must be rejected, not misparsed");
}
