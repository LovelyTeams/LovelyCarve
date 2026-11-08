//! Regression tests for the `LovelyCarve` changes to the ported recovery pass
//! (`src/recovery.rs`). The fixtures under `tests/data/lovelycarve/` come from
//! `make_corpus.py` there (Python `sqlite3`, SQLite 3.37.2, seed 20261003); every
//! row's text embeds its own number, so no deleted row equals a live one.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use sqlite_core::recovery::{
    attribute_records, carve_all_deleted_records, Attribution, RecoverySource,
};
use sqlite_core::{Database, Value};

const AUTOVACUUM: &[u8] = include_bytes!("data/lovelycarve/autovacuum.db");
const DROP_TABLE: &[u8] = include_bytes!("data/lovelycarve/drop_table.db");

fn without_first_column(values: &[Value]) -> &[Value] {
    values.get(1..).unwrap_or_default()
}

/// `auto_vacuum = FULL` moved pages after the deletions, leaving freeblock copies
/// of live rows whose rowid (and so their INTEGER PRIMARY KEY) was destroyed.
/// Upstream's exact-value guard missed four of them; none may be reported.
#[test]
fn stale_copies_of_live_rows_are_not_reported_as_deleted() {
    let db = Database::open(AUTOVACUUM.to_vec()).unwrap();
    let live: Vec<Vec<Value>> = db
        .live_table_rows()
        .into_iter()
        .flat_map(|table| table.rows.into_iter().map(|row| row.values))
        .collect();
    let records = carve_all_deleted_records(&db);
    assert!(
        records.len() > 100,
        "the fixture still yields its deleted rows"
    );
    let resurfaced: Vec<_> = records
        .iter()
        .filter(|record| {
            live.iter()
                .any(|row| without_first_column(row) == without_first_column(&record.values))
        })
        .map(|record| (record.page, record.offset))
        .collect();
    assert_eq!(resurfaced, vec![]);
}

/// Rows of a dropped table share rowids with the surviving table. Upstream
/// labelled them prior versions of the surviving rows; they are freed-page
/// records, attributed by shape (or not at all), never prior versions.
#[test]
fn dropped_table_rows_are_not_prior_versions_of_another_table() {
    let db = Database::open(DROP_TABLE.to_vec()).unwrap();
    let records = carve_all_deleted_records(&db);
    let attributions = attribute_records(&db, &records);
    let contacts: Vec<_> = records
        .iter()
        .zip(&attributions)
        .filter(|(record, _)| {
            matches!(record.values.get(2), Some(Value::Text(phone)) if phone.starts_with("+86-139"))
        })
        .collect();
    assert!(
        contacts.len() >= 100,
        "dropped contacts are recovered: {}",
        contacts.len()
    );
    for (record, attribution) in &contacts {
        assert_eq!(record.source, RecoverySource::FreelistPage, "{record:?}");
        assert_ne!(attribution, &&Attribution::Known("messages".into()));
    }
    assert!(records
        .iter()
        .all(|record| record.source != RecoverySource::PriorVersion));
}
