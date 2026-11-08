//! Independent **`sqlite3` CLI** oracle for the rebuild writer.
//!
//! The self-reader round-trip (`rebuild.rs`) checks the bytes against our OWN
//! reader. This test removes that shared-author blind spot: it writes the
//! produced bytes to a temp file and has the real `sqlite3` binary read them —
//! `PRAGMA integrity_check`, `count(*)`, per-cell `typeof`/`quote`, and `hex()`
//! of an overflow-spanning BLOB — so an external engine confirms the file is a
//! genuine `SQLite` database, not merely one our reader happens to accept.
//!
//! Gated on a `sqlite3` binary (PATH, or `SQLITE3_BIN`); **skips gracefully**
//! when absent so CI runners without `sqlite3` stay green (mirrors the
//! `sqlite3_bin()` pattern in `forensic/tests/oracle_differential.rs`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::process::Command;

use sqlite_core::rebuild::{
    build_recovered_db, build_recovered_db_with_fragments, FragmentRow, RebuildRow,
};
use sqlite_core::Value;

/// Resolve a usable `sqlite3` binary, or `None` (test then skips).
fn sqlite3_bin() -> Option<String> {
    let bin = std::env::var("SQLITE3_BIN").unwrap_or_else(|_| "sqlite3".to_string());
    Command::new(&bin)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| bin)
}

/// Write `bytes` to a unique temp file and return its path (caller cleans up).
fn temp_db(bytes: &[u8], tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    p.push(format!("sqlite4n6_rebuild_{tag}_{nonce}.db"));
    std::fs::write(&p, bytes).unwrap();
    p
}

