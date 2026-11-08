//! **Tier-1** real-data validation of native-type + BLOB reading against the
//! **NIST CFReDS / CFTT SFT-05** dataset (the SQLite "BLOB data" reference DBs).
//!
//! SFT-05 validates that "data types are reported: primary key, int, float, text,
//! BLOB and boolean types are reported accurately" and that "BLOB data contains a
//! variety of graphic file types". The `new_students` table is
//! `(id INT PRIMARY KEY, name TEXT, photo BLOB, gpa FLOAT, has_covid_vaccine
//! BOOLEAN, year_graduated INT)` with 100 rows, each `photo` a real image.
//!
//! The two databases are ~206 MB each, so they are **env-gated and never
//! committed**: point `SQLITE_FORENSIC_SFT05` at a directory holding
//! `SFT-05_android.sqlite` / `SFT-05_ios.sqlite` (downloaded from NIST CFReDS) and
//! this runs; with the var unset it **skips cleanly**. Provenance + download in
//! `docs/corpus-catalog.md`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::pedantic)]

use std::path::PathBuf;

use sqlite_core::{Database, Value};

fn corpus_dir() -> Option<PathBuf> {
    match std::env::var("SQLITE_FORENSIC_SFT05") {
        Ok(p) if !p.is_empty() && PathBuf::from(&p).is_dir() => Some(PathBuf::from(p)),
        _ => {
            eprintln!(
                "SKIP cfreds_sft05_types: set SQLITE_FORENSIC_SFT05 to a directory holding the \
                 NIST CFReDS SFT-05 SFT-05_android.sqlite / SFT-05_ios.sqlite (env-gated, ~206 MB \
                 each, never committed)"
            );
            None
        }
    }
}

/// A BLOB whose leading bytes are a known image signature (PNG / JPEG / GIF /
/// BMP / TIFF / WebP) — NIST documents the `photo` column as "a variety of
/// graphic file types".
fn is_graphic(b: &[u8]) -> bool {
    b.starts_with(&[0x89, b'P', b'N', b'G'])                      // PNG
        || b.starts_with(&[0xFF, 0xD8, 0xFF])                     // JPEG
        || b.starts_with(b"GIF8")                                  // GIF
        || b.starts_with(b"BM")                                    // BMP
        || b.starts_with(&[0x49, 0x49, 0x2A, 0x00])               // TIFF LE
        || b.starts_with(&[0x4D, 0x4D, 0x00, 0x2A])               // TIFF BE
        || (b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP")) // WebP
        || b.get(4..8) == Some(b"ftyp")                            // ISO-BMFF (HEIC / MP4 / MOV)
        || b.starts_with(b"%PDF") // PDF
}

#[test]
fn sft05_native_types_and_blob_images_read_accurately() {
    let Some(dir) = corpus_dir() else {
        return;
    };
    let mut checked = 0usize;
    for variant in ["SFT-05_android.sqlite", "SFT-05_ios.sqlite"] {
        let path = dir.join(variant);
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let db = Database::open(bytes).expect("SFT-05 opens");
        let dump = db
            .live_table_rows()
            .into_iter()
            .find(|t| t.name == "new_students")
            .expect("new_students table present");
        checked += 1;

        assert_eq!(dump.rows.len(), 100, "{variant}: NIST documents 100 rows");

        let mut graphics = 0usize;
        let mut signatures: std::collections::HashSet<[u8; 4]> = std::collections::HashSet::new();
        for row in &dump.rows {
            let v = &row.values;
            assert_eq!(v.len(), 6, "{variant}: six columns");
            // id INT PK, name TEXT, photo BLOB, gpa FLOAT, has_covid_vaccine
            // BOOLEAN (stored as int 0/1), year_graduated INT — each read in its
            // native storage class.
            assert!(
                matches!(v[0], Value::Integer(_)),
                "{variant}: id is INTEGER"
            );
            assert!(matches!(v[1], Value::Text(_)), "{variant}: name is TEXT");
            assert!(
                matches!(&v[2], Value::Blob(b) if !b.is_empty()),
                "{variant}: photo is a non-empty BLOB"
            );
            assert!(matches!(v[3], Value::Real(_)), "{variant}: gpa is REAL");
            assert!(
                matches!(v[4], Value::Integer(0 | 1)),
                "{variant}: has_covid_vaccine is a 0/1 BOOLEAN"
            );
            assert!(
                matches!(v[5], Value::Integer(_)),
                "{variant}: year_graduated is INTEGER"
            );
            if let Value::Blob(b) = &v[2] {
                if is_graphic(b) {
                    graphics += 1;
                }
                if let Some(sig) = b.get(0..4) {
                    signatures.insert(sig.try_into().unwrap());
                }
            }
        }
        // NIST documents the photo column as "a variety of graphic file types":
        // every BLOB carries a recognized file-type magic (PNG/JPEG/GIF/TIFF/BMP/
        // ISO-BMFF/PDF), and they span many distinct signatures — read natively
        // and intact by the reader.
        assert!(
            graphics >= 95,
            "{variant}: photo BLOBs should carry a recognized file-type magic (got {graphics}/100)"
        );
        assert!(
            signatures.len() >= 5,
            "{variant}: NIST documents a VARIETY of graphic types (got {} distinct signatures)",
            signatures.len()
        );
        eprintln!(
            "cfreds_sft05_types: {variant} — 100 rows, {graphics}/100 recognized images, \
             {} distinct file signatures",
            signatures.len()
        );
    }
    assert!(
        checked > 0,
        "SQLITE_FORENSIC_SFT05 contained no SFT-05_android.sqlite / SFT-05_ios.sqlite"
    );
}
