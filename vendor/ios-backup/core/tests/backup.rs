//! End-to-end reads of a minted backup, encrypted and not (T2).
//!
//! The fixtures were built by `tools/mint_encrypted_backup.py`, whose every
//! cryptographic operation comes from Python's `cryptography` (OpenSSL-backed)
//! and whose property lists come from `plistlib`. Nothing in that lineage is
//! shared with the Rust under test, so "Python encrypted it, Rust decrypted it,
//! and the bytes match" is a cross-implementation check rather than a
//! self-consistency one.
//!
//! The two fixtures are minted from the same seed and therefore hold identical
//! plaintext. That makes the plain backup the answer key for the encrypted one:
//! a decrypt is correct exactly when it reproduces the plain fixture's bytes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use ios_backup_core::{Backup, Credentials, Error, FileKind, Password};

const PASSWORD: &str = "test-password-1234";

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data")
        .join(name)
}

fn plain() -> Backup {
    Backup::open(&fixture("plain-backup")).expect("unencrypted fixture opens")
}

fn encrypted() -> Backup {
    Backup::open_with(
        &fixture("encrypted-backup"),
        &Credentials::password(Password::new(PASSWORD)),
    )
    .expect("encrypted fixture opens with the right password")
}

// ------------------------------------------------------------ the file tree

#[test]
fn an_unencrypted_backup_reports_itself_unencrypted() {
    assert!(!plain().is_encrypted());
    assert!(Backup::open(&fixture("encrypted-backup")).map_or(true, |b| b.is_encrypted()));
}

#[test]
fn lists_every_manifest_row_including_directories() {
    let backup = plain();
    // Four files plus one directory row.
    assert_eq!(backup.files().len(), 5);
    assert_eq!(
        backup
            .files()
            .iter()
            .filter(|f| f.kind == FileKind::Directory)
            .count(),
        1
    );
}

#[test]
fn file_id_is_sha1_of_domain_dash_relative_path() {
    // 3d0d7e5f… is the published fileID for HomeDomain-Library/SMS/sms.db in
    // every iOS backup — an externally known value, not one this crate chose.
    let backup = plain();
    let sms = backup
        .find("HomeDomain", "Library/SMS/sms.db")
        .expect("the fixture carries sms.db");

    assert_eq!(sms.file_id, "3d0d7e5fb2ce288813306e4d4636395e047a3d28");
}

#[test]
fn records_domain_relative_path_and_size() {
    let backup = plain();
    let sms = backup.find("HomeDomain", "Library/SMS/sms.db").unwrap();

    assert_eq!(sms.domain, "HomeDomain");
    assert_eq!(sms.relative_path, "Library/SMS/sms.db");
    assert_eq!(sms.kind, FileKind::File);
    // Some(316), not 316: the manifest recording no size is a distinct state
    // from it recording zero (ADR-0003).
    assert_eq!(sms.size, Some(316)); // 16-byte "SQLite format 3\0" header + 300 'A's
}

// ------------------------------------------------------------ reading bytes

#[test]
fn reads_an_unencrypted_file_verbatim() {
    let mut backup = plain();
    let sms = backup
        .find("HomeDomain", "Library/SMS/sms.db")
        .unwrap()
        .clone();

    let bytes = backup.read(&sms).unwrap();
    assert!(bytes.starts_with(b"SQLite format 3\0"));
    assert_eq!(bytes.len(), 316);
}

#[test]
fn decrypting_reproduces_the_plaintext_fixture_byte_for_byte() {
    // The oracle check: Python's `cryptography` encrypted these bytes, this
    // crate decrypted them, and they must equal the independently-minted
    // plaintext for every file in the backup.
    let mut plain_backup = plain();
    let mut encrypted_backup = encrypted();

    let entries: Vec<_> = plain_backup
        .files()
        .iter()
        .filter(|f| f.kind == FileKind::File)
        .cloned()
        .collect();
    assert_eq!(entries.len(), 4, "all four files must be compared");

    for entry in entries {
        let expected = plain_backup.read(&entry).unwrap();
        let actual_entry = encrypted_backup
            .find(&entry.domain, &entry.relative_path)
            .expect("the encrypted backup holds the same tree")
            .clone();
        let actual = encrypted_backup.read(&actual_entry).unwrap();

        assert_eq!(
            actual, expected,
            "decrypt mismatch for {}-{}",
            entry.domain, entry.relative_path
        );
    }
}

