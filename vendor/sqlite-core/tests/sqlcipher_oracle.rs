//! Tier-2 SQLCipher decryption validation against REAL SQLCipher-engine output.
//!
//! The three fixtures under `tests/data/sqlcipher/` were minted by the SQLCipher
//! 4.17 CLI (an independent implementation — the oracle) with the exact commands
//! recorded in `tests/data/README.md`:
//!
//!   sqlcipher enc_v4.db  -> PRAGMA key='correct horse battery staple';
//!   sqlcipher enc_v3.db  -> + PRAGMA cipher_compatibility = 3;
//!   sqlcipher enc_rawkey.db -> PRAGMA key="x'<64 hex>'";  (raw 32-byte key)
//!
//! into a table `t(id INTEGER PRIMARY KEY, name TEXT, val INTEGER)` with three
//! known rows (and a second table `notes` in the v4 fixture). Our RustCrypto
//! decryptor must reproduce the plaintext the OpenSSL-backed engine produced;
//! reading back the known rows through the native reader is the cross-check.

#![allow(clippy::unwrap_used, clippy::expect_used)]
// The module doc embeds literal `sqlcipher` reproducer command lines and product
// names; backticking every token would mangle the reproducer (cf. real_db.rs).
#![allow(clippy::doc_markdown)]

use sqlite_core::sqlcipher::{self, SqlCipherKey, SqlCipherVersion};
use sqlite_core::{Database, Value};

const ENC_V4: &[u8] = include_bytes!("data/sqlcipher/enc_v4.db");
const ENC_V3: &[u8] = include_bytes!("data/sqlcipher/enc_v3.db");
const ENC_RAWKEY: &[u8] = include_bytes!("data/sqlcipher/enc_rawkey.db");

const PASSPHRASE: &[u8] = b"correct horse battery staple";
/// The raw key passed to `PRAGMA key = "x'...'"` when minting `enc_rawkey.db`.
const RAW_KEY: [u8; 32] = [
    0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf, 0x4f, 0x3c,
    0x76, 0x2e, 0x71, 0x60, 0xf3, 0x8b, 0x4d, 0xa5, 0x6a, 0x78, 0x4d, 0x90, 0x45, 0x19, 0x0c, 0xfe,
];

/// The three rows inserted into `t` in every fixture.
fn assert_table_t(db: &Database) {
    let rows = db.read_table(2, 3).expect("walk table t (root page 2)");
    assert_eq!(rows.len(), 3, "three inserted rows in t");

    assert_eq!(rows[0].rowid, 1);
    assert_eq!(rows[0].values[1], Value::Text("alpha".into()));
    assert_eq!(rows[0].values[2], Value::Integer(100));

    assert_eq!(rows[1].rowid, 2);
    assert_eq!(rows[1].values[1], Value::Text("bravo".into()));
    assert_eq!(rows[1].values[2], Value::Integer(200));

    assert_eq!(rows[2].rowid, 3);
    assert_eq!(rows[2].values[1], Value::Text("unicode-snow".into()));
    assert_eq!(rows[2].values[2], Value::Integer(300));
}

#[test]
fn decrypts_v4_and_reads_known_rows() {
    let db = Database::open_encrypted(ENC_V4, &SqlCipherKey::Passphrase(PASSPHRASE.to_vec()))
        .expect("open_encrypted v4");
    assert_eq!(db.header().page_size, 4096);
    // SQLCipher sets a non-zero reserved-space byte for its per-page IV+HMAC.
    assert!(
        db.header().reserved > 0,
        "SQLCipher reserves per-page space"
    );
    assert_table_t(&db);

    // Second table `notes` on root page 3, single column.
    let notes = db.read_table(3, 1).expect("walk notes (root page 3)");
    assert_eq!(notes.len(), 1);
    assert_eq!(
        notes[0].values[0],
        Value::Text("the quick brown fox".into())
    );
}

#[test]
fn detects_v4_version() {
    let out = sqlcipher::decrypt(ENC_V4, &SqlCipherKey::Passphrase(PASSPHRASE.to_vec()))
        .expect("decrypt v4");
    assert_eq!(out.version, SqlCipherVersion::V4);
    assert_eq!(out.page_size, 4096);
    // First 16 bytes of the reconstructed plaintext are the standard magic.
    assert_eq!(&out.plaintext[..16], b"SQLite format 3\x00");
}

#[test]
fn detects_v3_compat_and_reads_known_rows() {
    let out = sqlcipher::decrypt(ENC_V3, &SqlCipherKey::Passphrase(PASSPHRASE.to_vec()))
        .expect("decrypt v3");
    assert_eq!(out.version, SqlCipherVersion::V3, "v3 auto-detected");
    assert_eq!(out.page_size, 1024);

    let db = Database::open_encrypted(ENC_V3, &SqlCipherKey::Passphrase(PASSPHRASE.to_vec()))
        .expect("open_encrypted v3");
    assert_eq!(db.header().page_size, 1024);
    assert_table_t(&db);
}

#[test]
fn decrypts_raw_key_and_reads_known_rows() {
    let db = Database::open_encrypted(ENC_RAWKEY, &SqlCipherKey::RawKey(RAW_KEY))
        .expect("open_encrypted raw key");
    assert_table_t(&db);
}

#[test]
fn wrong_passphrase_is_a_clean_error() {
    let err = Database::open_encrypted(ENC_V4, &SqlCipherKey::Passphrase(b"wrong".to_vec()));
    assert!(
        err.is_err(),
        "a wrong key must fail loud, not panic or misread"
    );

    let err = sqlcipher::decrypt(ENC_V4, &SqlCipherKey::Passphrase(b"wrong".to_vec()));
    assert_eq!(
        err.err(),
        Some(sqlcipher::DecryptError::KeyOrParametersMismatch)
    );
}

#[test]
fn wrong_raw_key_is_a_clean_error() {
    let mut bad = RAW_KEY;
    bad[0] ^= 0xff;
    let err = sqlcipher::decrypt(ENC_RAWKEY, &SqlCipherKey::RawKey(bad));
    assert_eq!(
        err.err(),
        Some(sqlcipher::DecryptError::KeyOrParametersMismatch)
    );
}

#[test]
fn truncated_ciphertext_never_panics() {
    for len in 0..ENC_V4.len().min(4200) {
        let _ = sqlcipher::decrypt(
            &ENC_V4[..len],
            &SqlCipherKey::Passphrase(PASSPHRASE.to_vec()),
        );
    }
}

#[test]
fn empty_input_is_too_small() {
    let err = sqlcipher::decrypt(&[], &SqlCipherKey::RawKey(RAW_KEY));
    assert_eq!(err.err(), Some(sqlcipher::DecryptError::TooSmall));
}
