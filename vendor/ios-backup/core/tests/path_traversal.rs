//! A `fileID` must never walk the reader out of the backup directory.
//!
//! `Manifest.db` is attacker-controllable — that is the whole premise of
//! ADR-0012 — and `fileID` is used to build a filesystem path. Found by reading
//! MVT (Amnesty International Security Lab), which guards the same path
//! explicitly:
//!
//! ```python
//! if not Path(source_file_path).resolve().is_relative_to(Path(self.backup_path).resolve()):
//!     log.warning("Skipping unsafe file_id: %s", ...)
//! ```
//!
//! Two escapes existed here, and the second is the dangerous one:
//!
//! ```text
//! "../../../../etc/passwd"  ->  /evidence/backup/../../../../../etc/passwd
//! "/etc/passwd"             ->  /etc/passwd
//! ```
//!
//! Rust's `Path::join` **replaces** the whole path when the argument is
//! absolute, so an absolute `fileID` does not merely climb out of the backup —
//! it addresses the examiner's filesystem directly, from the first join.
//!
//! The fix is structural rather than a check: a `fileID` *is* a SHA-1 hex
//! digest, so validating that shape makes traversal impossible by construction.
//! There is no path to sanitise because no path is built.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use ios_backup_core::{Backup, Credentials, Error};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data")
        .join(name)
}

#[test]
fn a_traversing_file_id_resolves_to_no_path_at_all() {
    let backup = Backup::open_with(&fixture("evil-file-id"), &Credentials::none()).unwrap();

    let entry = backup
        .files()
        .iter()
        .find(|f| f.file_id.contains(".."))
        .expect("the fixture carries a traversing fileID");

    assert!(
        entry.blob_relative_path().is_none(),
        "a fileID that is not a SHA-1 digest must resolve to no blob path; \
         got {:?}",
        entry.blob_relative_path()
    );
    assert!(backup.blob_path(entry).is_none());
}

#[test]
fn reading_a_traversing_file_id_is_refused_and_names_the_value() {
    let mut backup = Backup::open_with(&fixture("evil-file-id"), &Credentials::none()).unwrap();
    let entry = backup
        .files()
        .iter()
        .find(|f| f.file_id.contains(".."))
        .unwrap()
        .clone();

    let err = backup.read(&entry).unwrap_err();

    assert!(
        matches!(err, Error::UnsafeFileId(_)),
        "the refusal must be specific, not a generic missing-blob; got: {err}"
    );
    // Show-the-unrecognized-value: an examiner needs to see what was rejected.
    assert!(
        format!("{err}").contains("etc/passwd"),
        "the offending fileID must appear verbatim; got: {err}"
    );
}

#[test]
fn an_absolute_file_id_cannot_address_the_examiners_filesystem() {
    // The worse of the two escapes: Path::join replaces rather than appends
    // when given an absolute path, so this never even leaves the first join.
    for evil in ["/etc/passwd", "/private/etc/passwd"] {
        let entry = entry_with_file_id(evil);
        assert!(
            entry.blob_relative_path().is_none(),
            "absolute fileID {evil} resolved to {:?}",
            entry.blob_relative_path()
        );
    }
}

#[test]
fn only_a_forty_character_hex_digest_resolves() {
    // The general rule, not a blocklist of bad shapes: a fileID IS a SHA-1 hex
    // digest. Anything else names no blob, so there is nothing to sanitise.
    let good = entry_with_file_id("3d0d7e5fb2ce288813306e4d4636395e047a3d28");
    assert_eq!(
        good.blob_relative_path().unwrap(),
        PathBuf::from("3d").join("3d0d7e5fb2ce288813306e4d4636395e047a3d28")
    );

    for bad in [
        "",                                          // empty
        "3d",                                        // too short
        "3d0d7e5fb2ce288813306e4d4636395e047a3d2",   // 39 chars
        "3d0d7e5fb2ce288813306e4d4636395e047a3d288", // 41 chars
        "3d0d7e5fb2ce288813306e4d4636395e047a3d2g",  // non-hex
        "3d0d7e5f/../../../../etc/passwd",           // separator
        "..",
        "../..",
        "3d0d7e5f\\..\\..\\windows\\system32", // Windows separators
    ] {
        assert!(
            entry_with_file_id(bad).blob_relative_path().is_none(),
            "fileID {bad:?} must not resolve to a path"
        );
    }
}

#[test]
fn an_uppercase_digest_still_resolves() {
    // Hex is hex. Refusing uppercase would drop a legitimate row on a
    // case-difference, which is a different way to lose evidence.
    let entry = entry_with_file_id("3D0D7E5FB2CE288813306E4D4636395E047A3D28");
    assert!(entry.blob_relative_path().is_some());
}

/// A `BackupFile` carrying `file_id`, built through the public reader so the
/// test exercises the same construction path production does.
fn entry_with_file_id(file_id: &str) -> ios_backup_core::BackupFile {
    let mut backup = Backup::open_with(&fixture("plain-backup"), &Credentials::none()).unwrap();
    let mut entry = backup.files()[0].clone();
    file_id.clone_into(&mut entry.file_id);
    let _ = &mut backup;
    entry
}
