//! Malformed and hostile input: every path must yield a typed error, never a
//! panic and never a silently-empty success (ADR-0012, Paranoid Gatekeeper).
//!
//! Regression tests, added after the implementation.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ios_backup_core::crypto::{self, ClassKeys};
use ios_backup_core::nskeyed::Archive;
use ios_backup_core::{Backup, Credentials, Error};

// --------------------------------------------------------------- NSKeyedArchiver

#[test]
fn a_blob_that_is_not_a_plist_is_rejected() {
    let err = Archive::parse(b"not a property list at all").unwrap_err();
    assert!(matches!(err, Error::BadFileMetadata(_)), "got: {err}");
}

#[test]
fn a_plist_that_is_not_an_archive_names_what_is_missing() {
    // A valid plist, but not an NSKeyedArchiver archive. The error must say
    // which structural piece was absent rather than "parse failed".
    let xml = br#"<?xml version="1.0"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>Size</key><integer>7</integer></dict></plist>"#;

    let err = Archive::parse(xml).unwrap_err();
    assert!(
        format!("{err}").contains("$objects"),
        "the error must name the missing structure; got: {err}"
    );
}

#[test]
fn an_archive_without_a_top_root_is_rejected_rather_than_assuming_index_one() {
    // Defaulting to $objects[1] would be right most of the time and silently
    // wrong the rest, which is the worst combination.
    let xml = br#"<?xml version="1.0"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>$objects</key><array><string>$null</string></array>
</dict></plist>"#;

    let err = Archive::parse(xml).unwrap_err();
    assert!(format!("{err}").contains("$top.root"), "got: {err}");
}

#[test]
fn a_plist_that_is_an_array_not_a_dictionary_is_rejected() {
    let xml = br#"<?xml version="1.0"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><array><integer>1</integer></array></plist>"#;

    assert!(Archive::parse(xml).is_err());
}

// ------------------------------------------------------------------ crypto

#[test]
fn class_keys_start_empty_and_report_it() {
    let keys = ClassKeys::default();

    assert!(keys.is_empty());
    assert_eq!(keys.len(), 0);
    assert!(keys.classes().is_empty());
    assert!(keys.get(1).is_none());
    // Debug must not render key bytes, only which classes are held.
    assert!(format!("{keys:?}").contains("ClassKeys"));
}

#[test]
fn unwrapping_a_file_key_with_no_class_key_names_the_class() {
    let err = crypto::unwrap_file_key(&ClassKeys::default(), 7, &[0u8; 40]).unwrap_err();

    assert!(
        format!("{err}").contains('7'),
        "the error must name the protection class; got: {err}"
    );
    assert!(matches!(err, Error::NoKeyForClass(7)));
}

#[test]
fn a_zero_length_ciphertext_decrypts_to_nothing_without_erroring() {
    // Zero is a whole number of blocks. An empty file is legal in a backup, and
    // rejecting it would make a legitimate entry unreadable.
    assert_eq!(
        crypto::decrypt_aes_cbc_zero_iv(&[0u8; 32], &[]).unwrap(),
        Vec::<u8>::new()
    );
}

#[test]
fn a_bad_iv_length_is_reported_with_its_value() {
    let err = crypto::decrypt_aes_cbc(&[0u8; 32], &[0u8; 8], &[0u8; 16]).unwrap_err();
    assert!(format!("{err}").contains('8'), "got: {err}");
}

#[test]
fn an_unsupported_key_width_for_cbc_is_reported_with_its_value() {
    let err = crypto::decrypt_aes_cbc(&[0u8; 20], &[0u8; 16], &[0u8; 16]).unwrap_err();
    assert!(format!("{err}").contains("20"), "got: {err}");
}

// ------------------------------------------------------------------- backup

#[test]
fn a_backup_whose_manifest_plist_is_corrupt_says_so() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Manifest.plist"), b"\x00\x01 not a plist").unwrap();

    let err = Backup::open(dir.path()).unwrap_err();
    assert!(
        matches!(err, Error::BadPlist { .. }),
        "a present-but-unreadable plist is distinct from an absent one; got: {err}"
    );
}

#[test]
fn a_backup_marked_encrypted_with_no_keybag_is_reported_precisely() {
    let dir = tempfile::tempdir().unwrap();
    let xml = br#"<?xml version="1.0"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>IsEncrypted</key><true/>
  <key>Version</key><string>10.0</string>
</dict></plist>"#;
    std::fs::write(dir.path().join("Manifest.plist"), xml).unwrap();
    // A Manifest.db that is not plaintext SQLite, so the ONLY defect under test
    // is the absent keybag. Without this the fixture also lacked the manifest
    // entirely and asserted on whichever failure surfaced first.
    std::fs::write(dir.path().join("Manifest.db"), b"\x00\x01 not a database").unwrap();

    let err = Backup::open_with(
        dir.path(),
        &Credentials::password(ios_backup_core::Password::new("pw")),
    )
    .unwrap_err();

    assert!(matches!(err, Error::MissingKeyBag), "got: {err}");
}

#[test]
fn a_manifest_db_that_is_not_a_database_is_reported_as_such() {
    let dir = tempfile::tempdir().unwrap();
    let xml = br#"<?xml version="1.0"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>IsEncrypted</key><false/>
  <key>Version</key><string>10.0</string>
</dict></plist>"#;
    std::fs::write(dir.path().join("Manifest.plist"), xml).unwrap();
    std::fs::write(dir.path().join("Manifest.db"), b"definitely not sqlite").unwrap();

    let err = Backup::open(dir.path()).unwrap_err();
    assert!(matches!(err, Error::Sqlite(_)), "got: {err}");
}

#[test]
fn a_backup_with_no_manifest_db_names_the_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let xml = br#"<?xml version="1.0"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>IsEncrypted</key><false/></dict></plist>"#;
    std::fs::write(dir.path().join("Manifest.plist"), xml).unwrap();

    let err = Backup::open(dir.path()).unwrap_err();
    assert!(
        format!("{err}").contains("Manifest.db"),
        "the error must name the file it could not read; got: {err}"
    );
}
