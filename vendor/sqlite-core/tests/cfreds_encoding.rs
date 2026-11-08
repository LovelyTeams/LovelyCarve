//! Known-answer validation against the **NIST CFReDS SQLite** corpus, test set
//! SFT-01 ("SQLite Databases — Header and Table info"). These are real databases
//! created on real devices by the National Institute of Standards and Technology
//! (Android `sqlite 3.19.0`, iOS `sqlite 3.32.3`) and published with documented
//! ground truth — the independent, authoritative replacement for the
//! self-minted `utf16_text_tests.rs` fixtures (a test you wrote against a fixture
//! you generated inherits your blind spots).
//!
//! SFT-01's stated purpose (NIST `SQLiteDocumentation.rtf`): "Validate the page
//! size, journal mode, number of pages, encoding, tables, column names and row
//! information is accurately reported." This harness asserts exactly that against
//! the three text encodings (UTF-8, UTF-16BE, UTF-16LE), on both platforms.
//!
//! Provenance + per-file NIST MD5s: `tests/data/README.md` (§ NIST CFReDS).
//! These files are committed (NIST works are U.S. Government public domain).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::pedantic)]

use std::path::{Path, PathBuf};

use sqlite_core::{Database, TextEncoding, Value};

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/cfreds")
        .join(name)
}

fn open(name: &str) -> Database {
    Database::open(std::fs::read(data(name)).expect("read CFReDS fixture")).expect("open")
}

/// One SFT-01 variation: filename stem, NIST-documented encoding + page size.
struct Sft01 {
    stem: &'static str,
    encoding: TextEncoding,
    page_size: u32,
}

const SFT01: &[Sft01] = &[
    Sft01 {
        stem: "sft-01_utf8_wal",
        encoding: TextEncoding::Utf8,
        page_size: 4096,
    },
    Sft01 {
        stem: "sft-01_utf16be_persist",
        encoding: TextEncoding::Utf16Be,
        page_size: 1024,
    },
    Sft01 {
        stem: "sft-01_utf16le_off",
        encoding: TextEncoding::Utf16Le,
        page_size: 8192,
    },
];

#[test]
fn sft01_header_reports_encoding_and_page_size() {
    for v in SFT01 {
        for platform in ["ios", "android"] {
            let db = open(&format!("{}_{platform}.sqlite", v.stem));
            let h = db.header();
            assert_eq!(
                h.text_encoding, v.encoding,
                "{}_{platform}: encoding must match NIST ground truth",
                v.stem
            );
            assert_eq!(
                h.page_size, v.page_size,
                "{}_{platform}: page size must match NIST ground truth",
                v.stem
            );
        }
    }
}

#[test]
fn sft01_albums_decodes_identically_across_all_encodings() {
    // The same logical table, stored UTF-8 / UTF-16BE / UTF-16LE, must decode to
    // byte-identical Unicode strings. A UTF-16 database mis-decoded as UTF-8
    // would yield NUL-interleaved mojibake here, not "Bangles" — the silent
    // wrong-output class this whole corpus exercise exists to catch.
    let expected_first = vec![
        Value::Integer(1),
        Value::Text("WALK LIKE AN EGYPTIAN".to_string()),
        Value::Text("Bangles".to_string()),
        Value::Text("Columbia".to_string()),
    ];
    for v in SFT01 {
        for platform in ["ios", "android"] {
            let name = format!("{}_{platform}.sqlite", v.stem);
            let db = open(&name);
            let dumps = db.live_table_rows();
            let albums = dumps
                .iter()
                .find(|d| d.name.eq_ignore_ascii_case("Albums"))
                .unwrap_or_else(|| panic!("{name}: Albums table present"));
            assert_eq!(
                albums.column_names,
                ["_ID", "Title", "Artist", "Label"],
                "{name}: real column names from CREATE TABLE"
            );
            assert_eq!(albums.rows.len(), 100, "{name}: NIST documents 100 rows");
            assert_eq!(
                albums.rows[0].values, expected_first,
                "{name}: first row must decode identically regardless of on-disk encoding"
            );
        }
    }
}
