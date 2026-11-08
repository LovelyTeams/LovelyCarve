//! Freelist-walk validation against a REAL SQLite `.db` built with the `sqlite3`
//! CLI / Python `sqlite3` module (see `docs/corpus-catalog.md` for the exact
//! generator). The fixture has `secure_delete=OFF` and a contiguous high-id
//! `DELETE` that frees whole leaf pages onto the freelist.
//!
//! Ground truth (cross-checked with `PRAGMA freelist_count`/`page_count`):
//!   live rows = 200 (ids 1..=200), deleted ids 201..=400, `freelist_count` = 5,
//!   `page_count` = 13.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use sqlite_core::Database;

const DB: &[u8] = include_bytes!("data/deleted_places.db");

#[test]
fn freelist_pages_match_pragma_freelist_count() {
    let db = Database::open(DB.to_vec()).expect("open deleted_places.db");
    let free = db.freelist_pages().expect("walk freelist");
    // `PRAGMA freelist_count` on this fixture reports 5 free pages.
    assert_eq!(free.len(), 5, "freelist page count must match PRAGMA");
    // Every freed page number is in-range for the 13-page file.
    for &p in &free {
        assert!((1..=13).contains(&p), "free page {p} out of file range");
    }
}

#[test]
fn page_count_matches_header_and_file() {
    let db = Database::open(DB.to_vec()).expect("open");
    // In-header page count (offset 28) == file_len / page_size == 13.
    assert_eq!(db.page_count(), 13);
}

#[test]
fn empty_freelist_on_clean_db() {
    // The original spike fixture has no deletions → empty freelist.
    let clean: &[u8] = include_bytes!("data/places.db");
    let db = Database::open(clean.to_vec()).expect("open clean");
    assert!(
        db.freelist_pages().expect("walk").is_empty(),
        "a never-deleted DB must have an empty freelist"
    );
}
