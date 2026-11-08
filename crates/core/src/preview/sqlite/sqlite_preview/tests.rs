use super::*;

fn session_from_bytes(bytes: Vec<u8>) -> SqliteSession {
    let main_name = format!(
        "lovelycarve-sqlite-test-{}.sqlite",
        uuid::Uuid::new_v4().simple()
    );
    sqlite_vfs::import_file(&main_name, &bytes).expect("import test db into vfs");
    SqliteSession {
        image_id: "test-image".to_string(),
        node_id: "test-node".to_string(),
        main_name,
        sidecars: Vec::new(),
        bytes_copied: bytes.len() as u64,
        truncated: false,
        head16: head16_from_bytes(&bytes),
    }
}

#[test]
fn detects_sqlite_magic() {
    assert!(looks_like_sqlite(b"SQLite format 3\0extra"));
    assert!(!looks_like_sqlite(b"not a db at all"));
    assert!(!looks_like_sqlite(b"SQLite format 3"));
}

#[test]
fn truncate_text_keeps_short_strings() {
    let s = "hello";
    assert_eq!(truncate_text(s), "hello");
}

#[test]
fn truncate_text_caps_long_strings() {
    let long: String = "x".repeat(MAX_CELL_TEXT + 50);
    let truncated = truncate_text(&long);
    assert!(truncated.ends_with('…'));
    assert!(truncated.chars().count() <= MAX_CELL_TEXT + 1);
}

#[test]
fn quote_ident_doubles_internal_quotes() {
    assert_eq!(quote_ident("normal"), "\"normal\"");
    assert_eq!(quote_ident("we\"ird"), "\"we\"\"ird\"");
}

#[test]
fn sanitise_replaces_unsafe_chars() {
    assert_eq!(sanitise_filename("foo:bar/baz.db"), "foo_bar_baz.db");
    assert_eq!(sanitise_filename(""), "preview.sqlite");
}

/// Build a fresh SQLite file on disk, then run [`read_digest`]
/// against it and assert we surface the expected schema, row
/// counts, and a NULL-aware sample. This is the only end-to-end
/// path that exercises the rusqlite plumbing without a full
/// [`EvidenceSession`] — the [`preview_sqlite`] entry point can't be
/// covered by a unit test because it needs a mounted forensic
/// image.
#[test]
fn read_digest_reports_schema_and_samples() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("fixture.sqlite");
    let conn = Connection::open(&path).expect("open writable");
    conn.execute_batch(
        "CREATE TABLE messages (id INTEGER PRIMARY KEY, body TEXT NOT NULL, attachment BLOB);
         CREATE INDEX idx_messages_body ON messages(body);
         INSERT INTO messages (id, body, attachment) VALUES (1, 'hello', NULL);
         INSERT INTO messages (id, body, attachment) VALUES (2, 'world', X'DEADBEEF');",
    )
    .expect("seed");
    drop(conn);

    let digest = read_digest_path(&path, &SqliteKey::default()).expect("read digest");
    assert_eq!(digest.tables.len(), 1);
    let table = &digest.tables[0];
    assert_eq!(table.name, "messages");
    assert_eq!(table.kind, "table");
    assert_eq!(table.row_count, Some(2));
    assert_eq!(table.columns.len(), 3);
    assert!(table
        .columns
        .iter()
        .any(|col| col.name == "id" && col.pk == 1));
    assert_eq!(table.sample_rows.len(), 2);

    let null_attachment = matches!(table.sample_rows[0][2], SqliteCell::Null);
    let blob_attachment = matches!(table.sample_rows[1][2], SqliteCell::Blob { size: 4 });
    assert!(null_attachment, "row 1 attachment should be NULL");
    assert!(blob_attachment, "row 2 attachment should be a 4-byte BLOB");

    assert_eq!(digest.indexes.len(), 1);
    assert_eq!(digest.indexes[0].name, "idx_messages_body");
    assert_eq!(digest.indexes[0].table, "messages");
}