/// Run one `sqlite3 <db> "<sql>"` and return trimmed stdout.
fn run_sql(bin: &str, db: &PathBuf, sql: &str) -> String {
    let out = Command::new(bin)
        .arg(db)
        .arg(sql)
        .output()
        .expect("sqlite3 must execute");
    assert!(
        out.status.success(),
        "sqlite3 failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn sqlite3_reads_rebuilt_db_small_and_overflow() {
    let Some(bin) = sqlite3_bin() else {
        eprintln!("SKIP sqlite3_reads_rebuilt_db_small_and_overflow: no sqlite3 (set SQLITE3_BIN)");
        return;
    };

    // A multi-KB BLOB forces an overflow chain; distinct byte pattern catches
    // any misassembly. A second row exercises an unknown (NULL) rowid.
    let big: Vec<u8> = (0..9000u32).map(|i| (i % 251) as u8).collect();
    let rows = vec![
        RebuildRow {
            page: 7,
            offset: 128,
            rowid: Some(42),
            source: "freelist-page".into(),
            confidence: 0.9,
            cells: vec![
                Value::Integer(7),
                Value::Text("alice".into()),
                Value::Real(1.5),
                Value::Blob(vec![0xDE, 0xAD, 0xBE, 0xEF]),
            ],
        },
        RebuildRow {
            page: 8,
            offset: 256,
            rowid: None,
            source: "freeblock-reconstructed".into(),
            confidence: 0.4,
            cells: vec![Value::Text("spilled".into()), Value::Blob(big.clone())],
        },
    ];
    let bytes = build_recovered_db(&rows);
    let db = temp_db(&bytes, "small_overflow");

    // An external engine must vouch for the file's structural integrity.
    assert_eq!(run_sql(&bin, &db, "PRAGMA integrity_check;"), "ok");
    assert_eq!(
        run_sql(&bin, &db, "SELECT count(*) FROM recovered_records;"),
        "2"
    );

    // Native storage classes survive: integer / text / real / blob, plus a NULL
    // rowid for the unknown-rowid record.
    let typed = run_sql(
        &bin,
        &db,
        "SELECT typeof(c0), quote(c0), typeof(c1), typeof(c2), typeof(c3) \
         FROM recovered_records WHERE _rowid = 42;",
    );
    assert_eq!(typed, "integer|7|text|real|blob", "got: {typed}");
    assert_eq!(
        run_sql(
            &bin,
            &db,
            "SELECT quote(c3) FROM recovered_records WHERE _rowid = 42;"
        ),
        "X'DEADBEEF'"
    );
    assert_eq!(
        run_sql(
            &bin,
            &db,
            "SELECT count(*) FROM recovered_records WHERE _rowid IS NULL;"
        ),
        "1"
    );

    // The overflow-spanning BLOB reads back at full length with the exact bytes.
    let blob_len = run_sql(
        &bin,
        &db,
        "SELECT length(c1) FROM recovered_records WHERE _source = 'freeblock-reconstructed';",
    );
    assert_eq!(blob_len, big.len().to_string());
    let head = run_sql(
        &bin,
        &db,
        "SELECT hex(substr(c1,1,8)) FROM recovered_records WHERE _source = 'freeblock-reconstructed';",
    );
    assert_eq!(head, "0001020304050607", "overflow BLOB head bytes");

    let _ = std::fs::remove_file(&db);
}

#[test]
fn sqlite3_reads_rebuilt_empty_db() {
    let Some(bin) = sqlite3_bin() else {
        eprintln!("SKIP sqlite3_reads_rebuilt_empty_db: no sqlite3 (set SQLITE3_BIN)");
        return;
    };
    let bytes = build_recovered_db(&[]);
    let db = temp_db(&bytes, "empty");
    assert_eq!(run_sql(&bin, &db, "PRAGMA integrity_check;"), "ok");
    assert_eq!(
        run_sql(&bin, &db, "SELECT count(*) FROM recovered_records;"),
        "0"
    );
    let _ = std::fs::remove_file(&db);
}

#[test]
fn sqlite3_reads_two_table_db_with_fragments() {
    let Some(bin) = sqlite3_bin() else {
        eprintln!("SKIP sqlite3_reads_two_table_db_with_fragments: no sqlite3 (set SQLITE3_BIN)");
        return;
    };

    let records = vec![RebuildRow {
        page: 7,
        offset: 128,
        rowid: Some(42),
        source: "freelist-page".into(),
        confidence: 0.9,
        cells: vec![Value::Integer(7), Value::Text("alice".into())],
    }];
    let fragments = vec![
        FragmentRow {
            page: 2,
            offset: 3965,
            missing: 1,
            confidence: 0.2,
            surviving: vec![
                (0, Value::Integer(20004)),
                (1, Value::Text("Anja".into())),
                (2, Value::Text("Frank".into())),
            ],
        },
        // Sparse fragment: only column 2 present, a BLOB (native storage class).
        FragmentRow {
            page: 5,
            offset: 64,
            missing: 2,
            confidence: 0.2,
            surviving: vec![(2, Value::Blob(vec![0xCA, 0xFE, 0xBA]))],
        },
    ];
    let bytes = build_recovered_db_with_fragments(&records, Some(&fragments));
    let db = temp_db(&bytes, "two_table");

    // An external engine vouches for the whole two-b-tree file's integrity.
    assert_eq!(run_sql(&bin, &db, "PRAGMA integrity_check;"), "ok");
    assert_eq!(
        run_sql(&bin, &db, "SELECT count(*) FROM recovered_records;"),
        "1"
    );
    assert_eq!(
        run_sql(&bin, &db, "SELECT count(*) FROM recovered_fragments;"),
        "2"
    );

    // A surviving cell keeps its native storage class at its native column index.
    assert_eq!(
        run_sql(
            &bin,
            &db,
            "SELECT typeof(c1), quote(c1) FROM recovered_fragments WHERE c0 = 20004;"
        ),
        "text|'Anja'"
    );
    // The sparse fragment: c2 is a BLOB, c0/c1 are NULL.
    assert_eq!(
        run_sql(
            &bin,
            &db,
            "SELECT typeof(c2), quote(c2), c0 IS NULL, c1 IS NULL \
             FROM recovered_fragments WHERE _offset = 64;"
        ),
        "blob|X'CAFEBA'|1|1"
    );

    let _ = std::fs::remove_file(&db);
}

#[test]
fn sqlite3_reads_two_table_db_with_empty_fragment_set() {
    let Some(bin) = sqlite3_bin() else {
        eprintln!(
            "SKIP sqlite3_reads_two_table_db_with_empty_fragment_set: no sqlite3 (set SQLITE3_BIN)"
        );
        return;
    };
    let records = vec![RebuildRow {
        page: 1,
        offset: 0,
        rowid: Some(1),
        source: "freelist-page".into(),
        confidence: 0.9,
        cells: vec![Value::Integer(1)],
    }];
    let bytes = build_recovered_db_with_fragments(&records, Some(&[]));
    let db = temp_db(&bytes, "two_table_empty");
    assert_eq!(run_sql(&bin, &db, "PRAGMA integrity_check;"), "ok");
    // An empty-but-present fragment table is queryable and returns 0.
    assert_eq!(
        run_sql(&bin, &db, "SELECT count(*) FROM recovered_fragments;"),
        "0"
    );
    let _ = std::fs::remove_file(&db);
}
