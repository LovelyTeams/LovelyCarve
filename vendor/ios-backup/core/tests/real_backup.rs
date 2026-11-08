//! Validation against a **real** iOS backup (T2 — genuine third-party artifact,
//! ground truth taken from the format spec and an independent decryptor).
//!
//! Real backups are personal evidence: they are never committed. Point
//! `IOS_BACKUP_DIR` at one to run these, e.g.
//!
//! ```text
//! IOS_BACKUP_DIR="$HOME/Library/Application Support/MobileSync/Backup/<UDID>" \
//!   cargo test -p ios-backup-core --test real_backup -- --nocapture
//! ```
//!
//! Absent the variable the tests skip in ~0.00s. That near-zero duration is the
//! fingerprint of work not done — do not read a green run here as validation
//! unless the variable was set (CLAUDE.md, the three-way control).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use ios_backup_core::keybag::{KeyBag, KeyBagKind};
use ios_backup_core::{Backup, Credentials, Error, Password};

/// The backup under test, or `None` when the env gate is unset.
fn backup_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("IOS_BACKUP_DIR")?);
    assert!(
        dir.join("Manifest.plist").is_file(),
        "IOS_BACKUP_DIR={} has no Manifest.plist — pointing the gate at the wrong \
         directory would skip silently and read as a pass",
        dir.display()
    );
    Some(dir)
}

/// The raw `BackupKeyBag` blob out of `Manifest.plist`.
fn raw_keybag(dir: &std::path::Path) -> Vec<u8> {
    let value = plist::Value::from_file(dir.join("Manifest.plist")).expect("Manifest.plist parses");
    let dict = value
        .as_dictionary()
        .expect("Manifest.plist is a dictionary");
    dict.get("BackupKeyBag")
        .expect("an encrypted backup carries BackupKeyBag")
        .as_data()
        .expect("BackupKeyBag is a data blob")
        .to_vec()
}

#[test]
fn real_keybag_parses_and_is_a_backup_keybag() {
    let Some(dir) = backup_dir() else {
        eprintln!("SKIP: IOS_BACKUP_DIR unset");
        return;
    };

    let bytes = raw_keybag(&dir);
    let kb = KeyBag::parse(&bytes).expect("a real BackupKeyBag must parse");

    assert_eq!(kb.kind, KeyBagKind::Backup);
    assert!(!kb.salt.is_empty(), "a backup keybag always carries SALT");
    assert!(kb.iterations > 0, "ITER must be a real iteration count");

    // Every backup keybag published since iOS 5 carries the file protection
    // classes; a bag that parsed into zero classes means the block-splitting
    // rule is wrong, which a synthetic fixture would not reveal.
    assert!(
        !kb.classes.is_empty(),
        "parsed {} bytes into zero class keys — the class-block rule is wrong",
        bytes.len()
    );

    eprintln!(
        "real keybag: {} bytes, version {}, iter {}, dp_iter {:?}, {} classes {:?}",
        bytes.len(),
        kb.version,
        kb.iterations,
        kb.double_protection_iterations,
        kb.classes.len(),
        kb.classes
            .iter()
            .map(|c| c.protection_class)
            .collect::<Vec<_>>()
    );
}

#[test]
fn real_keybag_class_keys_are_the_wrapped_width() {
    let Some(dir) = backup_dir() else {
        eprintln!("SKIP: IOS_BACKUP_DIR unset");
        return;
    };

    let kb = KeyBag::parse(&raw_keybag(&dir)).unwrap();

    for class in &kb.classes {
        // RFC 3394 wraps an n-byte key to n+8 bytes, so a 32-byte AES class key
        // is stored as 40. A different width here means the field was split at
        // the wrong boundary.
        assert_eq!(
            class.wrapped_key.len(),
            40,
            "class {} WPKY is {} bytes, expected a 40-byte wrapped AES-256 key",
            class.protection_class,
            class.wrapped_key.len()
        );
    }
}

// ------------------------------------------------------- the reader end-to-end

