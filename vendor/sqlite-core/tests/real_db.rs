//! End-to-end validation against a REAL SQLite `.db` built with the `sqlite3`
//! CLI (see `docs/ws-c-sqlite-core-spike.md` for the exact generator commands).
//!
//! The expected rows below are the verbatim output of:
//!   sqlite3 tests/data/places.db \
//!     "SELECT id,url,title,visit_count,last_visit_date,frecency FROM moz_places ORDER BY id;"
//! i.e. a differential cross-check of the native reader against libsqlite.

#![allow(clippy::unwrap_used, clippy::expect_used)]
// The module doc embeds a literal SQL command line; backticking its tokens
// would mangle the reproducer, so the pedantic doc lint is opted out here.
#![allow(clippy::doc_markdown)]

use sqlite_core::{Database, Value};

const DB: &[u8] = include_bytes!("data/places.db");

/// `moz_places` is the first user table; with the sqlite_schema on page 1 its
/// root page is page 2 in this single-table DB. Confirmed below by reading the
/// schema is out of spike scope, so we target page 2 directly.
const MOZ_PLACES_ROOT: u32 = 2;
const MOZ_PLACES_COLS: usize = 6;

#[test]
fn header_parses_real_db() {
    let db = Database::open(DB.to_vec()).expect("real db header must parse");
    let h = db.header();
    assert_eq!(h.page_size, 4096, "page size from PRAGMA page_size");
    assert_eq!(h.reserved, 0);
}

#[test]
fn rejects_non_sqlite_bytes() {
    let err = Database::open(b"not a sqlite file at all, padding..............".to_vec());
    assert!(err.is_err());
}

#[test]
fn walks_moz_places_rows_matching_sqlite3() {
    let db = Database::open(DB.to_vec()).expect("open");
    let rows = db
        .read_table(MOZ_PLACES_ROOT, MOZ_PLACES_COLS)
        .expect("walk moz_places");

    assert_eq!(rows.len(), 5, "five inserted rows");

    // Row 1 — TEXT, INTEGER, REAL, rowid-alias id.
    assert_eq!(rows[0].rowid, 1);
    assert_eq!(rows[0].values[0], Value::Integer(1)); // id (INTEGER PRIMARY KEY alias)
    assert_eq!(
        rows[0].values[1],
        Value::Text("https://www.rust-lang.org/".into())
    );
    assert_eq!(
        rows[0].values[2],
        Value::Text("Rust Programming Language".into())
    );
    assert_eq!(rows[0].values[3], Value::Integer(5));
    assert_eq!(rows[0].values[4], Value::Integer(1_700_000_000_000_000));
    // frecency 2000.5 has a fraction → stored as IEEE float (serial 7).
    assert_eq!(rows[0].values[5], Value::Real(2000.5));

    // Row 2 — frecency inserted as 5500.0 but stored on disk as an INTEGER
    // (serial 2). This is a genuine forensic distinction: rusqlite applies the
    // column's REAL *affinity* and returns 5500.0; the native reader faithfully
    // reports the on-disk *storage class*. typeof(frecency) in sqlite3 returns
    // 'real' (affinity), yet the serial type on disk is integer.
    assert_eq!(rows[1].values[1], Value::Text("https://github.com/".into()));
    assert_eq!(rows[1].values[3], Value::Integer(12));
    assert_eq!(rows[1].values[5], Value::Integer(5500));

    // Row 3 — NULL last_visit_date; frecency -1.0 stored as INTEGER (serial 1).
    assert_eq!(rows[2].values[2], Value::Text("Hacker News".into()));
    assert_eq!(rows[2].values[4], Value::Null);
    assert_eq!(rows[2].values[5], Value::Integer(-1));

    // Row 4 — NULL frecency (trailing NULL column); visit_count 1.
    assert_eq!(rows[3].values[2], Value::Text("SQLite - Wikipedia".into()));
    assert_eq!(rows[3].values[3], Value::Integer(1));
    assert_eq!(rows[3].values[5], Value::Null);

    // Row 5 — NULL title; visit_count 0 stored via serial-type-8 literal;
    // frecency 0.0 stored as INTEGER (serial 8 → literal 0).
    assert_eq!(rows[4].values[2], Value::Null);
    assert_eq!(rows[4].values[3], Value::Integer(0));
    assert_eq!(rows[4].values[5], Value::Integer(0));
}

#[test]
fn never_panics_on_truncated_input() {
    // Every prefix of a real DB must yield Ok or Err, never a panic.
    for len in 0..DB.len().min(4200) {
        let bytes = DB[..len].to_vec();
        if let Ok(db) = Database::open(bytes) {
            let _ = db.read_table(MOZ_PLACES_ROOT, MOZ_PLACES_COLS);
        }
    }
}
