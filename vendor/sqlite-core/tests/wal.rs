//! WAL-overlay validation against a REAL SQLite database + `-wal` sidecar (see
//! `docs/corpus-catalog.md` for the generator). The fixture was captured while
//! a held reader connection blocked checkpointing, so the `-wal` sidecar holds
//! one committed COMMIT frame for page 2 that the main file does not yet reflect.
//!
//! Ground truth (cross-checked with the `sqlite3` CLI):
//! - main-only view:  id=1 title `Rust`, `visit_count` 5; 2 rows.
//! - WAL-applied view: id=1 title `Rust (EDITED IN WAL)`, `visit_count` 777;
//!   plus id=3 `WAL-ONLY ROW`; 3 rows.
//!
//! The reader must overlay the WAL WITHOUT mutating either file (the forensic-safe
//! alternative to libsqlite checkpointing).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use sqlite_core::{Database, Value};

const MAIN: &[u8] = include_bytes!("data/wal_places.db");
const WAL: &[u8] = include_bytes!("data/wal_places.db-wal");
const MOZ_PLACES_ROOT: u32 = 2;
const MOZ_PLACES_COLS: usize = 6;

#[test]
fn main_only_view_shows_pre_wal_state() {
    let db = Database::open(MAIN.to_vec()).expect("open main");
    let rows = db
        .read_table(MOZ_PLACES_ROOT, MOZ_PLACES_COLS)
        .expect("walk");
    assert_eq!(rows.len(), 2, "main file has 2 committed rows");
    assert_eq!(rows[0].values[2], Value::Text("Rust".into()));
    assert_eq!(rows[0].values[3], Value::Integer(5));
}

#[test]
fn wal_applied_view_overlays_committed_frame() {
    let db = Database::open_with_wal(MAIN.to_vec(), WAL).expect("open with wal");
    assert!(db.wal_applied(), "WAL frames were overlaid");

    let rows = db
        .read_table(MOZ_PLACES_ROOT, MOZ_PLACES_COLS)
        .expect("walk wal-applied");

    // The overlay changed page 2: id=1 title + visit_count updated, and a third
    // row appeared — all from the WAL, none of it written back to the file.
    assert_eq!(rows.len(), 3, "WAL adds a third row");
    assert_eq!(
        rows[0].values[2],
        Value::Text("Rust (EDITED IN WAL)".into()),
        "WAL overlay changed the title"
    );
    assert_eq!(rows[0].values[3], Value::Integer(777));
    assert_eq!(rows[2].values[2], Value::Text("WAL-ONLY ROW".into()));
}

#[test]
fn open_without_wal_does_not_claim_wal_applied() {
    let db = Database::open(MAIN.to_vec()).expect("open");
    assert!(!db.wal_applied());
}

#[test]
fn empty_wal_overlays_nothing() {
    // A WAL with only its 32-byte header (no frames) is a no-op overlay.
    let mut empty_wal = vec![0u8; 32];
    empty_wal[0..4].copy_from_slice(&0x377f_0682u32.to_be_bytes());
    let db = Database::open_with_wal(MAIN.to_vec(), &empty_wal).expect("open empty wal");
    let rows = db
        .read_table(MOZ_PLACES_ROOT, MOZ_PLACES_COLS)
        .expect("walk");
    assert_eq!(rows.len(), 2, "no frames → main-only state");
    assert_eq!(rows[0].values[2], Value::Text("Rust".into()));
}

// --- WAL-frame carving (#60) -------------------------------------------------
//
// `wal_carve.db` + `-wal`: a checkpoint(TRUNCATE) flushed baseline rows 1..=50
// to the MAIN file (empty WAL), then — with a reader holding a read txn to block
// any further checkpoint — rows 101..=150 were inserted (COMMIT) and 121..=140
// DELETEd (COMMIT), with NO checkpoint. So the freed-cell residue for the
// deleted rows 121..=140 lives ONLY in the uncheckpointed WAL frames; the main
// file's pages never held them. See `docs/corpus-catalog.md` §E.

const CARVE_MAIN: &[u8] = include_bytes!("data/wal_carve.db");
const CARVE_WAL: &[u8] = include_bytes!("data/wal_carve.db-wal");

#[test]
fn wal_frame_pages_exposes_each_frames_image_and_provenance() {
    let db = Database::open_with_wal(CARVE_MAIN.to_vec(), CARVE_WAL).expect("open with wal");
    let frames = db.wal_frame_pages();

    // The sidecar holds two committed frames for page 2 (the INSERT commit and
    // the DELETE commit), each a full page image carrying frame provenance.
    assert_eq!(frames.len(), 2, "two committed WAL frames");
    let page_size = db.header().page_size as usize;
    for (i, f) in frames.iter().enumerate() {
        assert_eq!(f.frame_index, i, "frame_index is the 0-based file order");
        assert_eq!(f.page_no, 2, "both frames rewrite page 2");
        assert_eq!(f.page.len(), page_size, "each frame carries one full page");
        // Salts identify the checkpoint generation (the (salt1,salt2,frame_index)
        // LSN that #55 will formalize); both frames share the WAL header salts.
        assert!(f.salt1 != 0 && f.salt2 != 0, "frame carries WAL salts");
    }
    assert_eq!(
        frames[0].salt1, frames[1].salt1,
        "same checkpoint generation"
    );
    assert_eq!(
        frames[0].salt2, frames[1].salt2,
        "same checkpoint generation"
    );

    // The newest frame (the DELETE commit) is a commit frame.
    assert!(frames[1].is_commit, "the DELETE frame is a COMMIT frame");

    // The deleted-row residue (body text of ids 121..=140) is present in a WAL
    // frame's page image — the genuinely-different residue an on-disk-only carve
    // cannot see.
    let needle = b"secret WAL body 130";
    let in_wal_frame = frames
        .iter()
        .any(|f| f.page.windows(needle.len()).any(|w| w == needle));
    assert!(
        in_wal_frame,
        "deleted-row residue must live in a WAL frame image"
    );

    // ...and it is NOT on the main file's on-disk page 2 (the post-checkpoint
    // baseline never held rows 101..=150).
    let on_disk_page2 = db.raw_page(2).expect("page 2 present");
    let on_disk = on_disk_page2.windows(needle.len()).any(|w| w == needle);
    assert!(
        !on_disk,
        "deleted-row residue must NOT be on the on-disk page (WAL-only)"
    );
}

#[test]
fn wal_frame_pages_empty_without_wal() {
    let db = Database::open(CARVE_MAIN.to_vec()).expect("open");
    assert!(
        db.wal_frame_pages().is_empty(),
        "no WAL → no frame images to carve"
    );
}