#[test]
fn read_digest_reports_schema_via_preview_vfs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("fixture.sqlite");
    let conn = Connection::open(&path).expect("open writable");
    conn.execute_batch(
        "CREATE TABLE messages (id INTEGER PRIMARY KEY, body TEXT);
         INSERT INTO messages (id, body) VALUES (1, 'hello');",
    )
    .expect("seed");
    drop(conn);

    let bytes = std::fs::read(&path).expect("read fixture bytes");
    let session = session_from_bytes(bytes);
    let digest = read_digest(&session, &SqliteKey::default()).expect("read digest via vfs");
    assert_eq!(digest.tables.len(), 1);
    assert_eq!(digest.tables[0].name, "messages");
    assert_eq!(digest.tables[0].row_count, Some(1));
}

/// A cleanly closed WAL database keeps its WAL-mode header (bytes 18/19
/// = 2) with no `-wal` beside it. The preview VFS has no shared memory,
/// so it opens only through the heap wal-index.
#[test]
fn wal_mode_header_without_a_wal_opens_via_preview_vfs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("wal.sqlite");
    let conn = Connection::open(&path).expect("open writable");
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE messages (id INTEGER PRIMARY KEY, body TEXT);
         INSERT INTO messages (id, body) VALUES (1, 'hello');",
    )
    .expect("seed");
    drop(conn);

    let bytes = std::fs::read(&path).expect("read fixture bytes");
    assert_eq!(bytes[18..20], [2, 2]);
    let session = session_from_bytes(bytes);
    let wal_name = wal_vfs_name(&session.main_name);
    let digest = read_digest(&session, &SqliteKey::default()).expect("WAL-mode header opens");
    assert_eq!(digest.tables[0].row_count, Some(1));
    assert_eq!(digest.header.journal_mode.as_deref(), Some("wal"));
    drop(session);
    assert!(
        !sqlite_vfs::exists(&wal_name),
        "the WAL SQLite created goes with the session"
    );
}

#[test]
fn recursive_view_is_bounded_in_digest_and_paged_reads() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("recursive.sqlite");
    let connection = Connection::open(&path).expect("open");
    connection
        .execute_batch(
            "CREATE VIEW endless AS WITH RECURSIVE n(x) AS (
            SELECT 1 UNION ALL SELECT x+1 FROM n
         ) SELECT x FROM n ORDER BY x DESC;",
        )
        .expect("view");
    drop(connection);
    let session = session_from_bytes(std::fs::read(path).expect("bytes"));
    let error = read_digest(&session, &SqliteKey::default()).expect_err("bounded digest");
    assert!(error.to_string().contains("超过执行限制"));
    let error = read_rows(
        &session,
        &SqliteKey::default(),
        "endless",
        0,
        20,
        None,
        false,
    )
    .expect_err("bounded COUNT and page");
    assert!(error.to_string().contains("超过执行限制"));
}

#[test]
fn clearing_cache_keeps_active_reader_alive_until_it_finishes() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("reader.sqlite");
    let connection = Connection::open(&path).expect("open");
    connection
        .execute_batch("CREATE TABLE data(value); INSERT INTO data VALUES(42);")
        .expect("seed");
    drop(connection);
    let session = Arc::new(session_from_bytes(std::fs::read(path).expect("bytes")));
    let name = session.main_name.clone();
    let mut cache = Some(Arc::clone(&session));
    drop(cache.take());
    assert!(sqlite_vfs::exists(&name));
    let page = table_rows_for_session(&session, &SqliteKey::default(), "data", 0, 20, None, false)
        .expect("reader survives eviction");
    assert_eq!((page.total, page.returned), (1, 1));
    drop(session);
    assert!(!sqlite_vfs::exists(&name), "last reader removes VFS files");
}

