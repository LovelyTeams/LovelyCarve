//! WITHOUT ROWID table recovery (roadmap §1.4): such tables store their whole row
//! in an index b-tree (no separate table b-tree, no rowid), so the ordinary
//! table-b-tree reader is blind to them. `without_rowid_table_rows` walks each
//! WITHOUT ROWID table's index b-tree (interior 0x02 → leaf 0x0a) and returns its
//! live rows, keyed by table name — making these tables visible for the first time.
//!
//! Tier-2 oracle: the databases are minted by the real `sqlite3` engine, so the
//! expected rows are derived from the documented construction. Env-gated on a
//! `sqlite3` binary (`SQLITE3_BIN`, else PATH); a non-gated hand-built test keeps
//! the parser covered where `sqlite3` is absent.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use sqlite_core::{Database, Value};

fn sqlite3_bin() -> Option<String> {
    let bin = std::env::var("SQLITE3_BIN").unwrap_or_else(|_| "sqlite3".to_string());
    Command::new(&bin)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| bin)
}

#[test]
fn reads_live_without_rowid_table_rows() {
    let Some(bin) = sqlite3_bin() else {
        eprintln!("SKIP without_rowid: no sqlite3 (set SQLITE3_BIN)");
        return;
    };
    let dir = std::env::temp_dir().join("sqlite4n6-without-rowid");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db_path: PathBuf = dir.join("wr.db");

    // A WITHOUT ROWID table `kv` (single-leaf root), plus an ORDINARY rowid table
    // `plain` that must NOT appear in the result.
    let sql = "PRAGMA page_size=4096;
               CREATE TABLE kv(k TEXT PRIMARY KEY, v INTEGER, note TEXT) WITHOUT ROWID;
               INSERT INTO kv VALUES('alpha',1,'first'),('bravo',2,'second'),('charlie',3,'third');
               CREATE TABLE plain(id INTEGER PRIMARY KEY, x TEXT);
               INSERT INTO plain VALUES(1,'ordinary');";
    assert!(
        Command::new(&bin)
            .arg(&db_path)
            .arg(sql)
            .status()
            .unwrap()
            .success(),
        "sqlite3 construction failed"
    );

    let db = Database::open(std::fs::read(&db_path).unwrap()).unwrap();
    let tables = db.without_rowid_table_rows();

    // Exactly one WITHOUT ROWID table, named `kv`; the ordinary `plain` is absent.
    assert_eq!(
        tables.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        vec!["kv"],
        "only the WITHOUT ROWID table is returned: {tables:?}"
    );
    let kv = &tables[0];
    // Rows come back in table column order (k, v, note), PK-ordered.
    let keys: Vec<&str> = kv
        .rows
        .iter()
        .filter_map(|r| match r.first() {
            Some(Value::Text(t)) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(keys, vec!["alpha", "bravo", "charlie"], "rows: {kv:?}");
    assert!(
        kv.rows.iter().all(|r| r.len() == 3),
        "each row carries all 3 columns: {kv:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn walks_a_multi_page_without_rowid_btree() {
    // Enough rows that the index b-tree grows past one leaf, forcing an interior
    // (0x02) root the walk must recurse through.
    let Some(bin) = sqlite3_bin() else {
        return;
    };
    let dir = std::env::temp_dir().join("sqlite4n6-without-rowid-big");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("big.db");
    let sql = "PRAGMA page_size=512;
               CREATE TABLE kv(k TEXT PRIMARY KEY, v TEXT) WITHOUT ROWID;
               WITH RECURSIVE c(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM c WHERE n<400)
               INSERT INTO kv SELECT printf('key-%05d', n), printf('val-%05d', n) FROM c;";
    assert!(Command::new(&bin)
        .arg(&db_path)
        .arg(sql)
        .status()
        .unwrap()
        .success());
    let db = Database::open(std::fs::read(&db_path).unwrap()).unwrap();
    let tables = db.without_rowid_table_rows();
    assert_eq!(tables.len(), 1);
    assert_eq!(
        tables[0].rows.len(),
        400,
        "every row across all leaves of the multi-page b-tree is read"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Non-gated coverage: a hand-built db whose `sqlite_master` lists a `WITHOUT ROWID`
/// table rooted at a synthetic index-leaf page — exercises the walk without any
/// engine, so CI without `sqlite3` still covers it.
#[test]
fn parses_a_handbuilt_without_rowid_table() {
    let db = build_handbuilt_without_rowid();
    let tables = db.without_rowid_table_rows();
    assert_eq!(tables.len(), 1, "one WITHOUT ROWID table");
    assert_eq!(tables[0].name, "wr");
    assert_eq!(tables[0].rows, vec![vec![Value::Integer(42)]]);
}

#[test]
fn row_histories_carries_without_rowid_live_rows() {
    // §1.4 surfacing: row_histories folds each WITHOUT ROWID table's live rows into
    // its TableHistory.without_rowid_rows (non-gated, so CI without sqlite3 covers
    // the fold branch).
    let db = build_handbuilt_without_rowid();
    let wr = db
        .row_histories()
        .into_iter()
        .find(|h| h.table == "wr")
        .expect("the WITHOUT ROWID table appears in row_histories");
    assert!(wr.without_rowid, "flagged WITHOUT ROWID");
    assert!(wr.versions.is_empty(), "no rowid version history");
    assert_eq!(
        wr.without_rowid_rows,
        vec![vec![Value::Integer(42)]],
        "its live rows are carried"
    );
}

// Silence unused-import warnings when the env gate skips the oracle tests.
#[allow(dead_code)]
fn _uses(_: &Path) {}

/// Build a 3-page database by hand: page 1 = `sqlite_master` (a `table` row named
/// `wr`, WITHOUT ROWID, rootpage 3), page 3 = an index-leaf holding one 1-column
/// record (the integer 42). No engine required.
fn build_handbuilt_without_rowid() -> Database {
    let ps = 4096usize;
    let mut b = vec![0u8; ps * 3];
    // ---- file header ----
    b[..16].copy_from_slice(b"SQLite format 3\0");
    b[16] = 0x10; // page size 4096
    b[17] = 0x00;
    b[56..60].copy_from_slice(&1u32.to_be_bytes()); // text encoding UTF-8
    b[28..32].copy_from_slice(&3u32.to_be_bytes()); // in-header db size = 3 pages

    // ---- page 1: sqlite_master table-leaf with one row ----
    // sqlite_master columns: (type, name, tbl_name, rootpage, sql)
    let p1 = 0usize;
    b[p1 + 100] = 0x0d; // table-leaf (page-1 header sits after the 100-byte file header)
    b[p1 + 100 + 3..p1 + 100 + 5].copy_from_slice(&1u16.to_be_bytes()); // 1 cell
                                                                        // Build the record for the schema row.
    let sql = "CREATE TABLE wr(k INTEGER PRIMARY KEY) WITHOUT ROWID";
    let record = schema_record(b"table", b"wr", b"wr", 3, sql.as_bytes());
    // Cell = payload-len varint + rowid varint + record. Place near page end.
    let cell_off = ps - record.len() - 2;
    let mut cell = Vec::new();
    push_varint(&mut cell, record.len() as u64); // payload length
    push_varint(&mut cell, 1); // rowid
    cell.extend_from_slice(&record);
    b[p1 + cell_off..p1 + cell_off + cell.len()].copy_from_slice(&cell);
    // cell pointer (leaf header is 8 bytes, starts at offset 100 on page 1)
    let ptr = p1 + 100 + 8;
    b[ptr..ptr + 2].copy_from_slice(&u16::try_from(cell_off).unwrap().to_be_bytes());

    // ---- page 3: index-leaf holding one 1-column record (integer 42) ----
    let p3 = 2 * ps;
    b[p3] = 0x0a; // index-leaf
    b[p3 + 3..p3 + 5].copy_from_slice(&1u16.to_be_bytes()); // 1 cell
    let icell_off = 4000usize;
    // index-leaf cell = payload-len varint + record; record = header_len(2) + serial(1) + body(42)
    b[p3 + icell_off] = 0x03; // payload length = 3
    b[p3 + icell_off + 1] = 0x02; // record header_len = 2
    b[p3 + icell_off + 2] = 0x01; // serial type 1 (8-bit int)
    b[p3 + icell_off + 3] = 42; // body
    let iptr = p3 + 8;
    b[iptr..iptr + 2].copy_from_slice(&u16::try_from(icell_off).unwrap().to_be_bytes());

    Database::open(b).expect("hand-built db opens")
}

/// Encode a 5-column `sqlite_master` record: `(type, name, tbl_name, rootpage, sql)`.
fn schema_record(typ: &[u8], name: &[u8], tbl: &[u8], rootpage: u8, sql: &[u8]) -> Vec<u8> {
    // Serial types: text = 13 + 2*len; a small int (rootpage) = serial 1 (1 byte).
    let mut header = Vec::new();
    let mut body = Vec::new();
    for s in [typ, name, tbl] {
        push_varint(&mut header, (13 + 2 * s.len()) as u64);
        body.extend_from_slice(s);
    }
    push_varint(&mut header, 1); // rootpage: serial 1 (8-bit int)
    body.push(rootpage);
    push_varint(&mut header, (13 + 2 * sql.len()) as u64);
    body.extend_from_slice(sql);

    // header_len is a varint counting itself + the serial array.
    let mut header_len = header.len() + 1;
    while varint_len(header_len as u64) + header.len() != header_len {
        header_len += 1;
    }
    let mut record = Vec::new();
    push_varint(&mut record, header_len as u64);
    record.extend_from_slice(&header);
    record.extend_from_slice(&body);
    record
}

fn push_varint(out: &mut Vec<u8>, mut v: u64) {
    // SQLite varint: big-endian base-128, high bit set on all but the last byte;
    // sufficient here for the small values this fixture uses (< 2^28).
    let mut bytes = Vec::new();
    loop {
        bytes.push((v & 0x7f) as u8);
        v >>= 7;
        if v == 0 {
            break;
        }
    }
    bytes.reverse();
    for (i, byte) in bytes.iter().enumerate() {
        if i + 1 < bytes.len() {
            out.push(byte | 0x80);
        } else {
            out.push(*byte);
        }
    }
}

fn varint_len(v: u64) -> usize {
    let mut n = 1;
    let mut x = v >> 7;
    while x != 0 {
        n += 1;
        x >>= 7;
    }
    n
}
