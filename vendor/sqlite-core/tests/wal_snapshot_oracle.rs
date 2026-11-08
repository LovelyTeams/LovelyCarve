//! Real-`sqlite3` gold oracle for the WAL snapshot-read foundation (Phase 0).
//!
//! Builds an evidence database INCREMENTALLY with the real `sqlite3` binary
//! (`journal_mode=WAL`, `wal_autocheckpoint=0`, `secure_delete=OFF`), snapshotting
//! the `.db` + `-wal` pair after each commit, then uses `sqlite3` itself as the
//! per-commit oracle. Three properties are pinned against the external engine:
//!  1. every commit in a clean WAL is checksum-valid (file-format §4.2);
//!  2. flipping one byte of a frame's page data invalidates that commit's chain;
//!  3. the snapshot-scoped table read (overflow included) equals the oracle's rows.
//!
//! A held reader blocks the checkpoint-on-close so the `-wal` is RETAINED (this
//! host's `sqlite3` otherwise checkpoints and deletes the sidecar when the last
//! connection closes). The test asserts the retained `-wal` is non-empty.
//!
//! Gated on a `sqlite3` binary (PATH or `SQLITE3_BIN`); **skips cleanly** when
//! absent (mirrors the `sqlite3_bin()` pattern used across the suite).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

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
    p.push(format!("sqlite4n6_walsnap_{tag}_{nonce}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Run `sqlite3 <db> "<sql>"` on a SHORT-LIVED writer connection. With a held
/// reader already open (see [`build_incremental_fixture`]) the checkpoint-on-close
/// is blocked, so the `-wal` survives.
mod common;
use common::{writer_sql, HeldReader};

/// Query a snapshot db with `sqlite3` and return the oracle rows
/// `(id, name, quote(big))` in id order.
pub fn oracle_rows(bin: &str, db: &Path) -> Vec<(i64, String, String)> {
    // Query a DISPOSABLE copy of the snapshot pair. A plain sqlite3 open of a WAL
    // database checkpoints-and-truncates the `-wal` when the connection closes;
    // running that against the original would destroy the fixture our
    // snapshot-aware reader still needs. So copy `snapN.db` + `snapN.db-wal` to a
    // throwaway pair, let sqlite3 mutate THAT, and leave the original intact.
    let wal_src = db.with_file_name(format!("{}-wal", db.file_name().unwrap().to_string_lossy()));
    let tmp_db = db.with_file_name(format!(
        "oracle_{}",
        db.file_name().unwrap().to_string_lossy()
    ));
    let tmp_wal = tmp_db.with_file_name(format!(
        "{}-wal",
        tmp_db.file_name().unwrap().to_string_lossy()
    ));
    std::fs::copy(db, &tmp_db).unwrap();
    std::fs::copy(&wal_src, &tmp_wal).unwrap();

    let out = Command::new(bin)
        .arg("-separator")
        .arg("\u{1f}")
        .arg(&tmp_db)
        .arg("SELECT id, name, quote(big) FROM t ORDER BY id")
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&tmp_db);
    let _ = std::fs::remove_file(&tmp_wal);
    assert!(
        out.status.success(),
        "sqlite3 oracle failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            let mut f = line.split('\u{1f}');
            let id: i64 = f.next().unwrap().parse().unwrap();
            let name = f.next().unwrap().to_string();
            let big = f.next().unwrap().to_string();
            (id, name, big)
        })
        .collect()
}

/// One captured commit snapshot: the copied `.db` + `-wal` pair.
pub struct Snap {
    pub db: PathBuf,
    pub wal: PathBuf,
}

