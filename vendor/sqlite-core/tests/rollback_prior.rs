//! `Database::rollback_prior` + [`PriorSnapshot`] — the pre-transaction state.
//!
//! A rollback journal carries the page images as they were BEFORE the last
//! transaction. Overlaying them on the main db materializes the prior snapshot:
//! the database one step in the past. NIST CFReDS SFT-03 PERSIST deleted 100 of
//! 2240 `invoice_items` rows; the prior snapshot must hold all 2240 (the
//! pre-delete state), so reading `invoice_items` from the prior snapshot is the
//! known-answer oracle for the Tier-B (zeroed-header) reconstruction.
//!
//! Secure-by-design: `rollback_prior` returns a distinct [`PriorSnapshot`], not a
//! `Database`, so a prior/deleted row can never be read as "live", and it errors
//! when applied to a WAL-applied database (the two timelines are exclusive).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::pedantic)]

use std::path::{Path, PathBuf};

use sqlite_core::{Database, Error};

fn cfreds(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/cfreds")
        .join(name)
}

/// Read every row of the table named `table` from a prior snapshot, returning
/// its row count, by resolving its rootpage + arity from the prior schema.
fn prior_table_rows(snapshot: &sqlite_core::PriorSnapshot, table: &str) -> usize {
    let Some(t) = snapshot.tables().into_iter().find(|t| t.name == table) else {
        return 0;
    };
    snapshot
        .read_table(t.rootpage, t.columns.len())
        .map(|rows| rows.len())
        .unwrap_or(0)
}

#[test]
fn prior_snapshot_holds_pre_delete_invoice_items() {
    // SFT-03 PERSIST: 2140 rows live now, 2240 before the last transaction.
    for platform in ["ios", "android"] {
        let main = std::fs::read(cfreds(&format!("SFT-03_PERSIST_{platform}.sqlite"))).unwrap();
        let journal =
            std::fs::read(cfreds(&format!("SFT-03_PERSIST_{platform}.sqlite-journal"))).unwrap();

        let db = Database::open(main).expect("open main");
        // Sanity: the live db is the post-delete state.
        let live = db
            .live_table_rows()
            .iter()
            .find(|d| d.name == "invoice_items")
            .map_or(0, |d| d.rows.len());
        assert_eq!(live, 2140, "{platform}: live db is the post-delete state");

        let prior = db
            .rollback_prior(&journal)
            .expect("materialize prior snapshot");
        assert_eq!(
            prior_table_rows(&prior, "invoice_items"),
            2240,
            "{platform}: the prior snapshot holds all 2240 pre-delete rows"
        );
        // The last transaction DELETED rows (it did not grow the file), so no
        // journal page number exceeds the current main-db page count.
        assert!(
            !prior.grew_db(),
            "{platform}: a delete-only transaction does not grow the database"
        );
    }
}

/// A `notes` table fixture whose one ~12 KB row spills onto an overflow chain
/// (see `core/tests/overflow.rs`). Reading it THROUGH a PriorSnapshot exercises
/// the snapshot's overflow path (usable-size + page-bound + chain reassembly).
const OVERFLOW_DB: &[u8] = include_bytes!("data/overflow.db");
const NOTES_ROOT: u32 = 2;
const NOTES_COLS: usize = 2;

#[test]
fn prior_snapshot_reassembles_overflow_rows() {
    // An empty (zeroed) journal means prior == live: the snapshot overlays no
    // pages, so reading through it must still reassemble the spilled row exactly
    // as the live read does — covering PriorSnapshot's overflow page-source path.
    let db = Database::open(OVERFLOW_DB.to_vec()).expect("open overflow.db");
    let prior = db
        .rollback_prior(&[0u8; 512])
        .expect("empty journal → prior == live");

    let via_pages = prior
        .read_table_with_pages(NOTES_ROOT, NOTES_COLS)
        .expect("walk notes with pages");
    assert_eq!(via_pages.len(), 2, "two rows including the spilled one");
    let spilled = via_pages.iter().find(|(rowid, _, _)| *rowid == 1).unwrap();
    if let sqlite_core::Value::Text(body) = &spilled.1[1] {
        assert_eq!(body.len(), 12017, "the spilled body reassembles fully");
    } else {
        panic!("expected a TEXT body");
    }

    // The plain read_table must agree (same overflow path, no per-page leaf).
    let plain = prior
        .read_table(NOTES_ROOT, NOTES_COLS)
        .expect("walk notes");
    assert_eq!(plain.len(), 2);
}

#[test]
fn rollback_prior_rejects_wal_applied_database() {
    // WAL and rollback-journal modes are mutually exclusive timelines.
    let main = std::fs::read(cfreds("sft-03-WAL_ios.sqlite")).unwrap();
    let wal = std::fs::read(cfreds("sft-03-WAL_ios.sqlite-wal")).unwrap();
    let db = Database::open_with_wal(main, &wal).expect("open_with_wal");
    assert!(db.wal_applied());

    let journal = std::fs::read(cfreds("SFT-03_PERSIST_ios.sqlite-journal")).unwrap();
    let err = db
        .rollback_prior(&journal)
        .expect_err("a WAL-applied db must not accept a rollback journal");
    assert_eq!(err, Error::JournalModeConflict);
}