#[test]
fn a_real_encrypted_backup_asks_for_a_password_rather_than_failing_to_parse() {
    let Some(dir) = backup_dir() else {
        eprintln!("SKIP: IOS_BACKUP_DIR unset");
        return;
    };

    match Backup::open(&dir) {
        Err(Error::PasswordRequired) => {}
        Err(other) => panic!("expected PasswordRequired, got: {other}"),
        Ok(backup) => assert!(
            !backup.is_encrypted(),
            "an encrypted backup opened with no password"
        ),
    }
}

/// The production KDF parameters are a different animal from the fixture's: a
/// real backup runs PBKDF2-HMAC-SHA256 ten million times before the SHA-1 round
/// begins. This proves the derivation completes on those values and that a
/// wrong password is reported as such — the fixture, at a thousand rounds,
/// cannot tell us the real one terminates.
///
/// Gated separately because it is deliberately slow: set `IOS_BACKUP_SLOW_KDF=1`
/// alongside `IOS_BACKUP_DIR`.
#[test]
fn a_wrong_password_against_production_kdf_parameters_is_reported_as_wrong() {
    let Some(dir) = backup_dir() else {
        eprintln!("SKIP: IOS_BACKUP_DIR unset");
        return;
    };
    if std::env::var_os("IOS_BACKUP_SLOW_KDF").is_none() {
        eprintln!("SKIP: IOS_BACKUP_SLOW_KDF unset (this test runs a 10M-round KDF)");
        return;
    }

    let started = std::time::Instant::now();
    let err = Backup::open_with(
        &dir,
        &Credentials::password(Password::new("definitely-not-the-backup-password")),
    )
    .expect_err("a wrong password must not open the backup");
    let elapsed = started.elapsed();

    assert!(
        matches!(err, Error::WrongPassword),
        "a wrong password must be reported as WrongPassword, not as a corrupt \
         keybag or an empty backup; got: {err}"
    );
    // A derivation that returned instantly did not run ten million rounds.
    // Absent work leaves a timing signature, and this is it.
    assert!(
        elapsed.as_millis() > 100,
        "derivation finished in {elapsed:?} — too fast to have run the real \
         iteration count, so this test proved nothing"
    );
    eprintln!("production KDF rejected a wrong password in {elapsed:?}");
}

/// Read the backup with a **correct** password, when one is supplied.
///
/// This is the byte-exact end-to-end check against real evidence — the
/// strongest validation available for this crate, and the one gap the synthetic
/// fixtures cannot close. Set `IOS_BACKUP_PASSWORD` to run it.
#[test]
fn a_real_backup_opens_and_reads_with_the_correct_password() {
    let Some(dir) = backup_dir() else {
        eprintln!("SKIP: IOS_BACKUP_DIR unset");
        return;
    };
    let Some(password) = Password::from_env("IOS_BACKUP_PASSWORD") else {
        eprintln!("SKIP: IOS_BACKUP_PASSWORD unset");
        return;
    };

    let mut backup = Backup::open_with(&dir, &Credentials::password(password))
        .expect("the supplied password must open the backup");

    assert!(
        !backup.files().is_empty(),
        "a real backup decrypted to zero files — an empty manifest read as \
         success is the failure mode this assertion exists to catch"
    );

    // Read one real file end to end. sms.db is present in essentially every
    // iPhone backup and its fileID is a published constant.
    if let Some(entry) = backup
        .find_by_id("3d0d7e5fb2ce288813306e4d4636395e047a3d28")
        .cloned()
    {
        let bytes = backup.read(&entry).expect("sms.db must decrypt");
        assert_eq!(
            Some(bytes.len() as u64),
            entry.size,
            "decrypted length must equal the manifest-recorded size"
        );
        assert!(
            bytes.starts_with(b"SQLite format 3\0"),
            "sms.db must decrypt to a SQLite database — a wrong key produces \
             high-entropy bytes that would fail exactly here"
        );
    }

    eprintln!(
        "real backup: {} entries, iOS {:?}, device {:?}",
        backup.files().len(),
        backup.metadata().product_version,
        backup.metadata().product_type
    );
}