/// Exercise the paged-rows path directly: a seeded table read back a
/// page at a time, with sort and an injection-shaped `order_by` that
/// must be ignored rather than executed.
#[test]
fn read_rows_pages_sorts_and_rejects_bad_order_by() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("paged.sqlite");
    let conn = Connection::open(&path).expect("open writable");
    conn.execute_batch(
        "CREATE TABLE nums (id INTEGER PRIMARY KEY, label TEXT);
         INSERT INTO nums (id, label) VALUES (1,'a'),(2,'b'),(3,'c'),(4,'d'),(5,'e');",
    )
    .expect("seed");
    drop(conn);

    // First page of 2, default order.
    let key = SqliteKey::default();
    let page = read_rows_path(&path, &key, "nums", 0, 2, None, false).expect("page 0");
    assert_eq!(page.total, 5);
    assert_eq!(page.offset, 0);
    assert_eq!(page.returned, 2);
    assert!(matches!(page.rows[0][0], SqliteCell::Integer(1)));

    // Second page picks up where the first left off.
    let page2 = read_rows_path(&path, &key, "nums", 2, 2, None, false).expect("page 1");
    assert_eq!(page2.offset, 2);
    assert!(matches!(page2.rows[0][0], SqliteCell::Integer(3)));

    // Descending sort by a real column.
    let desc = read_rows_path(&path, &key, "nums", 0, 2, Some("id"), true).expect("desc");
    assert!(matches!(desc.rows[0][0], SqliteCell::Integer(5)));

    // Bogus order_by column is ignored (not injected) — query still
    // succeeds and returns rows in unsorted order.
    let injected = read_rows_path(
        &path,
        &key,
        "nums",
        0,
        5,
        Some("id; DROP TABLE nums"),
        false,
    )
    .expect("bad order_by must be ignored, not executed");
    assert_eq!(injected.total, 5);
    // Table survived — the injection string never reached SQL.
    let after = read_rows_path(&path, &key, "nums", 0, 5, None, false).expect("table intact");
    assert_eq!(after.total, 5);
}

