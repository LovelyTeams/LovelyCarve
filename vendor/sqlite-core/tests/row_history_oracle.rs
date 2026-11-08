//! Real-`sqlite3` gold oracle for the per-rowid VERSION HISTORY (Phase 1).
//!
//! Mints an evidence database with a KNOWN mutation sequence using the real
//! `sqlite3` binary (`journal_mode=WAL`, `wal_autocheckpoint=0`,
//! `secure_delete=OFF`), holding a reader open so the `-wal` is RETAINED (this
//! host's `sqlite3` otherwise checkpoints-and-deletes the sidecar on close), then
//! asserts [`Database::row_histories`] reconstructs the exact insert / update /
//! delete / rowid-REUSE history that the known script produced.
//!
//! Gated on a `sqlite3` binary (PATH or `SQLITE3_BIN`); **skips cleanly** when
//! absent (the `sqlite3_bin()` skip-if-absent pattern used across the suite).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use sqlite_core::row_history::ViewState;
use sqlite_core::{Database, Value};

/// Resolve a usable `sqlite3` binary, or `None` (the test then skips).
fn sqlite3_bin() -> Option<String> {
    let bin = std::env::var("SQLITE3_BIN").unwrap_or_else(|_| "sqlite3".to_string());
    Command::new(&bin)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| bin)
}

/// A unique scratch directory for one test run (caller cleans up).
fn scratch(tag: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut p = std::env::temp_dir();
    p.push(format!("sqlite4n6_rowhist_{tag}_{nonce}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

mod common;
use common::{writer_sql, HeldReader};

/// Build the WAL fixture with the KNOWN mutation sequence; return the held reader
/// (kept alive until dropped) and the `(db, wal)` paths.
///
/// Table `t(id INTEGER PRIMARY KEY, name TEXT)`:
///   C1: insert (1,'a'),(2,'b'),(4,'x')
///   C2: update 1->'A'; delete 2; delete 4
///   C3: insert 3->'c'; insert 4->'y'   (rowid 4 REUSED after its C2 delete)
fn build_fixture(bin: &str, dir: &Path) -> (HeldReader, PathBuf, PathBuf) {
    let db = dir.join("ev.db");
    let wal = dir.join("ev.db-wal");

    let mut reader = HeldReader::spawn(bin, &db);
    // Each `run` returns only once sqlite3 has executed the statement, so the
    // writers below cannot race ahead of the CREATE TABLE.
    reader.run(
        "PRAGMA journal_mode=WAL;\nPRAGMA wal_autocheckpoint=0;\nPRAGMA secure_delete=OFF;\n\
         CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT);",
    );
    // Open the read transaction that retains the -wal across writer commits.
    reader.run("BEGIN;\nSELECT count(*) FROM t;");

    writer_sql(
        bin,
        &db,
        "PRAGMA wal_autocheckpoint=0; \
         INSERT INTO t VALUES(1,'a'); INSERT INTO t VALUES(2,'b'); INSERT INTO t VALUES(4,'x');",
    );
    writer_sql(
        bin,
        &db,
        "PRAGMA wal_autocheckpoint=0; \
         UPDATE t SET name='A' WHERE id=1; DELETE FROM t WHERE id=2; DELETE FROM t WHERE id=4;",
    );
    writer_sql(
        bin,
        &db,
        "PRAGMA wal_autocheckpoint=0; \
         INSERT INTO t VALUES(3,'c'); INSERT INTO t VALUES(4,'y');",
    );

    (reader, db, wal)
}

/// Release the held reader and remove the scratch directory.
fn teardown(reader: HeldReader, dir: &Path) {
    reader.finish();
    let _ = std::fs::remove_dir_all(dir);
}

/// The `name` TEXT of a version (column index 1), for assertions.
fn name_of(v: &sqlite_core::row_history::RowVersion) -> String {
    match v.values.get(1) {
        Some(Value::Text(s)) => s.clone(),
        other => panic!("expected TEXT name, got {other:?}"),
    }
}

#[test]
fn row_histories_reconstruct_the_known_mutation_sequence() {
    let Some(bin) = sqlite3_bin() else {
        eprintln!("SKIP row_histories_reconstruct_the_known_mutation_sequence: no sqlite3");
        return;
    };
    let dir = scratch("oracle");
    let (reader, db_path, wal_path) = build_fixture(&bin, &dir);

    let main = std::fs::read(&db_path).unwrap();
    let walb = std::fs::read(&wal_path).expect("snapshot -wal present");
    assert!(!walb.is_empty(), "the -wal must be RETAINED (non-empty)");
    let db = Database::open_with_wal(main, &walb).expect("open with wal");

    let histories = db.row_histories();
    let t = histories
        .iter()
        .find(|h| h.table == "t")
        .expect("table t present");
    assert!(!t.without_rowid, "t is an ordinary rowid table");

    let by_rowid = |id: i64| -> Vec<&sqlite_core::row_history::RowVersion> {
        t.versions
            .iter()
            .filter(|v| v.rowid == Some(id))
            .collect::<Vec<_>>()
    };

    // rowid 1: 'a' (changed later) @C1, then 'A' present in final @C2.
    let r1 = by_rowid(1);
    assert_eq!(r1.len(), 2, "rowid 1 has two versions");
    assert_eq!(name_of(r1[0]), "a");
    assert_eq!(r1[0].view_state, ViewState::ValueChangedLater);
    assert_eq!(name_of(r1[1]), "A");
    assert_eq!(r1[1].view_state, ViewState::PresentInFinalView);
    // commit_seq is per-epoch monotonic: C1 < C2.
    assert!(
        r1[0].commit_seq.unwrap() < r1[1].commit_seq.unwrap(),
        "commit_seq is per-epoch monotonic"
    );

    // rowid 2: 'b' inserted @C1, deleted @C2 — AbsentInFinalView, is_deleted.
    let r2 = by_rowid(2);
    assert_eq!(r2.len(), 1, "rowid 2 has one (deleted) version");
    assert_eq!(name_of(r2[0]), "b");
    assert_eq!(r2[0].view_state, ViewState::AbsentInFinalView);
    assert!(r2[0].is_deleted);

    // rowid 3: 'c' inserted @C3, present in final.
    let r3 = by_rowid(3);
    assert_eq!(r3.len(), 1, "rowid 3 has one (live) version");
    assert_eq!(name_of(r3[0]), "c");
    assert_eq!(r3[0].view_state, ViewState::PresentInFinalView);

    // rowid 4: REUSED. 'x' deleted @C2 (AbsentInFinalView), 'y' reinserted @C3
    // (PresentInFinalView). Both flagged rowid_reused.
    let r4 = by_rowid(4);
    assert_eq!(r4.len(), 2, "rowid 4 has two distinct entities");
    assert_eq!(name_of(r4[0]), "x");
    assert_eq!(r4[0].view_state, ViewState::AbsentInFinalView);
    assert!(r4[0].is_deleted);
    assert_eq!(name_of(r4[1]), "y");
    assert_eq!(r4[1].view_state, ViewState::PresentInFinalView);
    assert!(
        r4.iter().all(|v| v.rowid_reused),
        "every version of a reused rowid is flagged rowid_reused"
    );

    teardown(reader, &dir);
}
