//! Tier-1 validation against a real FTK Imager AD1.
//!
//! Env-gated: set `AD1_USERBSS=/path/to/userbss.ad1` (2025 Magnet Summit CTF,
//! NIST CFReDS / Hexordia). FTK wrote the stored per-file MD5/SHA1 — an oracle
//! fully independent of `ad1-core`. This reconciles `ad1-core`'s decompression +
//! recomputed hashes against those stored hashes; agreement across a real image
//! closes the tier-3 residual (shared structural offsets) noted in
//! `docs/validation.md`.
//!
//! `AD1_USERBSS_LIMIT` (optional) caps the number of files checked (smoke runs).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use ad1::{Ad1Entry, Ad1Reader};
use md5::{Digest as _, Md5};
use sha1::Sha1;
use std::path::PathBuf;

fn hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        let _ = write!(s, "{x:02x}");
    }
    s
}

fn recompute(r: &Ad1Reader, e: &Ad1Entry) -> (String, String, u64) {
    let mut md5 = Md5::new();
    let mut sha1 = Sha1::new();
    let mut buf = vec![0u8; r.chunk_size().max(1) as usize];
    let (mut off, mut total) = (0u64, 0u64);
    loop {
        let n = r.read_at(e, off, &mut buf).expect("read_at");
        if n == 0 {
            break;
        }
        md5.update(&buf[..n]);
        sha1.update(&buf[..n]);
        off += n as u64;
        total += n as u64;
    }
    (hex(&md5.finalize()), hex(&sha1.finalize()), total)
}

#[test]
fn reconcile_stored_hashes_against_recomputed() {
    let Ok(path) = std::env::var("AD1_USERBSS") else {
        eprintln!("AD1_USERBSS not set — skipping tier-1 real-data test");
        return;
    };
    let limit = std::env::var("AD1_USERBSS_LIMIT")
        .ok()
        .and_then(|s| s.parse::<usize>().ok());

    let reader = Ad1Reader::open(&PathBuf::from(&path)).expect("open real AD1");
    eprintln!(
        "opened {path}: version={} chunk_size={} segments={} entries={}",
        reader.image_version(),
        reader.chunk_size(),
        reader.segment_count(),
        reader.entries().len()
    );

    let (mut files, mut checked, mut md5_ok, mut sha1_ok, mut mismatches, mut size_lies) =
        (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);

    for entry in reader.entries() {
        if entry.is_dir {
            continue;
        }
        files += 1;
        if entry.md5.is_none() && entry.sha1.is_none() {
            continue;
        }
        if let Some(l) = limit {
            if checked as usize >= l {
                break;
            }
        }
        checked += 1;
        let (md5, sha1, total) = recompute(&reader, entry);
        if total < entry.size {
            size_lies += 1;
            eprintln!("SIZE  {} declared={} got={}", entry.path, entry.size, total);
        }
        let mut bad = false;
        if let Some(stored) = &entry.md5 {
            if stored.eq_ignore_ascii_case(&md5) {
                md5_ok += 1;
            } else {
                bad = true;
                eprintln!("MD5   {} stored={stored} computed={md5}", entry.path);
            }
        }
        if let Some(stored) = &entry.sha1 {
            if stored.eq_ignore_ascii_case(&sha1) {
                sha1_ok += 1;
            } else {
                bad = true;
                eprintln!("SHA1  {} stored={stored} computed={sha1}", entry.path);
            }
        }
        if bad {
            mismatches += 1;
        }
    }

    eprintln!(
        "tier-1: files={files} checked={checked} md5_ok={md5_ok} sha1_ok={sha1_ok} \
         mismatches={mismatches} size_lies={size_lies}"
    );
    assert!(checked > 0, "no hashed files found — parse likely failed");
    assert_eq!(
        mismatches, 0,
        "{mismatches} stored-vs-recomputed hash mismatches"
    );
    assert_eq!(
        size_lies, 0,
        "{size_lies} files decompressed short of declared size"
    );
}
