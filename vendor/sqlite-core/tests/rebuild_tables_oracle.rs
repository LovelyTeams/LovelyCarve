//! `sqlite3` oracle for the generalized multi-table rebuild writer
//! ([`build_recovered_db_tables`]): arbitrary table names + arbitrary column
//! names (SQL-identifier-quoted) must produce a database the real `sqlite3`
//! engine reads back with the exact schema we asked for.
//!
//! Gated on a `sqlite3` binary (PATH or `SQLITE3_BIN`); skips when absent.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::process::Command;

use sqlite_core::rebuild::{build_recovered_db_tables, RecoveredTable};
use sqlite_core::Value;

fn sqlite3_bin() -> Option<String> {
    let bin = std::env::var("SQLITE3_BIN").unwrap_or_else(|_| "sqlite3".to_string());
    Command::new(&bin)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| bin)
}

fn temp_db(bytes: &[u8], tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    p.push(format!("sqlite4n6_rtbl_{tag}_{nonce}.db"));
    std::fs::write(&p, bytes).unwrap();
    p
}

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
fn arbitrary_table_and_column_names_round_trip_through_sqlite3() {
    let Some(bin) = sqlite3_bin() else {
        eprintln!("SKIP arbitrary_table_and_column_names_round_trip_through_sqlite3: no sqlite3");
        return;
    };

    let tables = vec![
        // A Tier-1 attributed table with real column names, including a name
        // that needs quoting ("first name") and a SQLite keyword ("order").
        RecoveredTable {
            name: "recovered_people".to_string(),
            columns: vec![
                "_page".into(),
                "_offset".into(),
                "_rowid".into(),
                "_source".into(),
                "_confidence".into(),
                "first name".into(),
                "order".into(),
            ],
            rows: vec![vec![
                Value::Integer(3),
                Value::Integer(128),
                Value::Integer(5),
                Value::Text("in-page-freeblock".into()),
                Value::Real(0.72),
                Value::Text("alice".into()),
                Value::Integer(7),
            ]],
        },
        // The Tier-2 inferred table with its guess + ambiguity flag.
        RecoveredTable {
            name: "recovered_inferred".to_string(),
            columns: vec![
                "_page".into(),
                "_offset".into(),
                "_rowid".into(),
                "_source".into(),
                "_confidence".into(),
                "_table_guess".into(),
                "_table_match_ambiguous".into(),
                "c0".into(),
                "c1".into(),
            ],
            rows: vec![vec![
                Value::Integer(9),
                Value::Integer(0),
                Value::Null,
                Value::Text("freelist-page".into()),
                Value::Real(0.9),
                Value::Text("people".into()),
                Value::Integer(0),
                Value::Integer(1),
                Value::Text("bob".into()),
            ]],
        },
        // Tier-3 unattributed.
        RecoveredTable {
            name: "recovered_unattributed".to_string(),
            columns: vec![
                "_page".into(),
                "_offset".into(),
                "_rowid".into(),
                "_source".into(),
                "_confidence".into(),
                "c0".into(),
            ],
            rows: vec![vec![
                Value::Integer(2),
                Value::Integer(64),
                Value::Integer(11),
                Value::Text("dropped-table".into()),
                Value::Real(0.5),
                Value::Integer(99),
            ]],
        },
    ];

    let bytes = build_recovered_db_tables(&tables);
    let db = temp_db(&bytes, "arbitrary");

    // External engine vouches for structural integrity.
    assert_eq!(run_sql(&bin, &db, "PRAGMA integrity_check;"), "ok");

    // Real column names survived quoting.
    assert_eq!(
        run_sql(
            &bin,
            &db,
            r#"SELECT "first name", "order" FROM recovered_people;"#
        ),
        "alice|7"
    );

    // Tier-2 guess + ambiguity flag columns exist and carry the values.
    assert_eq!(
        run_sql(
            &bin,
            &db,
            "SELECT _table_guess, _table_match_ambiguous FROM recovered_inferred;"
        ),
        "people|0"
    );

    // Tier-3 table present and queryable.
    assert_eq!(
        run_sql(&bin, &db, "SELECT count(*) FROM recovered_unattributed;"),
        "1"
    );

    let _ = std::fs::remove_file(&db);
}
