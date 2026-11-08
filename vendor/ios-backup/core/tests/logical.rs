//! The logical-container projection `disk_forensic::logical` consumes.
//!
//! An iOS backup is a logical file container, the same shape as AD1 and
//! AFF4-Logical: a captured file tree with no partition table and no filesystem
//! to walk. Those are reached through `disk_forensic::logical::open`, so this is
//! the surface that must match — entry list plus read-by-index — and it is
//! deliberately shaped to make the upstream adapter thin.
//!
//! The one thing `logical::open` cannot express today is a password. That gap is
//! why [`LogicalView::open`] takes [`Credentials`], and it is recorded in
//! ADR-0005.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use ios_backup_core::logical::LogicalView;
use ios_backup_core::{Credentials, Error, Password};

const PASSWORD: &str = "test-password-1234";

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data")
        .join(name)
}

#[test]
fn presents_entries_as_domain_slash_relative_path() {
    // The path convention every backup browser uses, and the one an examiner
    // will type: the domain is the top-level directory.
    let view = LogicalView::open(&fixture("plain-backup"), &Credentials::none()).unwrap();

    let paths: Vec<&str> = view.entries().iter().map(|e| e.path.as_str()).collect();
    assert!(
        paths.contains(&"HomeDomain/Library/SMS/sms.db"),
        "got {paths:?}"
    );
}

#[test]
fn marks_directories_and_sizes_them_zero() {
    let view = LogicalView::open(&fixture("plain-backup"), &Credentials::none()).unwrap();

    let dir = view
        .entries()
        .iter()
        .find(|e| e.is_dir)
        .expect("the fixture has a directory row");
    assert_eq!(dir.size, 0, "a directory has no content length");
    assert_eq!(dir.path, "HomeDomain/Library/SMS");
}

#[test]
fn reads_a_file_by_its_index_in_the_entry_list() {
    // read_file(index) is the contract disk_forensic::logical::LogicalImage
    // uses, so index and entry order must correspond exactly.
    let mut view = LogicalView::open(&fixture("plain-backup"), &Credentials::none()).unwrap();

    let index = view
        .entries()
        .iter()
        .position(|e| e.path == "HomeDomain/Library/SMS/sms.db")
        .unwrap();

    let bytes = view.read_file(index).unwrap();
    assert!(bytes.starts_with(b"SQLite format 3\0"));
    assert_eq!(bytes.len(), 316);
}

#[test]
fn decrypts_through_the_same_projection() {
    let mut view = LogicalView::open(
        &fixture("encrypted-backup"),
        &Credentials::password(Password::new(PASSWORD)),
    )
    .unwrap();

    let index = view
        .entries()
        .iter()
        .position(|e| e.path == "HomeDomain/Library/SMS/sms.db")
        .unwrap();

    assert!(view
        .read_file(index)
        .unwrap()
        .starts_with(b"SQLite format 3\0"));
}

#[test]
fn an_out_of_range_index_is_an_error_not_a_panic() {
    let mut view = LogicalView::open(&fixture("plain-backup"), &Credentials::none()).unwrap();
    assert!(view.read_file(9_999).is_err());
}

#[test]
fn an_encrypted_backup_without_credentials_asks_for_a_password() {
    let err = LogicalView::open(&fixture("encrypted-backup"), &Credentials::none()).unwrap_err();
    assert!(matches!(err, Error::PasswordRequired), "got: {err}");
}

#[test]
fn a_directory_read_is_refused_rather_than_returning_nothing() {
    let mut view = LogicalView::open(&fixture("plain-backup"), &Credentials::none()).unwrap();
    let index = view.entries().iter().position(|e| e.is_dir).unwrap();

    assert!(
        view.read_file(index).is_err(),
        "reading a directory must be refused; empty bytes would read as a \
         successfully-recovered empty file"
    );
}

#[test]
fn a_domain_or_path_containing_a_slash_still_yields_one_entry_per_row() {
    // Entry count must equal manifest row count: the projection joins fields,
    // it does not split or synthesise intermediate directories.
    let view = LogicalView::open(&fixture("plain-backup"), &Credentials::none()).unwrap();
    let backup =
        ios_backup_core::Backup::open_with(&fixture("plain-backup"), &Credentials::none()).unwrap();

    assert_eq!(view.entries().len(), backup.files().len());
}
