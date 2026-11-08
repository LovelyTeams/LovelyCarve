//! Overflow-page-chain validation against a REAL SQLite `.db` (see
//! `docs/corpus-catalog.md` for the generator). A `notes` table holds one row
//! whose ~12 KB TEXT body spills onto a chain of overflow pages, and one small
//! row that fits entirely on the leaf.
//!
//! Ground truth (cross-checked with `sqlite3`):
//!   notes root page = 2; row id=1 body length = 12017; row id=2 = 'small row'.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use sqlite_core::{Database, Value};

const DB: &[u8] = include_bytes!("data/overflow.db");
const NOTES_ROOT: u32 = 2;
const NOTES_COLS: usize = 2;

#[test]
fn large_text_row_decodes_fully_via_overflow() {
    let db = Database::open(DB.to_vec()).expect("open overflow.db");
    let rows = db.read_table(NOTES_ROOT, NOTES_COLS).expect("walk notes");
    assert_eq!(rows.len(), 2, "two rows");

    // Row id=1: the spilled body must reassemble to its full 12017 bytes, not
    // be truncated at the leaf-page boundary.
    let body = match &rows[0].values[1] {
        Value::Text(s) => s,
        other => panic!("expected Text body, got {other:?}"),
    };
    assert_eq!(
        body.len(),
        12017,
        "overflow chain must reassemble the full payload"
    );
    assert!(
        body.starts_with("OVERFLOW_PAYLOAD_"),
        "payload prefix preserved"
    );
    // The body is the prefix + 1200 repetitions of 'ABCDEFGHIJ'.
    assert!(body.ends_with("ABCDEFGHIJ"), "payload tail preserved");

    // Row id=2: the small row is unaffected.
    assert_eq!(rows[1].values[1], Value::Text("small row".into()));
}
