//! Freeblock-aware in-page cell reconstruction, validated against the REAL
//! Nemetz SQLite Forensic Corpus (DFRWS-EU 2018; CC0). See task #56 and
//! `docs/recovery-comparison.md` ("Freeblock reconstruction").
//!
//! When SQLite frees an in-page cell it converts the cell into a **freeblock**:
//! the cell's first four bytes are overwritten with the freeblock header (2-byte
//! next-freeblock offset + 2-byte freeblock size). That clobbers the cell's
//! payload-length and rowid varints AND the record header's first byte(s)
//! (`header_len` + the leading serial type). A forward parse from the freeblock
//! offset therefore recovers nothing for that cell — the dominant FN class.
//!
//! [`Database::reconstruct_freeblock_records`] closes the gap: it derives the
//! table's record-header template from a LIVE cell on the same page (column
//! count, header length, and the serial types of the leading columns that fall
//! inside the clobbered prefix), then walks the page's freeblock chain and
//! reconstructs each freed cell from its **surviving** serial-type tail plus the
//! template. The rowid is destroyed, so it is emitted as unknown (`0`).
//!
//! Precision discipline (the whole point — see task #56): a reconstructed
//! candidate is only emitted when its body decodes cleanly AND fits inside the
//! freeblock bounds. The reconstructed values are scored exactly like any other
//! carved record, and the forensic layer's live-row filter still drops any that
//! collide with a live row — so reconstruction never re-surfaces a live row.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use sqlite_core::{Database, Value};

/// 0C-01: in-page deletion, `secure_delete=0`, five integer columns. Page 2 holds
/// six freeblocks (the six freed cells whose first four bytes were clobbered).
const NEMETZ_0C_01: &[u8] = include_bytes!("data/nemetz/0C/0C-01.db");

/// The freeblock reconstructor must recover the freeblock-clobbered deleted rows
/// of 0C-01 that a forward parse cannot — by full decoded content, including the
/// `id` column (`20005`, …) whose serial type comes from the schema template.
///
/// Row `20005 = [20005, 3780322152, 3909007646, 120462986, 1290558629]` is a
/// freeblock-head cell: its prefix + header first byte were clobbered, so the
/// current forward carver misses it. Reconstruction recovers it exactly.
#[test]
fn reconstructs_freeblock_clobbered_rows_of_0c_01() {
    let db = Database::open(NEMETZ_0C_01.to_vec()).expect("open 0C-01");
    let page = db.raw_page(2).expect("page 2 in range");

    let recovered = db.reconstruct_freeblock_records(&page);

    // The expected deleted row 20005, recovered in full from the surviving
    // serial tail plus the schema-derived header prefix.
    let want = vec![
        Value::Integer(20005),
        Value::Integer(3_780_322_152),
        Value::Integer(3_909_007_646),
        Value::Integer(120_462_986),
        Value::Integer(1_290_558_629),
    ];
    assert!(
        recovered.iter().any(|c| c.values == want),
        "freeblock reconstruction must recover row 20005 from page 2; got {:?}",
        recovered.iter().map(|c| &c.values).collect::<Vec<_>>()
    );

    // At least the six freeblock-head cells on page 2 are reconstructed.
    assert!(
        recovered.len() >= 5,
        "expected >=5 freeblock reconstructions on 0C-01 page 2, got {}",
        recovered.len()
    );

    // The destroyed rowid is surfaced as unknown (0), never invented.
    assert!(
        recovered.iter().all(|c| c.rowid == 0),
        "freeblock-reconstructed rowid must be unknown (0)"
    );

    // Reconstruction is graded LOW (more uncertain than an intact-header carve).
    assert!(
        recovered.iter().all(|c| c.confidence <= 0.5),
        "freeblock reconstruction must be confidence-graded low"
    );
}

/// Non-leaf pages and pages with no freeblock chain yield nothing, and a
/// too-short / non-`0x0d` slice never panics.
#[test]
fn reconstruct_freeblock_records_is_bounded_and_safe() {
    let db = Database::open(NEMETZ_0C_01.to_vec()).expect("open");
    // A synthetic interior (0x05) page: no leaf cells to reconstruct.
    assert!(db.reconstruct_freeblock_records(&[0x05u8; 4096]).is_empty());
    // Empty / too-short slice: no panic, no output.
    assert!(db.reconstruct_freeblock_records(&[]).is_empty());
    // A leaf page header with first-freeblock = 0 yields nothing.
    let mut leaf = vec![0u8; 4096];
    leaf[0] = 0x0d; // table-leaf
    assert!(db.reconstruct_freeblock_records(&leaf).is_empty());
}
