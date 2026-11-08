//! Rollback-journal (`-journal`) parser + checksum unit vectors.
//!
//! The journal format is the temporal inverse of the WAL: each segment is a
//! sector-padded header followed by `[pgno BE u32][page image][checksum BE u32]`
//! records carrying the page's PRE-transaction bytes. Verified against SQLite's
//! `pager.c` (`writeJournalHdr`/`readJournalHdr`): header fields are magic@0,
//! nRec@8, nonce/cksumInit@12, mxPage/dbOrigSize@16, sectorSize@20, pageSize@24.
//!
//! These are crafted known-answer vectors (Tier A, valid header) and the Tier-B
//! zeroed-header reconstruction path, plus the robustness negatives the Paranoid
//! Gate's threat model requires (truncated tail, `nRec=0xFFFFFFFF` to-EOF,
//! `nRec=0` anomalous, duplicate `pgno`, garbage). The checksum vector is seeded
//! from the offset-12 nonce and the every-200th-byte sample, so a bit-split or
//! offset bug fails here rather than shipping green.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::pedantic)]

use sqlite_core::{JournalHeader, RollbackJournal};

const MAGIC: [u8; 8] = [0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7];

/// Deterministic, reproducible page-image bytes for the crafted vectors.
fn page_bytes(page_size: usize) -> Vec<u8> {
    (0..page_size).map(|i| ((i * 7 + 3) & 0xff) as u8).collect()
}

/// The journal page checksum: `nonce` plus every-200th byte from the tail,
/// starting at `page_size - 200` and stepping down by 200 while the index is
/// positive (wrapping u32). Mirrors `pager.c`'s `pager_cksum`.
fn cksum(nonce: u32, page: &[u8]) -> u32 {
    let mut sum = nonce;
    let mut x = page.len() as i64 - 200;
    while x > 0 {
        sum = sum.wrapping_add(u32::from(page[x as usize]));
        x -= 200;
    }
    sum
}

/// Build one sector-padded header (Tier A, valid magic).
fn header(n_rec: u32, nonce: u32, mx_page: u32, sector: u32, page_size: u32) -> Vec<u8> {
    let mut h = Vec::new();
    h.extend_from_slice(&MAGIC);
    h.extend_from_slice(&n_rec.to_be_bytes());
    h.extend_from_slice(&nonce.to_be_bytes());
    h.extend_from_slice(&mx_page.to_be_bytes());
    h.extend_from_slice(&sector.to_be_bytes());
    h.extend_from_slice(&page_size.to_be_bytes());
    h.resize(sector as usize, 0);
    h
}

/// One page record: `[pgno BE u32][page image][checksum BE u32]`.
fn record(pgno: u32, page: &[u8], nonce: u32) -> Vec<u8> {
    let mut r = Vec::new();
    r.extend_from_slice(&pgno.to_be_bytes());
    r.extend_from_slice(page);
    r.extend_from_slice(&cksum(nonce, page).to_be_bytes());
    r
}

#[test]
fn parses_valid_header_and_checksum() {
    let ps = 512u32;
    let nonce = 0x1234_5678u32;
    let page = page_bytes(ps as usize);
    let mut j = header(1, nonce, 3, 512, ps);
    j.extend_from_slice(&record(2, &page, nonce));

    let parsed = RollbackJournal::parse(&j, ps).expect("valid journal parses");

    match parsed.header() {
        JournalHeader::Valid {
            n_rec,
            mx_page,
            nonce: hn,
            sector_size,
            page_size,
        } => {
            assert_eq!(*n_rec, 1);
            assert_eq!(*mx_page, 3);
            assert_eq!(*hn, nonce);
            assert_eq!(*sector_size, 512);
            assert_eq!(*page_size, ps);
        }
        other => panic!("expected Valid header, got {other:?}"),
    }

    let images = parsed.page_images();
    assert_eq!(images.len(), 1, "one page record");
    assert_eq!(images[0].pgno, 2);
    assert_eq!(images[0].bytes, page);
    assert_eq!(
        images[0].checksum_valid,
        Some(true),
        "Tier-A checksum verifies against the offset-12 nonce"
    );
}

#[test]
fn detects_checksum_mismatch_as_low_confidence_not_dropped() {
    let ps = 512u32;
    let nonce = 0x1234_5678u32;
    let page = page_bytes(ps as usize);
    let mut j = header(1, nonce, 3, 512, ps);
    let mut rec = record(2, &page, nonce);
    // Corrupt one of the sampled bytes (index 312 is sampled for ps=512), so the
    // stored checksum no longer matches — a torn/tampered page.
    let sampled = 4 + 312; // pgno(4) + page offset 312
    rec[sampled] ^= 0xff;
    j.extend_from_slice(&rec);

    let parsed = RollbackJournal::parse(&j, ps).expect("parse keeps mismatched record");
    let images = parsed.page_images();
    assert_eq!(images.len(), 1, "mismatched record is KEPT, not discarded");
    assert_eq!(
        images[0].checksum_valid,
        Some(false),
        "a checksum mismatch is recorded, not silently dropped (Tier A)"
    );
}

#[test]
fn nrec_ffffffff_reads_to_eof() {
    let ps = 512u32;
    let nonce = 0xCAFEu32;
    let page = page_bytes(ps as usize);
    // nRec sentinel 0xFFFFFFFF => read records until EOF (here two records).
    let mut j = header(0xFFFF_FFFF, nonce, 5, 512, ps);
    j.extend_from_slice(&record(2, &page, nonce));
    j.extend_from_slice(&record(4, &page, nonce));

    let parsed = RollbackJournal::parse(&j, ps).expect("to-EOF parse");
    let pgnos: Vec<u32> = parsed.page_images().iter().map(|i| i.pgno).collect();
    assert_eq!(pgnos, vec![2, 4], "0xFFFFFFFF nRec walks to EOF");
}

