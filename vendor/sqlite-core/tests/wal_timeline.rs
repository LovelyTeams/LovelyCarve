//! Bespoke, format-exact WAL temporal model (`WalTimeline`) — task #55.
//!
//! Validated against the REAL `wal_carve.db` + `-wal` fixture (`docs/corpus-catalog.md`
//! §J): a `wal_checkpoint(TRUNCATE)` baseline (rows 1..=50, WAL emptied) then —
//! with a held reader blocking checkpoint — an INSERT commit (rows 101..=150) and a
//! DELETE commit (rows 121..=140), **no checkpoint**. The `-wal` therefore holds ONE
//! salt segment with TWO COMMIT frames (both rewriting page 2). That is exactly two
//! materializable commit snapshots in one segment.
//!
//! These tests pin the bespoke model's public contract: enumerate the commit
//! snapshots (each addressable by `CommitId`), materialize each as a read-only replay
//! overlay, diff base-vs-last-commit, and reject a malformed WAL at the right
//! validation tier (page-size mismatch = hard stop).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use sqlite_core::{CohortTopology, Database, MaterializationSafety, WalLsn, WalValidationError};

const CARVE_MAIN: &[u8] = include_bytes!("data/wal_carve.db");
const CARVE_WAL: &[u8] = include_bytes!("data/wal_carve.db-wal");

// --- segment / snapshot enumeration -----------------------------------------

#[test]
fn timeline_enumerates_one_segment_two_commit_snapshots() {
    let db = Database::open_with_wal(CARVE_MAIN.to_vec(), CARVE_WAL).expect("open with wal");
    let tl = db.wal_timeline().expect("timeline present");

    // One salt epoch (no checkpoint reset between the two commits).
    assert_eq!(tl.segments().len(), 1, "single salt segment");
    let seg = &tl.segments()[0];
    assert_eq!(seg.frame_count, 2, "two frames in the segment");
    assert!(
        seg.salt1 != 0 && seg.salt2 != 0,
        "segment carries WAL salts"
    );
    assert_eq!(seg.page_size, db.header().page_size);

    // Two COMMIT frames → two materializable commit snapshots.
    let snaps = tl.commit_snapshots();
    assert_eq!(snaps.len(), 2, "two materializable commit snapshots");

    // CommitId = (segment_id, commit_frame_index, db_size_after_commit). Both
    // commits leave a 2-page database; the commit frame indices are the file-order
    // positions of the two COMMIT frames (0 then 1 here).
    assert_eq!(snaps[0].id().commit_frame_index, 0);
    assert_eq!(snaps[1].id().commit_frame_index, 1);
    assert_eq!(snaps[0].id().db_size_after_commit, 2);
    assert_eq!(snaps[1].id().db_size_after_commit, 2);
    // Both snapshots belong to the one segment.
    assert_eq!(snaps[0].id().segment, seg.id);
    assert_eq!(snaps[1].id().segment, seg.id);
}

#[test]
fn single_segment_topology_is_linear() {
    let db = Database::open_with_wal(CARVE_MAIN.to_vec(), CARVE_WAL).expect("open with wal");
    let tl = db.wal_timeline().expect("timeline present");
    assert_eq!(
        tl.topology(),
        CohortTopology::LinearSegment,
        "one salt epoch with no reset is a linear cohort"
    );
}

// --- materialization (replay overlay) ---------------------------------------

#[test]
fn materialize_last_commit_recovers_surviving_rows_not_deleted_ones() {
    let db = Database::open_with_wal(CARVE_MAIN.to_vec(), CARVE_WAL).expect("open with wal");
    let tl = db.wal_timeline().expect("timeline present");
    let snaps = tl.commit_snapshots();

    // db_size_after_commit pins the database page count at each commit.
    assert_eq!(snaps[1].db_size_after_commit(), 2);

    // The newest commit's page-2 image is the post-DELETE state: it still carries the
    // surviving body text (e.g. row 101) but the freed cells for 121..=140 are gone
    // from the live area (their residue survives only as slack — that is the carver's
    // job, not the consistent snapshot's).
    let page2 = snaps[1]
        .page_version(2)
        .expect("page 2 present at last commit");
    assert_eq!(page2.page_no, 2);
    assert_eq!(page2.bytes.len(), db.header().page_size as usize);
    let surviving = b"secret WAL body 101";
    assert!(
        page2.bytes.windows(surviving.len()).any(|w| w == surviving),
        "surviving row 101 present in the materialized last-commit page"
    );
}

