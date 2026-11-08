//! The credential seam a mounting tool supplies a password through.
//!
//! Added after the implementation, so these are **regression tests**, not
//! TDD-compliant ones. They hold the supply routes and the redaction guarantee
//! in place rather than having driven them.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ios_backup_core::{Credentials, Error, Password};

#[test]
fn a_password_is_the_bytes_it_was_given() {
    assert_eq!(Password::new("hunter2").as_bytes(), b"hunter2");
}

#[test]
fn a_password_may_be_bytes_that_are_not_utf8() {
    // iOS accepts a passcode typed on a device keyboard; it is not guaranteed
    // to survive a round trip through String, so the byte route must exist.
    let raw = vec![0xff, 0xfe, 0x00, 0x41];
    let password = Password::from_bytes(raw.clone());

    assert_eq!(password.as_bytes(), raw.as_slice());
}

#[test]
fn an_empty_password_is_legal_and_reported_as_empty() {
    // iOS permits an empty backup password. Empty is informational here, never
    // a reason to refuse: refusing would make a legitimate backup unreadable.
    let password = Password::new("");

    assert!(password.is_empty());
    assert!(!Password::new("x").is_empty());
}

#[test]
fn from_file_strips_exactly_one_trailing_newline() {
    // Exactly one: an editor adds a single newline, so stripping one recovers
    // what the examiner typed. Stripping all of them would silently change a
    // password that genuinely ends in whitespace.
    let dir = tempfile::tempdir().unwrap();

    let unix = dir.path().join("unix");
    std::fs::write(&unix, b"secret\n").unwrap();
    assert_eq!(Password::from_file(&unix).unwrap().as_bytes(), b"secret");

    let windows = dir.path().join("windows");
    std::fs::write(&windows, b"secret\r\n").unwrap();
    assert_eq!(Password::from_file(&windows).unwrap().as_bytes(), b"secret");

    let bare = dir.path().join("bare");
    std::fs::write(&bare, b"secret").unwrap();
    assert_eq!(Password::from_file(&bare).unwrap().as_bytes(), b"secret");

    // A password that really does end in a blank line keeps the second one.
    let doubled = dir.path().join("doubled");
    std::fs::write(&doubled, b"secret\n\n").unwrap();
    assert_eq!(
        Password::from_file(&doubled).unwrap().as_bytes(),
        b"secret\n"
    );
}

#[test]
fn from_file_on_a_missing_file_names_the_path() {
    let err = Password::from_file(std::path::Path::new("/nonexistent/pw.txt")).unwrap_err();

    assert!(
        format!("{err}").contains("/nonexistent/pw.txt"),
        "the error must name the path it could not read; got: {err}"
    );
}

#[test]
fn from_env_reads_the_variable_or_reports_absence() {
    // SAFETY-adjacent: set/remove_var is process-global. Using a name no other
    // test touches keeps this from racing them.
    const VAR: &str = "IOS_BACKUP_TEST_PASSWORD_FROM_ENV";

    assert!(Password::from_env(VAR).is_none());
    std::env::set_var(VAR, "from-the-environment");
    assert_eq!(
        Password::from_env(VAR).unwrap().as_bytes(),
        b"from-the-environment"
    );
    std::env::remove_var(VAR);
}

#[test]
fn from_tty_reports_that_prompting_is_the_front_ends_job() {
    // The library does not read a terminal: that would pull a TTY crate into
    // every downstream binary and make the reader unusable from a daemon. The
    // variant exists so a front-end can report the failure in this crate's own
    // error vocabulary.
    assert!(matches!(
        Password::from_tty("Backup password: "),
        Err(Error::NoTerminal)
    ));
}

#[test]
fn credentials_default_to_offering_nothing() {
    // Secure by default: the zero-configuration path asks for a password on an
    // encrypted backup rather than silently trying the empty string.
    assert_eq!(Credentials::default(), Credentials::None);
    assert!(Credentials::default().as_password().is_none());
    assert!(Credentials::none().as_password().is_none());
}

#[test]
fn credentials_carry_the_password_they_were_built_with() {
    let creds = Credentials::password(Password::new("pw"));
    assert_eq!(creds.as_password().unwrap().as_bytes(), b"pw");

    // The From conversion is the ergonomic route for a front-end.
    let converted: Credentials = Password::new("pw").into();
    assert_eq!(converted, creds);
}

#[test]
fn neither_a_password_nor_credentials_renders_its_secret() {
    // Debug is the accident-prone path — a stray {:?} in a log line, a panic
    // message, an error report. Both must be safe to format.
    let password = Password::new("swordfish");
    assert!(!format!("{password:?}").contains("swordfish"));

    let creds = Credentials::password(Password::new("swordfish"));
    assert!(
        !format!("{creds:?}").contains("swordfish"),
        "got: {creds:?}"
    );
}
