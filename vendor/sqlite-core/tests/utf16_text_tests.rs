//! Real-artifact validation: text decoding must honor the database text
//! encoding (file-format §1.3.1, header byte 56: 1=UTF-8, 2=UTF-16LE,
//! 3=UTF-16BE). A UTF-16 database whose text is decoded as UTF-8 produces silent
//! mojibake (U+FFFD) — the format-coverage-audit "silent wrong output" class.
//!
//! Fixtures are produced by the real `sqlite3` engine (the `PRAGMA encoding`
//! must precede any table) and live in the repo-root `tests/data/` (gitignored;
//! generator recorded in `issen/docs/corpus-catalog.md`). The tests skip
//! gracefully when the fixtures are absent so a clean checkout still passes.
//!
//! Generator:
//! ```text
//! sqlite3 utf16le.sqlite "PRAGMA page_size=512; PRAGMA encoding='UTF-16le';
//!   CREATE TABLE t(s TEXT); INSERT INTO t VALUES('héllo wörld');"
//! sqlite3 utf8.sqlite    "PRAGMA ...; PRAGMA encoding='UTF-8';    ... ;"
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use sqlite_core::{Database, Value};

/// Real SQLite fixture path (repo-root `tests/data/`), or `None` to skip.
fn fixture(name: &str) -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name);
    if p.exists() {
        Some(p)
    } else {
        eprintln!(
            "SKIP — real fixture not present: {} (see corpus-catalog.md)",
            p.display()
        );
        None
    }
}

/// The single text value stored in table `t` (root page 2 in a fresh one-table
/// db, one column).
fn read_only_text(path: &Path) -> String {
    let bytes = std::fs::read(path).expect("read fixture");
    let db = Database::open(bytes).expect("valid sqlite db");
    let rows = db.read_table(2, 1).expect("read table t");
    let row = rows.first().expect("table t has one row");
    match &row.values[0] {
        Value::Text(s) => s.clone(),
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn utf8_database_text_decodes_correctly() {
    // Control: the UTF-8 database already decodes correctly today.
    let Some(p) = fixture("utf8.sqlite") else {
        return;
    };
    assert_eq!(read_only_text(&p), "héllo wörld");
}

#[test]
fn utf16le_database_text_decodes_correctly() {
    // A real UTF-16LE database: text must decode via the header's encoding, not
    // be lossy-UTF-8-decoded into U+FFFD mojibake.
    let Some(p) = fixture("utf16le.sqlite") else {
        return;
    };
    assert_eq!(
        read_only_text(&p),
        "héllo wörld",
        "UTF-16LE text must decode per header byte 56, not as lossy UTF-8"
    );
}

#[test]
fn utf16be_database_text_decodes_correctly() {
    // The UTF-16BE branch is exercised by a real big-endian database — not left
    // synthetic-only (the validation-axis trap this whole audit exists to avoid).
    let Some(p) = fixture("utf16be.sqlite") else {
        return;
    };
    assert_eq!(
        read_only_text(&p),
        "héllo wörld",
        "UTF-16BE must decode per header byte 56"
    );
}