/// Build the evidence DB incrementally, snapshotting after each commit. Returns
/// the held-reader child (kept alive until the caller drops it) and the snapshots.
///
/// The held reader is a `sqlite3` process reading commands from a pipe; it opens
/// a read transaction (`BEGIN; SELECT ...`) that blocks checkpoint so the `-wal`
/// is retained across the short-lived writer connections.
fn build_incremental_fixture(bin: &str, dir: &Path) -> (HeldReader, Vec<Snap>) {
    let db = dir.join("ev.db");
    let wal = dir.join("ev.db-wal");

    // Held reader: keep a connection open with an active read txn to block the
    // checkpoint-on-close that would otherwise delete the -wal.
    let mut reader = HeldReader::spawn(bin, &db);
    // Acknowledged, not slept on: the writers below cannot race the CREATE TABLE.
    reader.run(
        "PRAGMA journal_mode=WAL;\nPRAGMA wal_autocheckpoint=0;\nPRAGMA secure_delete=OFF;\n\
         CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT, big BLOB);",
    );
    // Open the read transaction that retains the -wal across writer commits.
    reader.run("BEGIN;\nSELECT count(*) FROM t;");

    let mut snaps = Vec::new();
    let snap = |dir: &Path, n: usize, db: &Path, wal: &Path| -> Snap {
        let sdb = dir.join(format!("snap{n}.db"));
        let swal = dir.join(format!("snap{n}.db-wal"));
        std::fs::copy(db, &sdb).unwrap();
        std::fs::copy(wal, &swal).unwrap();
        Snap { db: sdb, wal: swal }
    };

    // Commit 1: a 9000-byte zero blob (forces an overflow chain) + a small row.
    writer_sql(
        bin,
        &db,
        "PRAGMA wal_autocheckpoint=0; \
         INSERT INTO t VALUES(1,'alice', zeroblob(9000)); \
         INSERT INTO t VALUES(2,'bob', x'aabb');",
    );
    snaps.push(snap(dir, 1, &db, &wal));

    // Commit 2: edit row 1 to a 12000-byte random blob (overflow grows), delete
    // row 2, insert row 3 — a genuinely-different db state.
    writer_sql(
        bin,
        &db,
        "PRAGMA wal_autocheckpoint=0; \
         UPDATE t SET name='ALICE', big=randomblob(12000) WHERE id=1; \
         DELETE FROM t WHERE id=2; \
         INSERT INTO t VALUES(3,'carol',x'00');",
    );
    snaps.push(snap(dir, 2, &db, &wal));

    (reader, snaps)
}

/// Release the held reader and remove the scratch directory.
fn teardown(reader: HeldReader, dir: &Path) {
    reader.finish();
    let _ = std::fs::remove_dir_all(dir);
}

/// Quote a blob value the way `sqlite3`'s `quote()` does: `X'AABB…'` uppercase
/// hex, `NULL` for null.
pub fn quote_blob(v: &Value) -> String {
    match v {
        Value::Null => "NULL".to_string(),
        Value::Blob(b) => {
            use std::fmt::Write as _;
            let mut s = String::from("X'");
            for byte in b {
                let _ = write!(s, "{byte:02X}");
            }
            s.push('\'');
            s
        }
        other => panic!("unexpected non-blob big value: {other:?}"),
    }
}

/// Read the rows of table `t` AS OF the last commit of a snapshot's timeline,
/// returning `(id, name, quote(big))` in id order — the snapshot-aware path under
/// test, mirroring the oracle's `SELECT id, name, quote(big)` projection.
fn snapshot_rows(snap: &Snap) -> Vec<(i64, String, String)> {
    let main = std::fs::read(&snap.db).unwrap();
    let walb = std::fs::read(&snap.wal).expect("snapshot -wal present");
    assert!(!walb.is_empty(), "the -wal must be retained (non-empty)");
    let db = Database::open_with_wal(main, &walb).expect("open with wal");
    let tl = db.wal_timeline().expect("timeline present");
    let snapshot = tl
        .commit_snapshots()
        .last()
        .expect("at least one commit snapshot");

    // Resolve table `t`'s rootpage from the snapshot's OWN schema (page 1 as of
    // this commit), then read its rows via the snapshot-scoped path.
    let t = snapshot
        .tables()
        .into_iter()
        .find(|t| t.name == "t")
        .expect("table t present in snapshot schema");
    let mut rows: Vec<(i64, String, String)> = snapshot
        .read_table(t.rootpage, t.columns.len())
        .expect("snapshot-scoped read")
        .into_iter()
        .map(|(rowid, values)| {
            // Columns: (id INTEGER PRIMARY KEY, name TEXT, big BLOB).
            let id = match values.first() {
                Some(Value::Integer(i)) => *i,
                _ => rowid,
            };
            let name = match values.get(1) {
                Some(Value::Text(s)) => s.clone(),
                _ => String::new(),
            };
            let big = values.get(2).map_or_else(|| "NULL".to_string(), quote_blob);
            (id, name, big)
        })
        .collect();
    rows.sort_by_key(|r| r.0);
    rows
}