#[test]
fn nrec_zero_walks_until_invalid_not_zero_records() {
    let ps = 512u32;
    let nonce = 7u32;
    let page = page_bytes(ps as usize);
    // nRec == 0 is anomalous: do NOT treat as "zero records"; walk to EOF.
    let mut j = header(0, nonce, 5, 512, ps);
    j.extend_from_slice(&record(2, &page, nonce));

    let parsed = RollbackJournal::parse(&j, ps).expect("nRec=0 anomalous parse");
    assert_eq!(
        parsed.page_images().len(),
        1,
        "nRec=0 is anomalous → walk to EOF, recovering the present record"
    );
}

#[test]
fn truncated_final_record_is_dropped_not_panicked() {
    let ps = 512u32;
    let nonce = 9u32;
    let page = page_bytes(ps as usize);
    let mut j = header(2, nonce, 5, 512, ps);
    j.extend_from_slice(&record(2, &page, nonce));
    // Second record truncated mid-page (only a partial tail present).
    j.extend_from_slice(&3u32.to_be_bytes());
    j.extend_from_slice(&page[..100]);

    let parsed = RollbackJournal::parse(&j, ps).expect("truncated tail does not panic");
    assert_eq!(
        parsed.page_images().len(),
        1,
        "the complete first record survives; the truncated tail is dropped"
    );
}

#[test]
fn zeroed_header_reconstructs_from_external_page_size() {
    // PERSIST post-commit: the first sector is zeroed (magic gone). The page size
    // is supplied externally (from the main db). Sector size is unknown, so the
    // parser tries candidates and accepts the records.
    let ps = 512u32;
    let page = page_bytes(ps as usize);
    let mut j = vec![0u8; 512]; // zeroed first sector
                                // Records carry a zeroed-checksum tail (nonce gone); use 0 here.
    j.extend_from_slice(&record(2, &page, 0));
    j.extend_from_slice(&record(3, &page, 0));

    let parsed = RollbackJournal::parse(&j, ps).expect("zeroed-header reconstruction");
    match parsed.header() {
        JournalHeader::ReconstructedZeroed {
            page_size,
            sector_size,
        } => {
            assert_eq!(*page_size, ps);
            assert_eq!(*sector_size, 512, "scored 512-sector candidate");
        }
        other => panic!("expected ReconstructedZeroed, got {other:?}"),
    }
    let pgnos: Vec<u32> = parsed.page_images().iter().map(|i| i.pgno).collect();
    assert_eq!(pgnos, vec![2, 3]);
    assert_eq!(
        parsed.page_images()[0].checksum_valid,
        None,
        "checksum is unverifiable when the nonce was zeroed"
    );
}

#[test]
fn duplicate_pgno_keeps_first_and_flags_anomaly() {
    let ps = 512u32;
    let nonce = 11u32;
    let first = page_bytes(ps as usize);
    let mut second = first.clone();
    second[0] ^= 0xff; // a DIFFERENT image for the same pgno
    let mut j = header(2, nonce, 5, 512, ps);
    j.extend_from_slice(&record(2, &first, nonce));
    j.extend_from_slice(&record(2, &second, nonce));

    let parsed = RollbackJournal::parse(&j, ps).expect("duplicate pgno parse");
    let imgs: Vec<&_> = parsed
        .page_images()
        .iter()
        .filter(|i| i.pgno == 2)
        .collect();
    assert_eq!(imgs.len(), 1, "a duplicate pgno is not silently kept twice");
    assert_eq!(
        imgs[0].bytes, first,
        "the FIRST (earliest) image is kept as the truest pre-txn state"
    );
    assert!(
        parsed.has_duplicate_pgno(),
        "a repeated pgno raises the duplicate-page anomaly flag"
    );
}

#[test]
fn duplicate_pgnos_reports_the_repeated_page() {
    // roadmap §2.2: surface the offending VALUE (which page repeated), not just a
    // boolean flag — `from_walk` first-wins-dedups and must retain the dup pgno.
    let ps = 512u32;
    let nonce = 11u32;
    let first = page_bytes(ps as usize);
    let mut second = first.clone();
    second[0] ^= 0xff; // a DIFFERENT image for the same pgno
    let mut j = header(2, nonce, 5, 512, ps);
    j.extend_from_slice(&record(2, &first, nonce));
    j.extend_from_slice(&record(2, &second, nonce));

    let parsed = RollbackJournal::parse(&j, ps).expect("duplicate pgno parse");
    assert_eq!(
        parsed.duplicate_pgnos(),
        [2],
        "the repeated page number itself must be surfaced, not just a boolean"
    );
}

#[test]
fn garbage_yields_empty_not_panic() {
    let ps = 512u32;
    // Non-magic, too short to hold any record at any sector candidate.
    let j = vec![0xABu8; 300];
    let parsed = RollbackJournal::parse(&j, ps).expect("garbage parses to an empty journal");
    assert!(
        parsed.page_images().is_empty(),
        "no records carved from garbage shorter than one record"
    );
}

#[test]
fn rejects_non_power_of_two_page_size() {
    let j = vec![0u8; 2048];
    let err = RollbackJournal::parse(&j, 1000).expect_err("a bad page size is a typed error");
    // The offending value must be surfaced (Show-the-unrecognized-value).
    assert!(
        format!("{err:?}").contains("1000"),
        "the rejected page size is reported in the error: {err:?}"
    );
}
