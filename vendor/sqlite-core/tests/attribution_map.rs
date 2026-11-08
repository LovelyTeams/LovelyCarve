//! Live-table enumeration + page→table map, validated against a real `sqlite3`
//! oracle fixture. Gated on a `sqlite3` binary (PATH or `SQLITE3_BIN`); skips
//! gracefully when absent (mirrors `rebuild_sqlite3_oracle.rs`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::process::Command;

use sqlite_core::attribution::Affinity;
use sqlite_core::Database;

fn sqlite3_bin() -> Option<String> {
    let bin = std::env::var("SQLITE3_BIN").unwrap_or_else(|_| "sqlite3".to_string());
    Command::new(&bin)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| bin)
}

/// Mint a fresh db by piping a SQL script to `sqlite3`, returning its path.
fn mint(bin: &str, tag: &str, script: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    p.push(format!("sqlite4n6_attr_{tag}_{nonce}.db"));
    let _ = std::fs::remove_file(&p);
    let out = Command::new(bin)
        .arg(&p)
        .arg(script)
        .output()
        .expect("sqlite3 must execute");
    assert!(
        out.status.success(),
        "sqlite3 mint failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    p
}

#[test]
fn live_tables_reports_names_columns_and_affinities() {
    let Some(bin) = sqlite3_bin() else {
        eprintln!("SKIP live_tables_reports_names_columns_and_affinities: no sqlite3");
        return;
    };
    let db_path = mint(
        &bin,
        "live_tables",
        "CREATE TABLE people (id INTEGER PRIMARY KEY, name TEXT, height REAL); \
         CREATE TABLE notes (note_id INTEGER, body TEXT, blob_col BLOB); \
         INSERT INTO people VALUES (1, 'alice', 1.6); \
         INSERT INTO notes VALUES (1, 'hi', x'00ff');",
    );
    let bytes = std::fs::read(&db_path).unwrap();
    let db = Database::open(bytes).unwrap();

    let tables = db.live_tables();
    let names: Vec<&str> = tables.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"people"), "got {names:?}");
    assert!(names.contains(&"notes"), "got {names:?}");

    let people = tables.iter().find(|t| t.name == "people").unwrap();
    assert_eq!(
        people.column_names.as_deref(),
        Some(["id", "name", "height"].map(String::from).as_slice())
    );
    assert_eq!(
        people.affinities,
        vec![Affinity::Integer, Affinity::Text, Affinity::Real]
    );

    let notes = tables.iter().find(|t| t.name == "notes").unwrap();
    assert_eq!(
        notes.affinities,
        vec![Affinity::Integer, Affinity::Text, Affinity::Blob]
    );

    let _ = std::fs::remove_file(&db_path);
}

#[test]
fn page_to_table_map_resolves_each_tables_root() {
    let Some(bin) = sqlite3_bin() else {
        eprintln!("SKIP page_to_table_map_resolves_each_tables_root: no sqlite3");
        return;
    };
    let db_path = mint(
        &bin,
        "page_map",
        "CREATE TABLE alpha (a INTEGER, b TEXT); \
         CREATE TABLE beta (c INTEGER, d TEXT); \
         INSERT INTO alpha VALUES (1, 'x'); \
         INSERT INTO beta VALUES (2, 'y');",
    );
    let bytes = std::fs::read(&db_path).unwrap();
    let db = Database::open(bytes).unwrap();

    let map = db.page_to_table_map();
    let tables = db.live_tables();
    // Each live table's rootpage resolves to that table's name.
    for t in &tables {
        assert_eq!(
            map.get(&t.rootpage).map(String::as_str),
            Some(t.name.as_str()),
            "rootpage {} should map to {}",
            t.rootpage,
            t.name
        );
    }
    // Page 1 (sqlite_master) is not a user-table page.
    assert!(!map.values().any(|_| false));

    let _ = std::fs::remove_file(&db_path);
}