#[test]
fn a_file_whose_length_is_an_exact_block_multiple_is_not_truncated() {
    // 32 bytes is two whole AES blocks. A reader that strips PKCS#7 padding
    // here would return 16 bytes, or fewer, and call it success.
    let mut backup = encrypted();
    let entry = backup
        .find("AppDomain-com.example.app", "Documents/exact.bin")
        .unwrap()
        .clone();

    let bytes = backup.read(&entry).unwrap();
    assert_eq!(bytes.len(), 32);
    assert_eq!(bytes, vec![b'B'; 32]);
}

#[test]
fn reading_a_directory_entry_is_an_error_not_empty_bytes() {
    let mut backup = plain();
    let dir = backup
        .files()
        .iter()
        .find(|f| f.kind == FileKind::Directory)
        .unwrap()
        .clone();

    assert!(backup.read(&dir).is_err());
}

// ------------------------------------------------------------- the password

#[test]
fn opening_an_encrypted_backup_without_a_password_says_so() {
    let err = Backup::open(&fixture("encrypted-backup")).unwrap_err();

    assert!(
        matches!(err, Error::PasswordRequired),
        "an encrypted backup with no password must ask for one, not fail to \
         parse; got: {err}"
    );
}

#[test]
fn the_wrong_password_is_reported_as_a_wrong_password() {
    // Not as a corrupt keybag, and never as an empty file list: an examiner
    // must be able to tell "wrong password" from "nothing in this backup".
    let err = Backup::open_with(
        &fixture("encrypted-backup"),
        &Credentials::password(Password::new("not-the-password")),
    )
    .unwrap_err();

    assert!(matches!(err, Error::WrongPassword), "got: {err}");
}

#[test]
fn a_password_on_an_unencrypted_backup_is_harmless() {
    let backup = Backup::open_with(
        &fixture("plain-backup"),
        &Credentials::password(Password::new("irrelevant")),
    )
    .expect("a password supplied for a plaintext backup is simply unused");

    assert!(!backup.is_encrypted());
}

#[test]
fn a_password_never_renders_itself() {
    // A credential that prints itself will eventually print itself into a log
    // or an error report. Debug is the accident-prone path, so it is redacted.
    let password = Password::new("hunter2");
    let rendered = format!("{password:?}");

    assert!(!rendered.contains("hunter2"), "got: {rendered}");
}

// ------------------------------------------------------------ backup metadata

#[test]
fn exposes_the_device_metadata_an_examiner_needs() {
    let backup = plain();
    let meta = backup.metadata();

    assert_eq!(meta.product_version.as_deref(), Some("26.2"));
    assert_eq!(meta.product_type.as_deref(), Some("iPhone13,3"));
    assert_eq!(meta.device_name.as_deref(), Some("Fixture"));
}

// --------------------------------------------------------------- robustness

#[test]
fn a_directory_that_is_not_a_backup_is_rejected_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let err = Backup::open(dir.path()).unwrap_err();

    assert!(
        format!("{err}").contains("Manifest.plist"),
        "the error must name what was missing, got: {err}"
    );
}

#[test]
fn a_missing_content_blob_is_an_error_naming_the_file_id() {
    let dir = tempfile::tempdir().unwrap();
    let source = fixture("plain-backup");
    for name in [
        "Manifest.db",
        "Manifest.plist",
        "Status.plist",
        "Info.plist",
    ] {
        std::fs::copy(source.join(name), dir.path().join(name)).unwrap();
    }
    // Every blob subdirectory is deliberately absent.
    let mut backup = Backup::open(dir.path()).unwrap();
    let entry = backup
        .find("HomeDomain", "Library/SMS/sms.db")
        .unwrap()
        .clone();

    let err = backup.read(&entry).unwrap_err();
    assert!(
        format!("{err}").contains("3d0d7e5f"),
        "the error must name the missing blob, got: {err}"
    );
}