#[test]
fn materialize_first_commit_holds_rows_before_the_delete() {
    let db = Database::open_with_wal(CARVE_MAIN.to_vec(), CARVE_WAL).expect("open with wal");
    let tl = db.wal_timeline().expect("timeline present");
    let snaps = tl.commit_snapshots();

    // At the FIRST commit (the INSERT), a row later DELETEd (e.g. 130) is still live
    // in the page image — the earlier snapshot is a genuinely-different db state.
    let page2 = snaps[0]
        .page_version(2)
        .expect("page 2 present at first commit");
    let deleted_later = b"secret WAL body 130";
    assert!(
        page2
            .bytes
            .windows(deleted_later.len())
            .any(|w| w == deleted_later),
        "row 130 is live at the INSERT commit (deleted only at the second commit)"
    );
}

#[test]
fn snapshot_enumerates_its_materialized_page_numbers() {
    // The carve-at-snapshot primitive (task #63) needs to iterate a snapshot's
    // page images WITHOUT assuming the page numbers are a contiguous 1..=db_size
    // range. `page_numbers()` returns exactly the pages the snapshot materialized
    // (base ∪ committed frames, capped to db_size_after_commit), ascending, and
    // every one must resolve via `page_version`.
    let db = Database::open_with_wal(CARVE_MAIN.to_vec(), CARVE_WAL).expect("open with wal");
    let tl = db.wal_timeline().expect("timeline present");
    let snaps = tl.commit_snapshots();

    for snap in snaps {
        let pages = snap.page_numbers();
        // Ascending, no duplicates.
        assert!(
            pages.windows(2).all(|w| w[0] < w[1]),
            "page numbers ascending & unique: {pages:?}"
        );
        // Every enumerated page resolves to an image.
        for &p in &pages {
            assert!(
                snap.page_version(p).is_some(),
                "page {p} enumerated but page_version returned None"
            );
            assert!(p <= snap.db_size_after_commit(), "page {p} within db size");
        }
    }
    // wal_carve.db: both commits materialize pages 1 and 2 exactly.
    assert_eq!(snaps[0].page_numbers(), vec![1, 2]);
    assert_eq!(snaps[1].page_numbers(), vec![1, 2]);
}

#[test]
fn materialize_by_commit_id_is_stable() {
    let db = Database::open_with_wal(CARVE_MAIN.to_vec(), CARVE_WAL).expect("open with wal");
    let tl = db.wal_timeline().expect("timeline present");
    let snaps = tl.commit_snapshots();
    let id = snaps[0].id();
    // Re-resolving by CommitId yields the same snapshot.
    let resolved = tl.snapshot_at(id).expect("resolve by CommitId");
    assert_eq!(resolved.id(), id);
    assert_eq!(
        resolved.db_size_after_commit(),
        snaps[0].db_size_after_commit()
    );
}

// --- diff: base vs last valid commit ----------------------------------------

#[test]
fn diff_base_vs_last_commit_identifies_changed_pages() {
    let db = Database::open_with_wal(CARVE_MAIN.to_vec(), CARVE_WAL).expect("open with wal");
    let tl = db.wal_timeline().expect("timeline present");

    let diff = tl.diff_base_to_last_commit().expect("diff present");
    // The INSERT/DELETE activity rewrote page 2; the base never held those rows.
    assert!(
        diff.changed_pages().contains(&2),
        "page 2 changed between the acquired base and the last WAL commit"
    );
    // Page 1 (schema/header) was untouched by the row edits.
    assert!(
        !diff.changed_pages().contains(&1),
        "page 1 (schema) is unchanged across the segment"
    );
}

// --- validation tiers --------------------------------------------------------

#[test]
fn valid_wal_clears_all_three_validation_tiers() {
    let db = Database::open_with_wal(CARVE_MAIN.to_vec(), CARVE_WAL).expect("open with wal");
    let tl = db.wal_timeline().expect("timeline present");
    assert_eq!(
        tl.safety(),
        MaterializationSafety::ReplaySafe,
        "a well-formed single-segment WAL is replay-safe"
    );
}

#[test]
fn page_size_mismatch_is_a_hard_stop_at_physical_validation() {
    // A WAL header that declares a different page size than the DB header must be
    // rejected at the PhysicalValidation tier — never silently mis-sliced.
    let mut bad_wal = CARVE_WAL.to_vec();
    // WAL header page-size field is at offset 8 (big-endian u32). The DB is 4096;
    // claim 8192 to force the mismatch.
    bad_wal[8..12].copy_from_slice(&8192u32.to_be_bytes());
    let err = Database::wal_timeline_from(CARVE_MAIN, &bad_wal)
        .expect_err("page-size mismatch must be rejected");
    assert_eq!(
        err,
        WalValidationError::PageSizeMismatch {
            db: 4096,
            wal: 8192
        }
    );
}

#[test]
fn checksum_break_in_header_magic_is_rejected_at_physical_validation() {
    // Corrupt the WAL magic so the header fails the format check.
    let mut bad_wal = CARVE_WAL.to_vec();
    bad_wal[0] ^= 0xFF;
    let err =
        Database::wal_timeline_from(CARVE_MAIN, &bad_wal).expect_err("bad magic must be rejected");
    assert_eq!(err, WalValidationError::BadMagic);
}