/// End-to-end SQLCipher round-trip: build an encrypted database, then
/// confirm the right passphrase decrypts it, a wrong/absent key is
/// reported as `SQLITE_LOCKED`, and a plaintext DB reads as
/// unencrypted. Exercises the real codec (this only links when the
/// `bundled-sqlcipher-*` feature is on, which the crate always sets).
#[test]
fn encrypted_roundtrip_passphrase_and_rawkey() {
    let tmp = tempfile::tempdir().expect("tempdir");

    // ── passphrase-encrypted DB (default SQLCipher 4 params) ──
    let enc = tmp.path().join("enc.sqlite");
    let conn = Connection::open(&enc).expect("open writable");
    conn.execute_batch(
        "PRAGMA key = 'open sesame';
         CREATE TABLE secret (id INTEGER PRIMARY KEY, note TEXT);
         INSERT INTO secret (id, note) VALUES (1, 'hello'), (2, 'world');",
    )
    .expect("seed encrypted");
    drop(conn);

    // Header must NOT be the plaintext magic.
    assert!(
        !read_head16(&enc).is_some_and(|h| looks_like_sqlite(&h)),
        "encrypted file must not look plain"
    );

    // Correct passphrase → decrypts, flagged encrypted.
    let good = SqliteKey {
        passphrase: Some("open sesame".to_string()),
        ..Default::default()
    };
    let digest = read_digest_path(&enc, &good).expect("correct passphrase decrypts");
    assert!(digest.encrypted, "digest must report encrypted");
    assert_eq!(digest.tables.len(), 1);
    assert_eq!(digest.tables[0].name, "secret");
    assert_eq!(digest.tables[0].row_count, Some(2));

    let enc_bytes = std::fs::read(&enc).expect("read encrypted fixture");
    let enc_session = session_from_bytes(enc_bytes);
    let vfs_digest = read_digest(&enc_session, &good).expect("VFS passphrase decrypts");
    assert!(vfs_digest.encrypted);
    assert_eq!(vfs_digest.tables[0].name, "secret");

    // No key → SQLITE_LOCKED (the frontend signal to prompt).
    let no_key = read_digest_path(&enc, &SqliteKey::default());
    let locked_msg = no_key.expect_err("missing key must fail").to_string();
    assert!(
        locked_msg.contains("SQLITE_LOCKED"),
        "missing key error must carry SQLITE_LOCKED, got: {locked_msg}"
    );

    // Wrong key → SQLITE_LOCKED too.
    let wrong = SqliteKey {
        passphrase: Some("nope".to_string()),
        ..Default::default()
    };
    let wrong_msg = read_digest_path(&enc, &wrong)
        .expect_err("wrong key must fail")
        .to_string();
    assert!(
        wrong_msg.contains("SQLITE_LOCKED"),
        "wrong key must carry SQLITE_LOCKED"
    );

    // Paging an encrypted table with the right key works.
    let page = read_rows_path(&enc, &good, "secret", 0, 10, None, false).expect("paged read");
    assert_eq!(page.total, 2);
    assert_eq!(page.returned, 2);

    // ── raw-key-encrypted DB (64 hex chars = 32-byte key) ──
    let raw = tmp.path().join("raw.sqlite");
    let hexkey = "2DD29CA851E7B56E4697B0E1F08507293D761A05CE4D1B628663F411A8086D99";
    let conn = Connection::open(&raw).expect("open writable raw");
    conn.execute_batch(&format!(
        "PRAGMA key = \"x'{hexkey}'\";
         CREATE TABLE t (a INTEGER);
         INSERT INTO t VALUES (42);"
    ))
    .expect("seed raw-key encrypted");
    drop(conn);

    let raw_key = SqliteKey {
        raw_hex_key: Some(hexkey.to_string()),
        ..Default::default()
    };
    let raw_digest = read_digest_path(&raw, &raw_key).expect("raw key decrypts");
    assert!(raw_digest.encrypted);
    assert_eq!(raw_digest.tables[0].name, "t");

    // ── plaintext DB reads as unencrypted, key ignored ──
    let plain = tmp.path().join("plain.sqlite");
    let conn = Connection::open(&plain).expect("open writable plain");
    conn.execute_batch("CREATE TABLE p (x); INSERT INTO p VALUES (1);")
        .expect("seed plain");
    drop(conn);
    assert!(read_head16(&plain).is_some_and(|h| looks_like_sqlite(&h)));
    let plain_digest =
        read_digest_path(&plain, &SqliteKey::default()).expect("plain reads without key");
    assert!(
        !plain_digest.encrypted,
        "plain DB must not be flagged encrypted"
    );
}

/// An empty / sub-16-byte file is reported as "not a database", NOT as
/// `SQLITE_LOCKED` — otherwise the UI would prompt for a decryption key
/// on a damaged fragment or zero-length placeholder.
#[test]
fn too_small_file_is_not_a_database_not_locked() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let empty = tmp.path().join("empty.db");
    std::fs::write(&empty, b"").expect("write empty");
    let err = read_digest_path(&empty, &SqliteKey::default())
        .expect_err("empty must fail")
        .to_string();
    assert!(
        !err.contains("SQLITE_LOCKED"),
        "empty file must not prompt for a key: {err}"
    );
    assert!(
        err.contains("不是 SQLite 数据库"),
        "unexpected message: {err}"
    );

    // A few bytes, still under the 16-byte header → same treatment.
    let tiny = tmp.path().join("tiny.db");
    std::fs::write(&tiny, b"abc").expect("write tiny");
    let err = read_digest_path(&tiny, &SqliteKey::default())
        .expect_err("tiny must fail")
        .to_string();
    assert!(
        !err.contains("SQLITE_LOCKED"),
        "tiny file must not prompt for a key: {err}"
    );
}
