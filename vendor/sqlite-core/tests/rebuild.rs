//! Self-reader round-trip oracle for the pure-Rust rebuild writer.
//!
//! The writer ([`sqlite_core::rebuild::build_recovered_db`]) and the reader
//! ([`sqlite_core::Database::open`]) are independent code paths, so re-opening the
//! produced bytes and reading every row/value back is a genuine cross-check (one
//! of the feature's two oracles; the other is the real `sqlite3` CLI, in the
//! forensic crate's integration test).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use sqlite_core::rebuild::{
    build_recovered_db, build_recovered_db_with_fragments, FragmentRow, RebuildRow,
};
use sqlite_core::{Database, Value};

/// The fixed leading columns of `recovered_records`, in schema order.
const LEAD_COLS: usize = 5; // _page, _offset, _rowid, _source, _confidence

/// The fixed leading columns of `recovered_fragments`, in schema order.
const FRAG_LEAD_COLS: usize = 4; // _page, _offset, _missing, _confidence

fn frag(
    page: u32,
    offset: usize,
    missing: usize,
    confidence: f32,
    surviving: Vec<(usize, Value)>,
) -> FragmentRow {
    FragmentRow {
        page,
        offset,
        missing,
        confidence,
        surviving,
    }
}

/// Read a named user table back as rows of values, decoding a generous column
/// width so every column materializes. Panics if the table is absent.
fn read_named_table(db: &Database, name: &str) -> Vec<sqlite_core::Row> {
    let schema = db.read_table(1, 5).expect("schema readable");
    let root = schema
        .iter()
        .find_map(
            |r| match (r.values.first(), r.values.get(1), r.values.get(3)) {
                (Some(Value::Text(t)), Some(Value::Text(n)), Some(Value::Integer(root)))
                    if t == "table" && n == name =>
                {
                    Some(*root as u32)
                }
                _ => None,
            },
        )
        .unwrap_or_else(|| panic!("{name} table present in schema"));
    db.read_table(root, 64).expect("table readable")
}

fn row(rowid: Option<i64>, source: &str, confidence: f32, cells: Vec<Value>) -> RebuildRow {
    RebuildRow {
        page: 7,
        offset: 128,
        rowid,
        source: source.to_string(),
        confidence,
        cells,
    }
}

/// Read the rebuilt db's single table back as rows of values, keyed by rowid.
fn read_back(bytes: Vec<u8>) -> Vec<sqlite_core::Row> {
    let db = Database::open(bytes).expect("rebuilt db must re-open");
    // The table is the only user table; its rootpage is in sqlite_master col 3.
    let schema = db.read_table(1, 5).expect("schema readable");
    let root = schema
        .iter()
        .find_map(|r| match (r.values.first(), r.values.get(3)) {
            (Some(Value::Text(t)), Some(Value::Integer(root))) if t == "table" => {
                Some(*root as u32)
            }
            _ => None,
        })
        .expect("recovered_records table present in schema");
    // Column count = lead cols + max cells across rows; read_table tolerates a
    // larger count (missing trailing cols read as the row's own width), but we
    // pass a generous width so every column is decoded.
    db.read_table(root, 64).expect("table readable")
}

#[test]
fn single_small_row_round_trips_through_self_reader() {
    let rows = vec![row(
        Some(42),
        "freelist-page",
        0.9,
        vec![
            Value::Integer(7),
            Value::Text("alice".into()),
            Value::Real(1.5),
            Value::Blob(vec![0xDE, 0xAD, 0xBE, 0xEF]),
        ],
    )];
    let bytes = build_recovered_db(&rows);
    let back = read_back(bytes);
    assert_eq!(back.len(), 1, "one carved record -> one row");
    let r = &back[0];
    // Lead columns: _page, _offset, _rowid, _source, _confidence.
    assert_eq!(r.values[0], Value::Integer(7)); // _page
    assert_eq!(r.values[1], Value::Integer(128)); // _offset
    assert_eq!(r.values[2], Value::Integer(42)); // _rowid
    assert_eq!(r.values[3], Value::Text("freelist-page".into())); // _source
    match &r.values[4] {
        Value::Real(c) => assert!((*c - 0.9).abs() < 1e-6, "confidence {c}"),
        other => panic!("_confidence must be REAL, got {other:?}"),
    }
    // Native cell types preserved (no stringification of the blob).
    assert_eq!(r.values[LEAD_COLS], Value::Integer(7));
    assert_eq!(r.values[LEAD_COLS + 1], Value::Text("alice".into()));
    assert_eq!(r.values[LEAD_COLS + 2], Value::Real(1.5));
    assert_eq!(
        r.values[LEAD_COLS + 3],
        Value::Blob(vec![0xDE, 0xAD, 0xBE, 0xEF]),
        "BLOB stored natively, byte-for-byte"
    );
}

