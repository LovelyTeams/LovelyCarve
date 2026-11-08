//! No-WAL degradation test for [`Database::row_histories`] (no sqlite3 needed).
//!
//! Opened WITHOUT a `-wal`, `wal_timeline()` is `None`, so there is no historical
//! state: the history must degrade cleanly to exactly the LIVE rows, each a single
//! `PresentInFinalView` / `VersionOrigin::Live` version with `commit_seq: None`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use sqlite_core::row_history::{VersionOrigin, ViewState};
use sqlite_core::Database;

#[test]
fn no_wal_degrades_to_live_rows_only() {
    let db_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/deleted_places.db");
    let db = Database::open(std::fs::read(&db_path).unwrap()).unwrap();
    assert!(!db.wal_applied(), "fixture opened without a -wal overlay");

    let histories = db.row_histories();
    assert!(
        !histories.is_empty(),
        "the live database has at least one user table"
    );

    // Every version is a live, present, unpositioned current row — no history.
    let live = db.live_table_rows();
    for h in &histories {
        if h.without_rowid {
            assert!(
                h.versions.is_empty(),
                "WITHOUT ROWID tables emit no rowid-keyed versions"
            );
            continue;
        }
        let live_count = live
            .iter()
            .find(|t| t.name == h.table)
            .map_or(0, |t| t.rows.len());
        let present: Vec<_> = h
            .versions
            .iter()
            .filter(|v| matches!(v.origin, VersionOrigin::Live))
            .collect();
        assert_eq!(
            present.len(),
            live_count,
            "table {} has one live version per live row",
            h.table
        );
        for v in &h.versions {
            assert_eq!(
                v.origin,
                VersionOrigin::Live,
                "no -wal => every version is Live"
            );
            assert_eq!(v.view_state, ViewState::PresentInFinalView);
            assert_eq!(v.commit_seq, None, "live rows carry no commit_seq");
            assert!(!v.is_deleted);
            assert!(!v.rowid_reused);
        }
    }
}
