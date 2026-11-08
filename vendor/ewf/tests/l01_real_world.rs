//! Real-world L01 validation, env-gated.
//!
//! Skips cleanly unless `EWF_L01_PATH` points at the first segment of a real
//! EnCase Logical Evidence File. No case identifiers, paths or expected values
//! live in this repository: the assertions are structural invariants that must
//! hold for ANY L01, so the test is meaningful on whatever file is supplied.
//!
//! This is the tier that matters. Every committed fixture here is hand-built
//! from the specification (Tier 3) and therefore confirms our reading of the
//! document, not EnCase's behaviour.
//!
//! Why it exists at all: libewf 20231119 cannot open the file this was
//! developed against — it rejects a short-name size that merely restates a
//! name's length, and aborts the whole acquisition over a redundant alias.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

fn l01_path() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var_os("EWF_L01_PATH")?);
    p.is_file().then_some(p)
}

macro_rules! require_l01 {
    () => {
        match l01_path() {
            Some(p) => p,
            None => {
                eprintln!("skipping: set EWF_L01_PATH to a real .L01 first segment");
                return;
            }
        }
    };
}

/// The container must be recognised as LOGICAL, not physical.
#[test]
fn a_real_l01_is_recognised_as_logical() {
    let path = require_l01!();
    let mut f = std::fs::File::open(&path).expect("open");
    let mut hdr = [0u8; ewf::sections::FILE_HEADER_SIZE];
    std::io::Read::read_exact(&mut f, &mut hdr).expect("read header");

    let h = ewf::sections::EwfFileHeader::parse(&hdr).expect("a real L01 header must parse");
    assert_eq!(
        h.kind,
        ewf::sections::EwfKind::Logical,
        "an LVF signature must be reported as a logical container"
    );
    assert!(h.segment_number >= 1, "segment numbering is 1-based");
}

/// The ltree must verify against its own stored MD5, and its entry tree must
/// parse with every entry carrying the declared number of fields.
///
/// Structural only — no count, name or hash from the evidence is pinned here,
/// so the test states nothing about any particular case.
#[test]
fn a_real_l01_ltree_verifies_and_parses() {
    let path = require_l01!();
    let (header, body) = ewf::logical::find_ltree(&path).expect("an L01 must carry an ltree");

    use md5::{Digest as _, Md5};
    assert_eq!(
        header.data_md5,
        <[u8; 16]>::from(Md5::digest(&body)),
        "the ltree must match the MD5 stored in its own header"
    );
    assert_eq!(
        header.data_size as usize,
        body.len(),
        "and the declared data size must match what was read"
    );

    let (text, replaced) = ewf::logical::decode_ltree_text(&body);
    let tree = ewf::logical::parse_entry_tree(&text).expect("the entry tree must parse");

    assert!(
        !tree.entries.is_empty(),
        "a logical acquisition has entries"
    );
    assert!(
        tree.indicators.len() >= 5,
        "the file declares its own field layout: {} indicators",
        tree.indicators.len()
    );
    let malformed = tree
        .warnings
        .iter()
        .filter(|w| w.contains("values for"))
        .count();
    assert_eq!(
        malformed, 0,
        "no entry may disagree with the declared indicator count"
    );

    // Reported for the record; neither is a failure.
    eprintln!(
        "  entries={} dirs={} indicators={} surrogate_replacements={} warnings={}",
        tree.entries.len(),
        tree.entries.iter().filter(|e| e.is_dir).count(),
        tree.indicators.len(),
        replaced,
        tree.warnings.len()
    );
}

/// Every non-directory entry must be accounted for: a name, and a size the
/// tree agrees on. An examiner cannot report on files the reader silently drops.
#[test]
fn a_real_l01_accounts_for_every_file() {
    let path = require_l01!();
    let (_, body) = ewf::logical::find_ltree(&path).expect("ltree");
    let (text, _) = ewf::logical::decode_ltree_text(&body);
    let tree = ewf::logical::parse_entry_tree(&text).expect("parses");

    let files: Vec<_> = tree.entries.iter().filter(|e| !e.is_dir).collect();
    assert!(!files.is_empty(), "a logical acquisition contains files");
    assert!(
        files.iter().all(|e| !e.name.is_empty()),
        "every file entry must have a name"
    );
    // Parent/child links must be consistent in both directions, or the tree
    // reported to an examiner is not the tree in the evidence.
    for (i, e) in tree.entries.iter().enumerate() {
        for &c in &e.children {
            assert_eq!(
                tree.entries[c].parent,
                Some(i),
                "child {c} must point back at parent {i}"
            );
        }
    }
}

/// The strongest available check: bytes recovered by OUR extent reader must
/// hash to the MD5 **EnCase recorded at acquisition**.
///
/// That hash is an independent oracle in the strict sense — written by another
/// vendor's tool, from the original file, before this code existed. It cannot
/// be satisfied by a reader that is internally consistent but wrong: a wrong
/// offset, a wrong length, an unhandled sparse extent or a missing truncation
/// all change the digest.
///
/// A sample is taken rather than the whole set, so the test stays a test. The
/// sample is deterministic (every Nth file), not random, so a failure is
/// reproducible.
#[test]
fn recovered_bytes_match_the_acquisition_md5() {
    let path = require_l01!();
    let (_, body) = ewf::logical::find_ltree(&path).expect("ltree");
    let (text, _) = ewf::logical::decode_ltree_text(&body);
    let tree = ewf::logical::parse_entry_tree(&text).expect("parses");

    let mut media = ewf::EwfReader::open(&path).expect("the media stream must open");

    let candidates: Vec<usize> = tree
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.is_dir && e.md5.is_some() && e.size > 0)
        .map(|(i, _)| i)
        .collect();
    assert!(
        !candidates.is_empty(),
        "the acquisition must contain hashed files to check against"
    );

    // Sample size is tunable so the committed test stays fast while a full
    // sweep of an exhibit is one environment variable away.
    let want: usize = std::env::var("EWF_L01_SAMPLE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let sample: Vec<usize> = candidates
        .iter()
        .copied()
        .step_by((candidates.len() / want.max(1)).max(1))
        .take(want)
        .collect();

    use md5::{Digest as _, Md5};
    let mut checked = 0usize;
    let mut mismatches = Vec::new();
    for i in sample {
        let e = &tree.entries[i];
        let data = match tree.read_entry(&mut media, i) {
            Ok(d) => d,
            Err(err) => {
                mismatches.push(format!("entry {i}: read failed: {err}"));
                continue;
            }
        };
        let got = format!("{:x}", Md5::digest(&data));
        let want = e.md5.clone().unwrap_or_default().to_ascii_lowercase();
        if got != want {
            mismatches.push(format!(
                "entry {i}: size {} declared, {} recovered",
                e.size,
                data.len()
            ));
        }
        checked += 1;
    }
    eprintln!("  verified {checked} files against their acquisition MD5");
    assert!(
        mismatches.is_empty(),
        "{} of {checked} files did not match the MD5 EnCase recorded:\n{}",
        mismatches.len(),
        mismatches
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