#[test]
fn unknown_rowid_is_null_and_missing_trailing_cells_are_null() {
    // Row A has 3 cells and a known rowid; row B has 1 cell and an unknown rowid.
    // N = max cell count - 1 = 2, so the table has c0,c1,c2. B's c1,c2 are NULL.
    let rows = vec![
        row(
            Some(10),
            "in-page-freeblock",
            0.8,
            vec![
                Value::Integer(1),
                Value::Text("x".into()),
                Value::Integer(3),
            ],
        ),
        row(
            None,
            "freeblock-reconstructed",
            0.4,
            vec![Value::Text("solo".into())],
        ),
    ];
    let bytes = build_recovered_db(&rows);
    let back = read_back(bytes);
    assert_eq!(back.len(), 2);
    // Rows come back in rowid order; the writer assigns its own row keys 1..=N.
    let a = back
        .iter()
        .find(|r| r.values[2] == Value::Integer(10))
        .unwrap();
    assert_eq!(a.values[LEAD_COLS], Value::Integer(1));
    assert_eq!(a.values[LEAD_COLS + 2], Value::Integer(3));
    let b = back.iter().find(|r| r.values[2] == Value::Null).unwrap();
    assert_eq!(b.values[3], Value::Text("freeblock-reconstructed".into()));
    assert_eq!(b.values[LEAD_COLS], Value::Text("solo".into()));
    // Missing trailing cells -> NULL.
    assert_eq!(b.values[LEAD_COLS + 1], Value::Null);
    assert_eq!(b.values[LEAD_COLS + 2], Value::Null);
}

#[test]
fn empty_input_produces_a_valid_empty_table() {
    let bytes = build_recovered_db(&[]);
    let back = read_back(bytes);
    assert!(back.is_empty(), "no rows -> empty (but valid) table");
}

#[test]
fn large_blob_spills_to_overflow_pages_and_round_trips() {
    // A multi-KB blob exceeds one page's usable size, forcing an overflow chain.
    // Distinctive byte pattern so a truncation/misassembly is caught exactly.
    let big: Vec<u8> = (0..9000u32).map(|i| (i % 251) as u8).collect();
    let rows = vec![row(
        Some(1),
        "freelist-page",
        0.95,
        vec![Value::Text("spilled".into()), Value::Blob(big.clone())],
    )];
    let bytes = build_recovered_db(&rows);
    let back = read_back(bytes);
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].values[LEAD_COLS], Value::Text("spilled".into()));
    assert_eq!(
        back[0].values[LEAD_COLS + 1],
        Value::Blob(big),
        "overflow-spilled BLOB reassembles byte-for-byte"
    );
}

// ---- two-table output: recovered_fragments alongside recovered_records -------

