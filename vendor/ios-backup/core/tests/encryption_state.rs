//! When `Manifest.plist` disagrees with the bytes.
//!
//! `IsEncrypted` is a **declaration**. It tells you what the plist claims, not
//! what is true of `Manifest.db`, and the two can disagree — a hand-edited
//! plist, a partial copy, a tool that rewrote metadata without rewriting data.
//!
//! Reading the declaration and stopping there is the "an unconfigured setting
//! tells you the default, never the truth" failure: the reader would refuse a
//! perfectly readable backup, or try to parse ciphertext as `SQLite`, on the
//! strength of one boolean.
//!
//! So the reader surveys the **effective** state — does `Manifest.db` actually
//! begin with the `SQLite` magic — and recovers from the disagreement rather
//! failing on it. Both values are kept, because the disagreement is itself
//! evidence.
//!
//! Contrast MVT, which infers encryption purely from whether `Manifest.db`
//! parses, and so cannot tell an encrypted backup from a corrupt one. Keeping
//! both signals distinguishes all three states.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use ios_backup_core::{Backup, Credentials, Error, Password};

const PASSWORD: &str = "test-password-1234";

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data")
        .join(name)
}

// ------------------------------------------------ the honest backups first

#[test]
fn an_honest_backup_reports_no_contradiction() {
    for (name, encrypted) in [("plain-backup", false), ("encrypted-backup", true)] {
        let creds = if encrypted {
            Credentials::password(Password::new(PASSWORD))
        } else {
            Credentials::none()
        };
        let backup = Backup::open_with(&fixture(name), &creds).unwrap();
        let state = backup.encryption_state();

        assert_eq!(state.declared, encrypted, "{name}");
        assert_eq!(state.observed, encrypted, "{name}");
        assert!(!state.is_contradictory(), "{name}");
    }
}

// ------------------------- declared plaintext, actually encrypted (the worse)

#[test]
fn a_backup_declared_plaintext_but_actually_encrypted_still_opens_with_a_password() {
    // The declaration is false; the bytes are the authority. Refusing here would
    // deny an examiner a backup that is entirely readable.
    let backup = Backup::open_with(
        &fixture("lie-unencrypted"),
        &Credentials::password(Password::new(PASSWORD)),
    )
    .expect("the bytes are encrypted and the password is right, so this opens");

    let state = backup.encryption_state();
    assert!(!state.declared, "Manifest.plist claims IsEncrypted = false");
    assert!(state.observed, "but Manifest.db is not plaintext SQLite");
    assert!(state.is_contradictory());

    assert!(
        !backup.files().is_empty(),
        "recovery must produce the real file tree, not an empty one"
    );
}

#[test]
fn its_files_decrypt_to_the_same_bytes_as_the_honest_backup() {
    // Recovery is only worth anything if it yields the right bytes.
    let mut lying = Backup::open_with(
        &fixture("lie-unencrypted"),
        &Credentials::password(Password::new(PASSWORD)),
    )
    .unwrap();
    let mut honest = Backup::open_with(&fixture("plain-backup"), &Credentials::none()).unwrap();

    let entry = honest
        .find("HomeDomain", "Library/SMS/sms.db")
        .unwrap()
        .clone();
    let expected = honest.read(&entry).unwrap();

    let actual_entry = lying
        .find("HomeDomain", "Library/SMS/sms.db")
        .unwrap()
        .clone();
    assert_eq!(lying.read(&actual_entry).unwrap(), expected);
}

#[test]
fn without_a_password_it_asks_for_one_rather_than_reporting_a_corrupt_manifest() {
    // The actionable error. "Manifest.db is not a database" sends the examiner
    // looking for damage; "this needs a password" is the truth and tells them
    // what to do, even though the plist said no password was needed.
    let err = Backup::open(&fixture("lie-unencrypted")).unwrap_err();

    assert!(matches!(err, Error::PasswordRequired), "got: {err}");
}

// ------------------------- declared encrypted, actually plaintext (the other)

#[test]
fn a_backup_declared_encrypted_but_actually_plaintext_opens_with_no_password() {
    // Symmetric recovery. Demanding a password for data that is not encrypted
    // would block a readable backup on a stale flag.
    let backup = Backup::open(&fixture("lie-encrypted"))
        .expect("the bytes are plaintext, so no password is needed");

    let state = backup.encryption_state();
    assert!(state.declared, "Manifest.plist claims IsEncrypted = true");
    assert!(!state.observed, "but Manifest.db is plaintext SQLite");
    assert!(state.is_contradictory());
    assert!(!backup.files().is_empty());
}

#[test]
fn is_encrypted_reports_the_bytes_not_the_declaration() {
    // The question a caller is really asking is "must this be decrypted", and
    // only the bytes can answer it.
    assert!(!Backup::open(&fixture("lie-encrypted"))
        .unwrap()
        .is_encrypted());
}

// ------------------------------------------------------- corrupt, not encrypted

#[test]
fn a_corrupt_manifest_with_no_keybag_is_still_reported_as_corrupt() {
    // The distinction MVT's probe cannot draw. Unparseable AND no keybag means
    // damaged, not encrypted — calling it encrypted would send the examiner
    // hunting for a password that does not exist.
    let dir = tempfile::tempdir().unwrap();
    let xml = br#"<?xml version="1.0"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>IsEncrypted</key><false/></dict></plist>"#;
    std::fs::write(dir.path().join("Manifest.plist"), xml).unwrap();
    std::fs::write(dir.path().join("Manifest.db"), b"not a database at all").unwrap();

    let err = Backup::open(dir.path()).unwrap_err();
    assert!(
        matches!(err, Error::Sqlite(_)),
        "no keybag means damaged, not encrypted; got: {err}"
    );
}