#[test]
fn absent_wal_yields_no_timeline_not_an_error() {
    // A main DB opened without a WAL has no timeline (None), never an error.
    let db = Database::open(CARVE_MAIN.to_vec()).expect("open");
    assert!(db.wal_timeline().is_none(), "no WAL → no timeline");
}

// --- [H] adapter seam --------------------------------------------------------

#[test]
fn lsn_seam_exposes_salt_qualified_frame_index() {
    // The future state-history-forensic [H] adapter maps to
    // LsnKind::SqliteWal { salt1, salt2, frame_index } — NEVER a bare frame_index.
    // The bespoke model exposes exactly that triple via WalLsn so the adapter can
    // attach without sqlite-core depending on state-history-forensic.
    let db = Database::open_with_wal(CARVE_MAIN.to_vec(), CARVE_WAL).expect("open with wal");
    let tl = db.wal_timeline().expect("timeline present");
    let snaps = tl.commit_snapshots();

    let lsn: WalLsn = snaps[1].lsn();
    assert_eq!(lsn.frame_index, 1);
    let seg = &tl.segments()[0];
    assert_eq!(lsn.salt1, seg.salt1, "LSN is salt-qualified, not bare");
    assert_eq!(lsn.salt2, seg.salt2);

    // tamper-resistance is LOW: WAL checksums detect corruption, not tampering.
    assert!(
        !tl.checksums_are_tamper_evident(),
        "WAL checksums are non-cryptographic — corruption detection only"
    );
}

// --- synthetic WALs: multi-segment, uncommitted tail, no-commit -------------
//
// The real corpus fixture is single-segment (one salt epoch, two commits). A
// checkpoint-reset across salt epochs is awkward to capture from libsqlite without
// a multi-checkpoint dance, so the cross-segment topology + residue paths are
// exercised with format-exact synthetic WALs built here (the layout is the same one
// the parser reads). The single-segment path is the authoritative real-artifact case
// above; these pin the discontinuity / residue behavior the model must surface.

const PS: usize = 4096;

/// Build a 32-byte WAL header with the given salts (big-endian, magic = BE variant).
fn wal_header(salt1: u32, salt2: u32, ckpt_seq: u32) -> Vec<u8> {
    let mut h = vec![0u8; 32];
    h[0..4].copy_from_slice(&0x377f_0682u32.to_be_bytes());
    h[4..8].copy_from_slice(&3_007_000u32.to_be_bytes());
    h[8..12].copy_from_slice(&(PS as u32).to_be_bytes());
    h[12..16].copy_from_slice(&ckpt_seq.to_be_bytes());
    h[16..20].copy_from_slice(&salt1.to_be_bytes());
    h[20..24].copy_from_slice(&salt2.to_be_bytes());
    h
}

/// Append one frame (24-byte header + `PS`-byte page) for `page_no`, with the given
/// `db_size_after_commit` (0 = non-commit frame) and salts. `marker` is written into
/// the page body so a snapshot can be identified.
fn push_frame(
    buf: &mut Vec<u8>,
    page_no: u32,
    db_size: u32,
    salt1: u32,
    salt2: u32,
    marker: &[u8],
) {
    let mut fh = vec![0u8; 24];
    fh[0..4].copy_from_slice(&page_no.to_be_bytes());
    fh[4..8].copy_from_slice(&db_size.to_be_bytes());
    fh[8..12].copy_from_slice(&salt1.to_be_bytes());
    fh[12..16].copy_from_slice(&salt2.to_be_bytes());
    buf.extend_from_slice(&fh);
    let mut page = vec![0u8; PS];
    page[..marker.len()].copy_from_slice(marker);
    buf.extend_from_slice(&page);
}

/// A minimal valid 2-page main DB (so `wal_timeline_from` parses the header + base).
fn synth_main() -> Vec<u8> {
    let mut db = vec![0u8; PS * 2];
    db[..16].copy_from_slice(b"SQLite format 3\0");
    db[16..18].copy_from_slice(&(PS as u16).to_be_bytes());
    db[28..32].copy_from_slice(&2u32.to_be_bytes()); // in-header page count
    db
}