#[test]
fn fragments_land_at_their_native_column_index() {
    // Two records (so recovered_records is non-trivially populated) plus two
    // fragments whose surviving cells are SPARSE: fragment A has columns 0,1,2;
    // fragment B has ONLY column 2 (a BLOB). Each surviving (idx, value) must read
    // back at c{idx} in its native storage class, with NULL in every other column.
    let records = vec![
        row(Some(1), "freelist-page", 0.9, vec![Value::Integer(10)]),
        row(
            Some(2),
            "freelist-page",
            0.8,
            vec![Value::Text("r2".into())],
        ),
    ];
    let fragments = vec![
        frag(
            2,
            3965,
            1,
            0.2,
            vec![
                (0, Value::Integer(20004)),
                (1, Value::Text("Anja".into())),
                (2, Value::Text("Frank".into())),
            ],
        ),
        // Sparse: only column 2 present, and it is a BLOB (native, lossless).
        frag(
            5,
            64,
            2,
            0.2,
            vec![(2, Value::Blob(vec![0xCA, 0xFE, 0xBA]))],
        ),
    ];
    let bytes = build_recovered_db_with_fragments(&records, Some(&fragments));
    let db = Database::open(bytes).expect("rebuilt db must re-open");

    // recovered_records still holds the full rows.
    let recs = read_named_table(&db, "recovered_records");
    assert_eq!(recs.len(), 2, "both full records present");

    // recovered_fragments: M = max surviving col index = 2 -> c0,c1,c2.
    let frags = read_named_table(&db, "recovered_fragments");
    assert_eq!(frags.len(), 2, "both fragments present");

    // Fragment A: provenance lead cols then c0,c1,c2 = 20004,'Anja','Frank'.
    let a = frags
        .iter()
        .find(|r| r.values[FRAG_LEAD_COLS] == Value::Integer(20004))
        .expect("fragment A by its surviving c0");
    assert_eq!(a.values[0], Value::Integer(2)); // _page
    assert_eq!(a.values[1], Value::Integer(3965)); // _offset
    assert_eq!(a.values[2], Value::Integer(1)); // _missing
    match &a.values[3] {
        Value::Real(c) => assert!((*c - 0.2).abs() < 1e-6, "_confidence {c}"),
        other => panic!("_confidence must be REAL, got {other:?}"),
    }
    assert_eq!(a.values[FRAG_LEAD_COLS + 1], Value::Text("Anja".into())); // c1
    assert_eq!(a.values[FRAG_LEAD_COLS + 2], Value::Text("Frank".into())); // c2

    // Fragment B: only c2 (a BLOB) survives; c0 and c1 must be NULL.
    let b = frags
        .iter()
        .find(|r| r.values[FRAG_LEAD_COLS + 2] == Value::Blob(vec![0xCA, 0xFE, 0xBA]))
        .expect("fragment B by its surviving c2 BLOB");
    assert_eq!(
        b.values[FRAG_LEAD_COLS],
        Value::Null,
        "c0 NULL (not surviving)"
    );
    assert_eq!(
        b.values[FRAG_LEAD_COLS + 1],
        Value::Null,
        "c1 NULL (not surviving)"
    );
    assert_eq!(b.values[2], Value::Integer(2)); // _missing
}

#[test]
fn fragments_none_is_byte_identical_to_single_table_output() {
    // The --no-fragments path: passing None must reproduce the historical
    // single-table bytes exactly, so existing output and tests are unaffected.
    let records = vec![
        row(Some(1), "freelist-page", 0.9, vec![Value::Integer(10)]),
        row(
            None,
            "freeblock-reconstructed",
            0.4,
            vec![Value::Text("x".into())],
        ),
    ];
    let single = build_recovered_db(&records);
    let two_table_none = build_recovered_db_with_fragments(&records, None);
    assert_eq!(
        single, two_table_none,
        "None fragments must be byte-identical to the single-table writer"
    );
}

#[test]
fn empty_fragment_set_still_creates_the_table() {
    // Fragments enabled but none found -> an EMPTY recovered_fragments table with
    // the right (minimal) schema, present and queryable. Predictable, not absent.
    let records = vec![row(Some(1), "freelist-page", 0.9, vec![Value::Integer(10)])];
    let bytes = build_recovered_db_with_fragments(&records, Some(&[]));
    let db = Database::open(bytes).expect("rebuilt db must re-open");
    let frags = read_named_table(&db, "recovered_fragments");
    assert!(frags.is_empty(), "no fragments -> empty fragment table");
    // recovered_records is still intact.
    assert_eq!(read_named_table(&db, "recovered_records").len(), 1);
}
