use base64::Engine;

use super::*;
use crate::core::common::error::{code_of, ErrorCode};

const BLANKPASS: &[u8] =
    include_bytes!("../../../../vendor/kdbx/tests/data/pykeepass/test4_blankpass.kdbx");
const TEST4_KEY: &[u8] = include_bytes!("../../../../vendor/kdbx/tests/data/pykeepass/test4.key");
const TEST3: &[u8] = include_bytes!("../../../../vendor/kdbx/tests/data/pykeepass/test3.kdbx");
const TEST3_KEY: &[u8] = include_bytes!("../../../../vendor/kdbx/tests/data/pykeepass/test3.key");
const SYNTHETIC: &str = include_str!("../../../../vendor/kdbx/tests/data/synthetic.json");

fn credentials(password: Option<&str>) -> KdbxCredentials {
    KdbxCredentials {
        password: password.map(str::to_owned),
        ..KdbxCredentials::default()
    }
}

fn code(error: &anyhow::Error) -> Option<ErrorCode> {
    code_of(error)
}

#[test]
fn an_empty_password_field_tries_the_keyfile_alone_then_an_empty_password() {
    // test4_blankpass.kdbx needs an empty password and the keyfile.
    let (database, used) = open_database(BLANKPASS, &credentials(None), Some(TEST4_KEY)).unwrap();
    assert_eq!(
        (used.password, used.keyfile, used.transformed_key),
        ("empty", Some("SHA-256 of file"), false)
    );
    assert_eq!(database.meta.database_name.as_deref(), Some("aaa"));

    // A synthetic database protected by an empty password and nothing else.
    let corpus: serde_json::Value = serde_json::from_str(SYNTHETIC).unwrap();
    let case = corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "kdbx40-argon2d-v10-empty-password")
        .unwrap();
    let data = base64::engine::general_purpose::STANDARD
        .decode(case["file"].as_str().unwrap())
        .unwrap();
    let (_, used) = open_database(&data, &credentials(Some("")), None).unwrap();
    assert_eq!(used.password, "empty");
}

#[test]
fn credentials_errors_are_invalid_input_and_damage_is_corrupt() {
    let wrong = open_database(TEST3, &credentials(Some("invalid")), Some(TEST3_KEY)).unwrap_err();
    assert_eq!(code(&wrong), Some(ErrorCode::InvalidInput));
    let short = KdbxCredentials {
        transformed_key: Some("00ff".into()),
        ..KdbxCredentials::default()
    };
    let error = open_database(TEST3, &short, None).unwrap_err();
    assert_eq!(code(&error), Some(ErrorCode::InvalidInput));

    let mut damaged = TEST3.to_vec();
    let middle = damaged.len() / 2;
    damaged[middle] ^= 0x40;
    let error =
        open_database(&damaged, &credentials(Some("password")), Some(TEST3_KEY)).unwrap_err();
    assert_eq!(code(&error), Some(ErrorCode::CorruptData));
    let other =
        open_database(b"PK\x03\x04 not a database", &credentials(Some("x")), None).unwrap_err();
    assert_eq!(code(&other), Some(ErrorCode::UnsupportedFormat));
}

#[test]
fn reports_the_database_and_reads_attachments_by_entry() {
    let (database, used) =
        open_database(TEST3, &credentials(Some("password")), Some(TEST3_KEY)).unwrap();
    assert_eq!((used.password, used.keyfile), ("set", Some("XML 1.0")));
    let report = database_report(&database, used, TEST3.len() as u64);
    assert_eq!(report.header.version, "3.1");
    assert_eq!(report.header.kdf.name, "AES-KDF");
    assert_eq!(report.header.kdf.rounds, Some(100));
    assert_eq!(report.header_hash, "matches");
    assert_eq!(report.inner_stream, "Salsa20");
    assert_eq!(report.counts.groups, 7);
    assert!(report.counts.history > 0);
    assert_eq!(report.transformed_key.len(), 64);

    // The transformed key from the report reopens the database.
    let again = KdbxCredentials {
        transformed_key: Some(report.transformed_key.clone()),
        ..KdbxCredentials::default()
    };
    let (_, used) = open_database(TEST3, &again, None).unwrap();
    assert!(used.transformed_key);

    // A KeePass field reference is resolved next to the stored text.
    let resolved: Vec<_> = report
        .root
        .entries
        .iter()
        .flat_map(|entry| entry.fields.iter())
        .filter_map(|field| field.resolved.clone())
        .collect();
    assert!(
        resolved.iter().any(|value| value == "domain\\foobar2"),
        "{resolved:?}"
    );

    let owner = database
        .entries()
        .into_iter()
        .find(|entry| !entry.attachments.is_empty())
        .unwrap();
    let reference = KdbxAttachmentRef {
        entry: owner.uuid.unwrap().to_string(),
        history: None,
        index: 0,
    };
    assert_eq!(attachment_bytes(&database, &reference, 1024).unwrap(), b"");
    let missing = KdbxAttachmentRef {
        index: 9,
        ..reference.clone()
    };
    let error = attachment_bytes(&database, &missing, 1024).unwrap_err();
    assert_eq!(code(&error), Some(ErrorCode::NotFound));
}

#[test]
fn credentials_never_reach_debug_output() {
    let debug = format!(
        "{:?}",
        KdbxCredentials {
            password: Some("hunter2".into()),
            keyfile: None,
            transformed_key: Some("ab".repeat(32)),
        }
    );
    assert!(
        !debug.contains("hunter2") && !debug.contains("abab"),
        "{debug}"
    );
}
