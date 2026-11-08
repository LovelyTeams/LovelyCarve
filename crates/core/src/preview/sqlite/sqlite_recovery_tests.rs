//! Deleted-record recovery over databases written by SQLite / SQLCipher
//! itself (the bundled `rusqlite` build), loaded as folder evidence.

use std::path::Path;
use std::sync::Arc;

use rusqlite::Connection;

use crate::filesystem::evidence_session::EvidenceSession;
use crate::filesystem::tree::fs_tree::{children_for_image, FsNode};
use crate::preview::sqlite::sqlite_preview::{self, SqliteCell, SqliteKey};
use crate::preview::sqlite::sqlite_recovery::{recover, RecordOrigin, RecoveredRecord};

/// Rows `1..=count` of `messages(id INTEGER PRIMARY KEY, body TEXT, amount REAL)`;
/// every body embeds its id, so no two rows share content.
fn fill(connection: &Connection, count: i64) {
    connection
        .execute_batch(
            "CREATE TABLE messages (id INTEGER PRIMARY KEY, body TEXT NOT NULL, amount REAL);
             BEGIN;",
        )
        .unwrap();
    for id in 1..=count {
        connection
            .execute(
                "INSERT INTO messages VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    id,
                    format!(
                        "message {id:04} sent by user{} with some padding text",
                        id % 7
                    ),
                    id as f64 * 1.25
                ],
            )
            .unwrap();
    }
    connection.execute_batch("COMMIT;").unwrap();
}