#[test]
fn snapshot_reads_match_sqlite3_oracle_per_commit() {
    let Some(bin) = sqlite3_bin() else {
        eprintln!(
            "SKIP snapshot_reads_match_sqlite3_oracle_per_commit: no sqlite3 (set SQLITE3_BIN)"
        );
        return;
    };
    let dir = scratch("oracle");
    let (reader, snaps) = build_incremental_fixture(&bin, &dir);

    for (n, snap) in snaps.iter().enumerate() {
        let expected = oracle_rows(&bin, &snap.db);
        let got = snapshot_rows(snap);
        assert_eq!(
            got,
            expected,
            "snapshot {} table state must match the sqlite3 oracle",
            n + 1
        );
    }

    // The second snapshot's row 1 carries the 12000-byte overflow blob; the quote
    // string length confirms the overflow reassembly matched the oracle exactly.
    let last = oracle_rows(&bin, &snaps[1].db);
    let row1 = last.iter().find(|r| r.0 == 1).expect("row 1 at commit 2");
    // X'…' wraps 2 hex chars per byte → 12000 bytes = 24000 hex + 3 framing chars.
    assert_eq!(
        row1.2.len(),
        24000 + 3,
        "row 1 big blob is the 12000-byte overflow value"
    );

    teardown(reader, &dir);
}

#[test]
fn every_clean_commit_is_checksum_valid_and_a_flip_breaks_the_chain() {
    let Some(bin) = sqlite3_bin() else {
        eprintln!(
            "SKIP every_clean_commit_is_checksum_valid_and_a_flip_breaks_the_chain: no sqlite3"
        );
        return;
    };
    let dir = scratch("cksum");
    let (reader, snaps) = build_incremental_fixture(&bin, &dir);

    // (1) Every commit of a clean, real-sqlite3 WAL is checksum-valid.
    let main = std::fs::read(&snaps[1].db).unwrap();
    let walb = std::fs::read(&snaps[1].wal).unwrap();
    assert!(!walb.is_empty(), "-wal retained (non-empty)");
    let db = Database::open_with_wal(main.clone(), &walb).expect("open with wal");
    let tl = db.wal_timeline().expect("timeline");
    assert!(
        !tl.commit_snapshots().is_empty(),
        "fixture has commit snapshots"
    );
    assert!(
        tl.commit_snapshots()
            .iter()
            .all(sqlite_core::CommitSnapshot::checksum_valid),
        "every commit in a clean WAL must be checksum-valid"
    );

    // (2) Flip one byte of the LAST frame's page data → the chain from that frame
    // on breaks, so the last commit becomes checksum-invalid.
    let frame_stride = 24 + db.header().page_size as usize;
    assert!(
        walb.len() >= 32 + frame_stride,
        "at least one frame present"
    );
    let n_frames = (walb.len() - 32) / frame_stride;
    let last_frame_off = 32 + (n_frames - 1) * frame_stride;
    let flip_at = last_frame_off + 24 + 16; // 16 bytes into the page data
    let mut tampered = walb.clone();
    tampered[flip_at] ^= 0x01;
    let db2 = Database::open_with_wal(main, &tampered).expect("open with tampered wal");
    let tl2 = db2.wal_timeline().expect("timeline");
    assert!(
        !tl2.commit_snapshots()
            .last()
            .expect("last commit")
            .checksum_valid(),
        "a flipped page-data byte must invalidate the last commit's checksum chain"
    );

    teardown(reader, &dir);
}
