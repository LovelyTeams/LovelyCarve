//! EnCase segment extensions roll over from digits to letters, and a reader
//! that stops at `99` silently loses the rest of the evidence.
//!
//! The sequence is `x01`…`x99`, then `xAA`, `xAB`, … `xAZ`, `xBA`, … `xZZ`.
//! It is not hypothetical: a real 478-segment acquisition on this machine runs
//! `E01`…`E99` then `EAA`…`EOO` — 99 numeric and **379** alpha segments, so a
//! reader capped at 99 would address 21% of the set and report the rest as
//! absent rather than as an error.
//!
//! These tests use `L` (logical) names because `segment_paths` serves the
//! logical reader, but the scheme is identical for `E`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;

/// Build an empty file so `is_file()` sees it.
fn touch(dir: &std::path::Path, name: &str) {
    fs::write(dir.join(name), b"").unwrap();
}

/// A set that crosses the digit → letter boundary must be enumerated whole.
#[test]
fn segment_enumeration_crosses_the_alpha_rollover() {
    let dir = std::env::temp_dir().join(format!("ewf-rollover-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    // L01..L99 then LAA..LAC — 102 segments, three of them past the rollover.
    for n in 1..=99 {
        touch(&dir, &format!("ev.L{n:02}"));
    }
    for s in ["LAA", "LAB", "LAC"] {
        touch(&dir, &format!("ev.{s}"));
    }

    let first = dir.join("ev.L01");
    let found = ewf::logical::segment_paths_for_test(&first);

    assert_eq!(
        found.len(),
        102,
        "99 numeric + 3 alpha segments must all be found; got {} -- a cap at 99 \
         drops the alpha tail silently",
        found.len()
    );
    let names: Vec<String> = found
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    for want in ["ev.L01", "ev.L99", "ev.LAA", "ev.LAC"] {
        assert!(names.contains(&want.to_string()), "missing {want}");
    }

    let _ = fs::remove_dir_all(&dir);
}

/// Enumeration stops at the first gap rather than skipping over it.
///
/// A missing middle segment means an INCOMPLETE set. Silently jumping the hole
/// and carrying on would assemble evidence from a discontiguous run and report
/// nothing wrong.
#[test]
fn enumeration_stops_at_a_missing_segment() {
    let dir = std::env::temp_dir().join(format!("ewf-gap-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    for n in [1u32, 2, 3, 5] {
        touch(&dir, &format!("ev.L{n:02}"));
    }

    let found = ewf::logical::segment_paths_for_test(&dir.join("ev.L01"));
    assert_eq!(
        found.len(),
        3,
        "L04 is missing, so enumeration must stop after L03 rather than silently \
         including L05 and presenting a discontiguous set as whole"
    );

    let _ = fs::remove_dir_all(&dir);
}