#[test]
fn salt_reset_opens_a_second_disconnected_segment() {
    let main = synth_main();
    let mut wal = wal_header(0x1111_1111, 0x2222_2222, 1);
    // Segment A: one COMMIT frame for page 2.
    push_frame(&mut wal, 2, 2, 0x1111_1111, 0x2222_2222, b"SEG-A-COMMIT");
    // Segment B: salts rolled (checkpoint reset) — a discontinuity. One COMMIT frame.
    push_frame(&mut wal, 2, 2, 0x3333_3333, 0x4444_4444, b"SEG-B-COMMIT");

    let tl = Database::wal_timeline_from(&main, &wal).expect("timeline");
    assert_eq!(tl.segments().len(), 2, "salt reset opens a second segment");
    assert_eq!(
        tl.topology(),
        CohortTopology::Disconnected,
        "two salt epochs are a disconnected cohort"
    );
    // Each segment contributed one commit snapshot, under its own salts.
    let snaps = tl.commit_snapshots();
    assert_eq!(snaps.len(), 2);
    assert_eq!(snaps[0].id().segment, tl.segments()[0].id);
    assert_eq!(snaps[1].id().segment, tl.segments()[1].id);
    assert_ne!(
        tl.segments()[0].salt1,
        tl.segments()[1].salt1,
        "the second segment is a different checkpoint generation"
    );
    assert_eq!(tl.page_size(), PS as u32);
}

#[test]
fn uncommitted_tail_is_surfaced_as_residue_not_history() {
    let main = synth_main();
    let mut wal = wal_header(0xAAAA_AAAA, 0xBBBB_BBBB, 1);
    // One COMMIT frame, then an uncommitted trailing frame (db_size = 0).
    push_frame(&mut wal, 2, 2, 0xAAAA_AAAA, 0xBBBB_BBBB, b"COMMITTED");
    push_frame(
        &mut wal,
        2,
        0,
        0xAAAA_AAAA,
        0xBBBB_BBBB,
        b"UNCOMMITTED-TAIL",
    );

    let tl = Database::wal_timeline_from(&main, &wal).expect("timeline");
    // Only the committed frame is history; the trailing frame is residue.
    assert_eq!(tl.commit_snapshots().len(), 1, "one commit snapshot");
    let res = tl.residue();
    assert_eq!(res.len(), 1, "the uncommitted tail is one residue span");
    assert_eq!(res[0].first_frame_index, 1, "residue starts at frame 1");
    assert_eq!(res[0].frame_count, 1);
    assert!(matches!(
        res[0].reason,
        sqlite_core::ResidueReason::BeyondLastCommit
    ));
    // Replay-safe: a valid commit exists.
    assert_eq!(tl.safety(), MaterializationSafety::ReplaySafe);
}

#[test]
fn no_commit_frames_yields_physical_validated_only() {
    let main = synth_main();
    let mut wal = wal_header(0xCAFE_BABE, 0xDEAD_BEEF, 1);
    // A single non-commit frame: physically valid, but nothing to materialize.
    push_frame(&mut wal, 2, 0, 0xCAFE_BABE, 0xDEAD_BEEF, b"NO-COMMIT");

    let tl = Database::wal_timeline_from(&main, &wal).expect("timeline");
    assert!(tl.commit_snapshots().is_empty(), "no materializable state");
    assert_eq!(
        tl.safety(),
        MaterializationSafety::PhysicalValidated,
        "physically valid but no commit → no replay"
    );
    // The lone non-commit frame is an uncommitted-tail residue span.
    assert_eq!(tl.residue().len(), 1);
}

#[test]
fn header_only_wal_has_no_segments_no_snapshots() {
    // A WAL with a valid header but ZERO frames: physically valid, empty timeline.
    let main = synth_main();
    let wal = wal_header(0x0102_0304, 0x0506_0708, 1);
    let tl = Database::wal_timeline_from(&main, &wal).expect("timeline");
    assert!(tl.segments().is_empty(), "no frames → no segment");
    assert!(tl.commit_snapshots().is_empty());
    assert!(tl.residue().is_empty());
    assert_eq!(tl.safety(), MaterializationSafety::PhysicalValidated);
    assert_eq!(
        tl.topology(),
        CohortTopology::LinearSegment,
        "zero/one segment is linear"
    );
    assert!(tl.diff_base_to_last_commit().is_none(), "no commit to diff");
}

#[test]
fn zero_page_no_frame_halts_the_scan() {
    // A frame whose page_no is 0 is malformed; the parser stops rather than
    // mis-indexing. The committed frame before it is still surfaced.
    let main = synth_main();
    let mut wal = wal_header(0x1212_1212, 0x3434_3434, 1);
    push_frame(&mut wal, 2, 2, 0x1212_1212, 0x3434_3434, b"GOOD-COMMIT");
    push_frame(&mut wal, 0, 0, 0x1212_1212, 0x3434_3434, b"BAD-PAGE-ZERO");
    let tl = Database::wal_timeline_from(&main, &wal).expect("timeline");
    assert_eq!(tl.commit_snapshots().len(), 1, "the good commit survives");
    assert_eq!(
        tl.segments()[0].frame_count,
        1,
        "the zero-page frame is dropped"
    );
}