/// Delete every third row; returns the deleted bodies.
fn delete_every_third(connection: &Connection) -> Vec<String> {
    let deleted: Vec<String> = connection
        .prepare("SELECT body FROM messages WHERE id % 3 = 0 ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    connection
        .execute_batch("DELETE FROM messages WHERE id % 3 = 0;")
        .unwrap();
    deleted
}

fn live_bodies(connection: &Connection) -> Vec<String> {
    connection
        .prepare("SELECT body FROM messages")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// The folder evidence's root directory node and the node of `name` in it.
fn evidence_nodes(image: &Arc<EvidenceSession>, name: &str) -> (String, FsNode) {
    let root = children_for_image(image, "root").unwrap().remove(0);
    let file = children_for_image(image, &root.id)
        .unwrap()
        .into_iter()
        .find(|node| node.name == name)
        .unwrap();
    (root.id, file)
}

fn body(record: &RecoveredRecord) -> Option<&str> {
    match record.values.get(1) {
        Some(SqliteCell::Text(text)) => Some(text.as_str()),
        _ => None,
    }
}

fn recovered_bodies(records: &[RecoveredRecord]) -> Vec<&str> {
    records.iter().filter_map(body).collect()
}

fn open_evidence(directory: &Path, name: &str) -> (Arc<EvidenceSession>, String, FsNode) {
    let image = Arc::new(EvidenceSession::open(directory).unwrap());
    let (parent, node) = evidence_nodes(&image, name);
    (image, parent, node)
}

#[test]
fn recovers_deleted_rows_and_never_live_ones() {
    let directory = tempfile::tempdir().unwrap();
    let connection = Connection::open(directory.path().join("chat.db")).unwrap();
    connection
        .execute_batch("PRAGMA secure_delete = OFF; PRAGMA journal_mode = DELETE;")
        .unwrap();
    fill(&connection, 300);
    let deleted = delete_every_third(&connection);
    let live = live_bodies(&connection);
    drop(connection);

    let (image, parent, node) = open_evidence(directory.path(), "chat.db");
    let recovery = recover(&image, &node.id, Some(&parent), &SqliteKey::default()).unwrap();

    let bodies = recovered_bodies(&recovery.records);
    assert!(bodies
        .iter()
        .all(|body| !live.iter().any(|live| live == body)));
    let found = deleted
        .iter()
        .filter(|body| bodies.contains(&body.as_str()))
        .count();
    assert!(
        found * 2 >= deleted.len(),
        "recovered {found} of {} deleted rows",
        deleted.len()
    );
    assert_eq!(recovery.cipher, None);
    assert!(recovery
        .records
        .iter()
        .all(|record| record.table.as_deref() != Some("sqlite_master")));
}

#[test]
fn uncheckpointed_wal_is_found_through_the_parent_directory() {
    let working = tempfile::tempdir().unwrap();
    let evidence = tempfile::tempdir().unwrap();
    let path = working.path().join("chat.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "PRAGMA secure_delete = OFF; PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;",
        )
        .unwrap();
    fill(&connection, 200);
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    let deleted = delete_every_third(&connection);
    // Copy while the connection still holds the uncheckpointed -wal.
    std::fs::copy(&path, evidence.path().join("chat.db")).unwrap();
    for sidecar in ["chat.db-wal", "chat.db-shm"] {
        std::fs::copy(working.path().join(sidecar), evidence.path().join(sidecar)).unwrap();
    }
    drop(connection);

    let (image, parent, node) = open_evidence(evidence.path(), "chat.db");
    let recovery = recover(&image, &node.id, Some(&parent), &SqliteKey::default()).unwrap();
    assert_eq!(recovery.sidecars, vec!["chat.db-wal".to_string()]);
    let from_wal: Vec<&str> = recovery
        .records
        .iter()
        .filter(|record| record.origin == RecordOrigin::WalFrame)
        .filter_map(body)
        .collect();
    assert!(!from_wal.is_empty());
    assert!(from_wal
        .iter()
        .all(|body| deleted.iter().any(|deleted| deleted == body)));

    // The preview finds the same sidecar through the directory, and SQLite
    // replays it: the deletions show. The -shm is not needed for that.
    let mut cache = None;
    let session =
        sqlite_preview::cached_session(&mut cache, &image, "evidence", &node.id, Some(&parent))
            .unwrap();
    let digest = sqlite_preview::preview_session(&session, &SqliteKey::default()).unwrap();
    assert_eq!(digest.sidecars, vec!["chat.db-wal".to_string()]);
    assert_eq!(digest.tables[0].row_count, Some(200 - deleted.len() as i64));
}

/// SQLite replays an encrypted WAL in the preview once the key is given; the
/// deleted-record view, which parses pages itself, leaves it out with a note.
#[test]
fn encrypted_wal_is_replayed_by_the_preview_and_noted_by_recovery() {
    let working = tempfile::tempdir().unwrap();
    let evidence = tempfile::tempdir().unwrap();
    let path = working.path().join("chat.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "PRAGMA key = 'evidence-key';
             PRAGMA secure_delete = OFF; PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;",
        )
        .unwrap();
    fill(&connection, 200);
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    let deleted = delete_every_third(&connection);
    for file in ["chat.db", "chat.db-wal"] {
        std::fs::copy(working.path().join(file), evidence.path().join(file)).unwrap();
    }
    drop(connection);

    let (image, parent, node) = open_evidence(evidence.path(), "chat.db");
    let key = SqliteKey {
        passphrase: Some("evidence-key".into()),
        ..SqliteKey::default()
    };
    let mut cache = None;
    let session =
        sqlite_preview::cached_session(&mut cache, &image, "evidence", &node.id, Some(&parent))
            .unwrap();
    let locked = sqlite_preview::preview_session(&session, &SqliteKey::default()).unwrap_err();
    assert!(format!("{locked:#}").starts_with("SQLITE_LOCKED"));
    let digest = sqlite_preview::preview_session(&session, &key).unwrap();
    assert!(digest.encrypted);
    assert_eq!(digest.sidecars, vec!["chat.db-wal".to_string()]);
    assert_eq!(digest.tables[0].row_count, Some(200 - deleted.len() as i64));

    let recovery = recover(&image, &node.id, Some(&parent), &key).unwrap();
    assert_eq!(recovery.sidecars, Vec::<String>::new());
    assert_eq!(
        recovery.notes,
        vec!["加密库的 -wal / -journal 也是密文，暂未参与恢复".to_string()]
    );
}

#[test]
fn persist_journal_yields_the_last_transactions_deleted_rows() {
    let directory = tempfile::tempdir().unwrap();
    let connection = Connection::open(directory.path().join("chat.db")).unwrap();
    connection
        .execute_batch("PRAGMA secure_delete = OFF; PRAGMA journal_mode = PERSIST;")
        .unwrap();
    fill(&connection, 120);
    let deleted = delete_every_third(&connection);
    drop(connection);

    let (image, parent, node) = open_evidence(directory.path(), "chat.db");
    let recovery = recover(&image, &node.id, Some(&parent), &SqliteKey::default()).unwrap();
    assert_eq!(recovery.sidecars, vec!["chat.db-journal".to_string()]);
    let mut from_journal: Vec<&str> = recovery
        .records
        .iter()
        .filter(|record| record.origin == RecordOrigin::JournalDeleted)
        .filter_map(body)
        .collect();
    from_journal.sort_unstable();
    let mut expected: Vec<&str> = deleted.iter().map(String::as_str).collect();
    expected.sort_unstable();
    assert_eq!(from_journal, expected);
}

/// Write an encrypted `chat.db` with SQLCipher itself; returns deleted and
/// live bodies.
fn encrypted_database(directory: &Path, setup: &str) -> (Vec<String>, Vec<String>) {
    let connection = Connection::open(directory.join("chat.db")).unwrap();
    connection.execute_batch(setup).unwrap();
    connection
        .execute_batch("PRAGMA secure_delete = OFF; PRAGMA journal_mode = DELETE;")
        .unwrap();
    fill(&connection, 200);
    let deleted = delete_every_third(&connection);
    let live = live_bodies(&connection);
    (deleted, live)
}

fn assert_carved_from_encrypted(
    directory: &Path,
    key: &SqliteKey,
    cipher: &str,
    deleted: &[String],
    live: &[String],
) {
    let (image, parent, node) = open_evidence(directory, "chat.db");
    let recovery = recover(&image, &node.id, Some(&parent), key).unwrap();
    assert_eq!(recovery.cipher.as_deref(), Some(cipher));
    let bodies = recovered_bodies(&recovery.records);
    assert!(bodies
        .iter()
        .all(|body| !live.iter().any(|live| live == body)));
    assert!(
        deleted.iter().any(|body| bodies.contains(&body.as_str())),
        "no deleted row recovered from the decrypted pages"
    );
}

#[test]
fn sqlcipher_4_database_is_carved_with_the_browser_key() {
    let directory = tempfile::tempdir().unwrap();
    let (deleted, live) = encrypted_database(directory.path(), "PRAGMA key = 'evidence-key';");
    let key = SqliteKey {
        passphrase: Some("evidence-key".into()),
        ..SqliteKey::default()
    };
    assert_carved_from_encrypted(
        directory.path(),
        &key,
        "SQLCipher 4 · 页大小 4096",
        &deleted,
        &live,
    );

    let (image, parent, node) = open_evidence(directory.path(), "chat.db");
    let missing = recover(&image, &node.id, Some(&parent), &SqliteKey::default()).unwrap_err();
    assert!(format!("{missing:#}").starts_with("SQLITE_LOCKED"));
    let wrong = SqliteKey {
        passphrase: Some("not-the-key".into()),
        ..SqliteKey::default()
    };
    let wrong = recover(&image, &node.id, Some(&parent), &wrong).unwrap_err();
    assert!(format!("{wrong:#}").starts_with("SQLITE_LOCKED"));
}

/// `WeChat` Android style: SQLCipher 1 parameters (no page HMAC, 4 000 KDF
/// iterations, 1 KiB pages), recovered with explicit parameters and by detection.
#[test]
fn sqlcipher_1_database_is_carved_explicitly_and_by_detection() {
    let directory = tempfile::tempdir().unwrap();
    let (deleted, live) = encrypted_database(
        directory.path(),
        "PRAGMA key = '1a2b3c4'; PRAGMA cipher_compatibility = 1;",
    );
    let explicit = SqliteKey {
        passphrase: Some("1a2b3c4".into()),
        compatibility: Some(1),
        page_size: Some(1024),
        kdf_iter: Some(4000),
        ..SqliteKey::default()
    };
    assert_carved_from_encrypted(
        directory.path(),
        &explicit,
        "SQLCipher 1 · 页大小 1024",
        &deleted,
        &live,
    );
    let detected = SqliteKey {
        passphrase: Some("1a2b3c4".into()),
        ..SqliteKey::default()
    };
    assert_carved_from_encrypted(
        directory.path(),
        &detected,
        "SQLCipher 1 · 页大小 1024",
        &deleted,
        &live,
    );
}
