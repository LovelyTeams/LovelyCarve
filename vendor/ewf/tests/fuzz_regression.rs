//! Inputs that once panicked the reader, kept as ordinary tests.
//!
//! The fixture is the reproducer libFuzzer actually found, downloaded from the
//! failing `parse_segment` run's artifact rather than hand-authored — so it
//! exercises the path that broke rather than the one I would have guessed.
//!
//! It lives here as well as in the fuzz corpus because a defect found by
//! fuzzing should not need fuzzing to be caught a second time: the fuzz job
//! runs nightly, this suite runs on every push.
//!
//! The assertion is deliberately weak on *what* comes back. A malformed E01 may
//! legitimately open, fail to open, or open and fail later; the contract under
//! test is only that it returns rather than panicking.

use std::io::Write;

use ewf::EwfReader;

/// `parse_segment` reproducer: `chunk_offset + base_offset` overflowed `u64`
/// while building the chunk table.
const PARSE_SEGMENT_CRASH: &[u8] = include_bytes!("data/fuzz-crash-parse_segment-add-overflow.E01");

#[test]
fn parse_segment_reproducer_does_not_panic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("fuzz.E01");
    let mut f = std::fs::File::create(&path).expect("create fixture");
    f.write_all(PARSE_SEGMENT_CRASH).expect("write fixture");
    drop(f);

    let _ = EwfReader::open(&path);
}
