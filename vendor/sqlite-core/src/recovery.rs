//! Deleted-record recovery over the reader: carve every free-space class,
//! attribute each record to a table, recover dropped schema rows, and diff a
//! rollback journal's prior state.
//!
//! Ported from the `sqlite-forensic` analyzer crate (same repository and
//! commit as this reader, Apache-2.0), without its `forensicnomicon` report
//! model, anomaly audit, fragment tier, CASE/UCO export or carver registry.
//! The recovery logic is unchanged.

use crate::{Database, JournalHeader, RollbackJournal, Value};

/// Which class of free space a deleted record was carved from. Records the
/// recovery provenance so the examiner can weigh reliability by class.
///
/// Exhaustive in this vendored copy (upstream marks it `#[non_exhaustive]`) so
/// the application has to place every new class deliberately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoverySource {
    /// A whole page that was freed onto the freelist (the strongest case: the
    /// page holds only deallocated content).
    FreelistPage,
    /// The in-page free space (unallocated gap / freeblock slack) of a page that
    /// is still allocated. The record is genuinely deleted but more likely to be
    /// partially overwritten, so it is graded lower.
    InPageFreeBlock,
    /// A page whose table was `DROP`ped — on the freelist with no `sqlite_master`
    /// schema, so the column count was inferred from the record itself.
    DroppedTable,
    /// A **prior version** of a still-live row: an `UPDATE` freed the old version
    /// of the row into slack while the new version kept the same rowid. The
    /// recovered values DIFFER from the current live row (e.g. an edited message
    /// or a changed amount), so it is genuine deleted content — the edit history.
    PriorVersion,
    /// A record rebuilt by **freeblock reconstruction**: the freed cell's first
    /// four bytes (payload-length + rowid varints, `header_len`, and the leading
    /// serial type) were overwritten by SQLite's freeblock header, so the record
    /// was rebuilt from its surviving serial-type tail plus a schema template. The
    /// rowid is destroyed (surfaced as `0`), so this is the weakest in-page class
    /// — a low-confidence "consistent with a deleted row" lead.
    FreeblockReconstructed,
    /// Residue carved from an **uncheckpointed WAL frame's page image** rather
    /// than the on-disk pages. A `-wal` frame holds a committed page version the
    /// main file does not yet reflect; deleted cells freed within that version
    /// survive in the frame's slack and exist NOWHERE on disk. The record carries
    /// the `(salt1, salt2, frame_index)` log-sequence provenance in
    /// [`CarvedRecord::wal`].
    WalFrame,
    /// Residue carved from a **materialized commit snapshot** — the database state
    /// replayed up to one COMMIT frame of the `-wal` (base image ∪ committed frames
    /// to that commit). A row that is a live cell at this commit but deleted by a
    /// later commit survives ONLY in this snapshot's page images. The record
    /// carries the commit's `(salt1, salt2, commit_frame_index)` LSN in
    /// [`CarvedRecord::wal`] — the per-commit temporal coordinate, distinct from a
    /// raw [`RecoverySource::WalFrame`] residue's frame index.
    CommitSnapshot,
    /// A **prior allocated row from the rollback journal** (design §5). The row
    /// was LIVE in the pre-transaction state the `-journal` preserves and the last
    /// transaction deleted or modified it — it is NOT a free-space residue, so
    /// downstream reports must not relabel it as unallocated. Carries the page /
    /// segment / header-state / checksum provenance in [`RollbackJournalSource`].
    RollbackJournal(RollbackJournalSource),
}

/// Whether a rollback-journal header was parsed intact (Tier A) or reconstructed
/// from a zeroed (PERSIST post-commit) header (Tier B). A reconstructed header
/// could not verify page checksums (the nonce was zeroed), so its recoveries
/// rest on cross-record structural consistency — a graded distinction the
/// examiner weighs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalHeaderState {
    /// Tier A: header magic present; the checksum nonce was known and verified.
    Valid,
    /// Tier B: header zeroed (PERSIST post-commit); parameters reconstructed,
    /// checksums unverifiable.
    ReconstructedZeroed,
}

/// Provenance for a record recovered from a rollback journal (design §5):
/// where in the journal its page image lived and how trustworthy the parse was.
/// `Copy` (all fields are scalar) so it threads through the existing
/// attribution → output pipeline exactly like [`WalProvenance`]; the journal's
/// path is supplied once at the API boundary, not duplicated per row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RollbackJournalSource {
    /// 0-based journal segment the page image came from.
    pub segment: usize,
    /// 1-based database page number the prior image restored.
    pub pgno: u32,
    /// Whether the header was intact (Tier A) or reconstructed (Tier B).
    pub header_state: JournalHeaderState,
    /// `Some(true/false)` when the page checksum could be verified (Tier A);
    /// `None` in Tier B (nonce zeroed).
    pub checksum_valid: Option<bool>,
}

/// Provenance for a record carved from a `-wal` frame: the
/// `(salt1, salt2, frame_index)` log-sequence identity of the frame it came from
/// (the LSN task #55 will formalize).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalProvenance {
    /// 0-based position of the source frame within the `-wal` file.
    pub frame_index: usize,
    /// WAL header salt-1 (checkpoint generation) of the source frame.
    pub salt1: u32,
    /// WAL header salt-2 (checkpoint generation) of the source frame.
    pub salt2: u32,
}

/// Provenance for a record reassembled across an **overflow-page chain** (task
/// #73): the pages whose bytes were concatenated to recover the row. An examiner
/// citing the row as evidence can name exactly where its bytes came from. Present
/// only on chain-reassembled rows; `None` for every contiguous recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverflowProvenance {
    /// First overflow page of the chain (the page the local-prefix pointer named).
    pub first_page: u32,
    /// Ordered overflow pages whose content was assembled into the payload.
    pub chain: Vec<u32>,
}

/// A deleted record recovered from unallocated space — the headline capability
/// rusqlite cannot provide. Carries the decoded row plus provenance so the
/// examiner can weigh it as a "consistent with a deleted row" observation.
#[derive(Debug, Clone, PartialEq)]
pub struct CarvedRecord {
    /// 1-based page the record was carved from.
    pub page: u32,
    /// Byte offset of the cell within that page.
    pub offset: usize,
    /// Recovered rowid.
    pub rowid: i64,
    /// Decoded column values, in column order.
    pub values: Vec<Value>,
    /// Heuristic confidence in `(0.0, 1.0]` that these bytes are a real record.
    pub confidence: f32,
    /// Always `false`: a carved record lives in unallocated space, never in the
    /// live b-tree. Present so callers cannot mistake it for an allocated row.
    pub allocated: bool,
    /// Which class of free space this record was recovered from.
    pub source: RecoverySource,
    /// WAL log-sequence provenance, present **only** for
    /// [`RecoverySource::WalFrame`] records (the frame the residue was carved
    /// from); `None` for every on-disk class.
    pub wal: Option<WalProvenance>,
    /// Overflow-chain provenance, present **only** for rows reassembled across a
    /// freed overflow-page chain (task #73); `None` for every contiguous recovery.
    pub overflow: Option<OverflowProvenance>,
}

/// Recover deleted records across **every** free-space class — the full-coverage
/// carver. Drives, in order:
///
/// 1. **Freelist pages** (whole pages freed onto the freelist), with the column
///    count inferred per record so it also recovers **dropped-table** pages whose
///    `sqlite_master` schema is gone (e.g. a `DROP TABLE` left the page on the
///    freelist with no recorded column count).
/// 2. **In-page free space** of still-allocated table-leaf pages (the unallocated
///    gap and inter-cell slack), via [`Database::carve_free_regions`], which
///    carves only the complement of the live cells — so a live (allocated) row is
///    **never** re-surfaced (the 0-false-positive guarantee, enforced
///    structurally).
///
/// Records are de-duplicated by `(rowid, values)` keeping the highest-confidence
/// copy, since a row can survive in more than one place. Every record is graded:
/// freelist-page recovery highest, dropped-table next, in-page residue lowest.
///
/// Read-only and panic-free; a malformed structure yields fewer records, never an
/// error or panic.
#[must_use]
pub fn carve_all_deleted_records(db: &Database) -> Vec<CarvedRecord> {
    let mut out: Vec<CarvedRecord> = Vec::new();

    // (1) Freelist pages, inferring the column count per record. Inference makes
    // this recover both normal freed pages AND schema-gone dropped-table pages
    // (a `DROP TABLE` leaves the page on the freelist with no recorded column
    // count). When the database has no live user table at all, the freed content
    // is necessarily from a dropped table, so mark those records accordingly.
    let dropped_table_db = !db.has_user_table();
    if let Ok(free) = db.freelist_pages() {
        for page in free {
            let Some(page_bytes) = db.raw_page(page) else {
                continue; // cov:unreachable: freelist_pages yields in-range pages
            };
            let source = if dropped_table_db {
                RecoverySource::DroppedTable
            } else {
                RecoverySource::FreelistPage
            };
            for cell in db.carve_cells_inferred(&page_bytes) {
                out.push(CarvedRecord {
                    page,
                    offset: cell.offset,
                    rowid: cell.rowid,
                    values: cell.values,
                    confidence: cell.confidence,
                    allocated: false,
                    source,
                    wal: None,
                    overflow: None,
                });
            }
        }
    }

    // (2) In-page free space of every still-allocated table-leaf page.
    let page_count = db.page_count();
    for page in 1..=page_count {
        let Some(page_bytes) = db.raw_page(page) else {
            continue; // cov:unreachable: 1..=page_count is in range
        };
        for cell in db.carve_free_regions(&page_bytes, 0) {
            out.push(CarvedRecord {
                page,
                offset: cell.offset,
                rowid: cell.rowid,
                values: cell.values,
                confidence: cell.confidence,
                allocated: false,
                source: RecoverySource::InPageFreeBlock,
                wal: None,
                overflow: None,
            });
        }
        // (2b) Freeblock reconstruction: the freed cells whose first four bytes
        // were clobbered by freeblock conversion, rebuilt from their surviving
        // serial tail plus the page's schema template. These carry an unknown
        // (destroyed) rowid, so the value-collision pass below — not the
        // rowid-keyed filter — is what guarantees no live row is re-surfaced.
        for cell in db.reconstruct_freeblock_records(&page_bytes) {
            out.push(CarvedRecord {
                page,
                offset: cell.offset,
                rowid: cell.rowid,
                values: cell.values,
                confidence: cell.confidence,
                allocated: false,
                source: RecoverySource::FreeblockReconstructed,
                wal: None,
                overflow: None,
            });
        }
        // (2c) Chain-aware overflow recovery (task #73): a freed cell whose
        // payload spilled onto an overflow-page chain, reassembled when every
        // chain page survives as a freelist leaf. Graded below the in-page
        // full-row tier (NOT part of the structural 0-FP guarantee — Codex
        // ruling #1); a broken chain (e.g. a trunk-clobbered page) is rejected
        // here and degrades to a Tier-2 fragment elsewhere.
        for (cell, chain) in db.carve_overflow_records(&page_bytes) {
            let first_page = chain.first().copied().unwrap_or(0);
            out.push(CarvedRecord {
                page,
                offset: cell.offset,
                rowid: cell.rowid,
                values: cell.values,
                confidence: cell.confidence,
                allocated: false,
                source: RecoverySource::InPageFreeBlock,
                wal: None,
                overflow: Some(OverflowProvenance { first_page, chain }),
            });
        }
        // (2d) Freeblock-clobbered spilled cells (task #73, Codex ruling #5;
        // UNPROVEN-BY-CORPUS — synthetic validation only). A spilled cell whose
        // prefix was also freeblock-clobbered: P re-derived from the surviving
        // serial array, chain resolved through freelist leaves, rowid destroyed.
        for (cell, chain) in db.carve_overflow_template_records(&page_bytes) {
            let first_page = chain.first().copied().unwrap_or(0);
            out.push(CarvedRecord {
                page,
                offset: cell.offset,
                rowid: cell.rowid,
                values: cell.values,
                confidence: cell.confidence,
                allocated: false,
                source: RecoverySource::FreeblockReconstructed,
                wal: None,
                overflow: Some(OverflowProvenance { first_page, chain }),
            });
        }
    }

    // (3) WAL-frame carving (additive — runs ONLY when a `-wal` overlay is in
    // effect). The on-disk path above reads only the main file's pages, so it
    // finds the SAME residue whether or not a WAL was supplied; the genuinely-
    // different deleted records live in the uncheckpointed WAL frames.
    //
    // A `-wal` frame is a FULL page snapshot at one point in the transaction
    // history. A row deleted late in that history is still a live cell in an
    // EARLIER frame's image; it exists nowhere on disk and in no later frame.
    // Three primitives over each committed frame's page image surface it:
    //
    //   * `carve_leaf_cells` — every cell the frame records as allocated. A cell
    //     that is allocated in a superseded frame but ABSENT from the final
    //     WAL-applied live view is exactly such a deleted row (recovered as a
    //     clean, intact record — the strongest WAL case).
    //   * `carve_free_regions` / `reconstruct_freeblock_records` — residue freed
    //     WITHIN a frame (e.g. the DELETE-commit frame's own freeblocks), matching
    //     the on-disk in-page classes.
    //
    // Every candidate is tagged `WalFrame` with the (salt1, salt2, frame_index)
    // LSN provenance, and the shared live-row precision filter below drops any
    // whose values match a currently-live row — so a surviving row is never
    // re-surfaced (the 0-false-positive guarantee, against the WAL-applied view).
    for frame in db.wal_frame_pages() {
        let prov = WalProvenance {
            frame_index: frame.frame_index,
            salt1: frame.salt1,
            salt2: frame.salt2,
        };
        let cells = db
            .carve_leaf_cells(&frame.page)
            .into_iter()
            .chain(db.carve_free_regions(&frame.page, 0))
            .chain(db.reconstruct_freeblock_records(&frame.page));
        for cell in cells {
            out.push(CarvedRecord {
                page: frame.page_no,
                offset: cell.offset,
                rowid: cell.rowid,
                values: cell.values,
                confidence: cell.confidence,
                allocated: false,
                source: RecoverySource::WalFrame,
                wal: Some(prov),
                overflow: None,
            });
        }
    }

    // VALUE-AWARE classification for carved records whose rowid is currently
    // LIVE. Two very different cases share a live rowid:
    //
    //   * Stale rebalance copy — a b-tree rebalance moved a still-live row to
    //     another page, leaving a byte-identical copy in the old page's slack.
    //     SAME rowid, SAME values → NOT deleted → drop (the 0-false-positive
    //     precision win we must preserve).
    //   * Prior version — an UPDATE freed the OLD version of the row into slack
    //     while the new version kept the same rowid. SAME rowid, DIFFERENT values
    //     → genuinely-deleted content (the edited message / changed amount, often
    //     THE evidence) → recover, tagged `PriorVersion`.
    //
    // A rowid-only filter cannot tell these apart and drops both (a false
    // negative on prior versions). Comparing decoded values does.
    // Structural-noise rejection (precision, not the live-row guarantee): drop
    // any carved record that carries no content — a rowid echo followed by an
    // all-NULL tail, the inferred over-read of free-space zero bytes. Applied to
    // every tier uniformly (no per-source special-case).
    out.retain(|rec| !is_structural_noise(&rec.values));

    let live_tables = db.live_table_rows();
    let page_table = db.page_to_table_map();
    // COMPLETE value-level identity of every live row, across ALL tables and
    // UNCOLLAPSED. `live_rows()` keys by a global rowid, so two tables sharing a
    // rowid collapse to one entry and a live row can vanish from the guard; build
    // the guard from every live row (roadmap §1.1) plus the CURRENT `sqlite_master`
    // rows (a record carved from a materialized page 1 whose values equal a live
    // schema entry is that live row re-surfaced, not deleted residue — value-based,
    // so a genuinely-deleted PRIOR schema version is still recovered).
    let live_values: Vec<&Vec<Value>> = live_tables
        .iter()
        .flat_map(|table| table.rows.iter().map(|row| &row.values))
        .collect();
    let live_value_keys: std::collections::HashSet<String> = live_values
        .iter()
        .copied()
        .chain(db.live_schema_rows().iter())
        .map(|v| format!("{v:?}"))
        .collect();
    // LovelyCarve: a live row reads its INTEGER PRIMARY KEY column as the rowid,
    // but a reconstruction whose rowid was destroyed decodes that column as NULL,
    // so a stale copy of a live row (left in slack by a page move) escaped the
    // exact-value guard. Compare such records on the remaining columns too.
    let live_masked_keys: std::collections::HashSet<String> =
        live_values.iter().map(|v| masked_key(v)).collect();
    // LovelyCarve: live rowids per table. Upstream tested the record's rowid
    // against every table, so in a multi-table database (rowids restart at 1 in
    // each) freed or dropped-table rows were mislabelled as prior versions.
    let live_rowids: std::collections::HashMap<&str, std::collections::HashSet<i64>> = live_tables
        .iter()
        .map(|table| {
            (
                table.name.as_str(),
                table.rows.iter().map(|row| row.rowid).collect(),
            )
        })
        .collect();
    out.retain_mut(|rec| {
        // Exclusion invariant (EVERY source): a record whose decoded values equal
        // ANY currently-live row is that live row re-surfaced — never report it as
        // deleted. This guards the cross-table rowid-collision case the rowid-keyed
        // branch below cannot, since the global live map collapses shared rowids.
        if live_value_keys.contains(&format!("{:?}", rec.values)) {
            return false;
        }
        if rowid_alias_lost(rec) && live_masked_keys.contains(&masked_key(&rec.values)) {
            return false;
        }
        // Freeblock reconstructions (destroyed rowid) and WAL-frame residue carry no
        // rowid usable by the branch below; the value guard above already protected
        // them, so keep them with their source tag intact.
        if rec.source == RecoverySource::FreeblockReconstructed
            || rec.source == RecoverySource::WalFrame
        {
            return true;
        }
        // A rowid still live in the table that owns the record's page, with
        // DIFFERING values (exact matches were dropped above), is a deleted prior
        // version of that row; otherwise it is an ordinary deletion. Either way the
        // record is kept — only its classification changes.
        let live_in_same_table = page_table
            .get(&rec.page)
            .and_then(|table| live_rowids.get(table.as_str()))
            .is_some_and(|rowids| rowids.contains(&rec.rowid));
        if live_in_same_table {
            rec.source = RecoverySource::PriorVersion;
        }
        true
    });

    dedup_keep_best(out, &page_table)
}

/// Whether a record's rowid was destroyed (freeblock conversion overwrote it)
/// while its first column decoded as NULL — the stored form of an `INTEGER
/// PRIMARY KEY`, which a live row reads back as its rowid.
fn rowid_alias_lost(rec: &CarvedRecord) -> bool {
    rec.rowid == 0 && matches!(rec.values.first(), Some(Value::Null))
}

/// Identity of a row ignoring its first column.
fn masked_key(values: &[Value]) -> String {
    format!("{:?}", values.get(1..).unwrap_or_default())
}

/// A carved full record is **structural noise** — not an information-bearing
/// deleted row — when every column past the first is NULL. The inferred carver,
/// lacking a column hint, can read a run of free-space zero bytes as a long
/// serial-type-0 (NULL) array; that surfaces as a rowid echo (the INTEGER-PK
/// column) followed by an all-NULL tail, carrying no recoverable content. A
/// genuine row — live, dropped-table, or overflow — has a non-NULL value in some
/// non-leading column.
///
/// The column count is deliberately NOT bounded against the live tables: a
/// dropped table can legitimately be wider than every surviving one, so a
/// width cap would discard genuine unattributed records. Requiring length >= 2
/// keeps a real single-column row from being mistaken for noise.
fn is_structural_noise(values: &[Value]) -> bool {
    values.len() >= 2 && values.iter().skip(1).all(|v| matches!(v, Value::Null))
}

/// A schema-table (`sqlite_master`) row recovered from free space — a **dropped
/// or replaced** table/index/view/trigger whose definition no longer appears in
/// the live schema. Recovering it tells the examiner an object *existed* and its
/// structure (the `CREATE` statement), which in turn explains residual data on
/// the freelist. A finding, not a certainty: it is "consistent with a dropped
/// `<name>`" — the examiner draws the conclusion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredSchema {
    /// `sqlite_master.type` — `table`, `index`, `view`, or `trigger`.
    pub object_type: String,
    /// `sqlite_master.name` — the object's name.
    pub name: String,
    /// `sqlite_master.tbl_name` — the table the object belongs to (== `name` for
    /// a table).
    pub tbl_name: String,
    /// `sqlite_master.rootpage` — the (now-freed) b-tree root, `None` when the
    /// recovered cell stored it as NULL (views/triggers).
    pub rootpage: Option<i64>,
    /// `sqlite_master.sql` — the recovered `CREATE` statement.
    pub sql: String,
}

/// Recover **dropped or replaced** schema definitions from free space: carved
/// `sqlite_master`-shaped rows whose `(name, sql)` no longer matches a live
/// schema entry. A `DROP TABLE`/`DROP INDEX` deletes the object's `sqlite_master`
/// row (its bytes survive in page-1 free space under `secure_delete=OFF`); this
/// surfaces that residue as a structured [`RecoveredSchema`] rather than a raw
/// 5-column carved record.
#[must_use]
pub fn recover_dropped_schemas(db: &Database) -> Vec<RecoveredSchema> {
    use std::collections::HashSet;
    // Live `(name, sql)` pairs — an exactly-live definition carved from page-1
    // residue is a re-surfaced live row, never a drop.
    let live: HashSet<(String, String)> = db.schema_sql().into_iter().collect();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut out = Vec::new();
    for rec in carve_all_deleted_records(db) {
        // A `sqlite_master` row lives only on page 1 (plus its overflow); restrict
        // there so a 5-column data record elsewhere is never mistaken for schema.
        if rec.page != 1 {
            continue;
        }
        let Some(schema) = as_schema_row(&rec.values) else {
            continue;
        };
        let key = (schema.name.clone(), schema.sql.clone());
        if live.contains(&key) {
            continue;
        }
        if seen.insert(key) {
            out.push(schema);
        }
    }
    out
}

/// Interpret a carved record as a `sqlite_master` row
/// `(type, name, tbl_name, rootpage, sql)`, or `None` if it does not match that
/// shape. `type` must be one of the four schema object kinds, the `name` /
/// `tbl_name` / `sql` columns TEXT, and `rootpage` an INTEGER (or NULL for
/// views/triggers).
fn as_schema_row(values: &[Value]) -> Option<RecoveredSchema> {
    let [type_v, name_v, tbl_v, root_v, sql_v] = values else {
        return None;
    };
    let object_type = match type_v {
        Value::Text(t) if matches!(t.as_str(), "table" | "index" | "view" | "trigger") => t.clone(),
        _ => return None,
    };
    let (Value::Text(name), Value::Text(tbl_name), Value::Text(sql)) = (name_v, tbl_v, sql_v)
    else {
        return None;
    };
    let rootpage = match root_v {
        Value::Integer(n) => Some(*n),
        Value::Null => None,
        _ => return None,
    };
    Some(RecoveredSchema {
        object_type,
        name: name.clone(),
        tbl_name: tbl_name.clone(),
        rootpage,
        sql: sql.clone(),
    })
}

/// De-duplicate carved records by content identity, keeping the
/// highest-confidence copy of each (a row can survive in several free regions).
///
/// `Value` carries an `f64` (`Real`) so it is not `Hash`/`Eq`; the identity key
/// is the record's `rowid` plus a stable `Debug` rendering of its values, which
/// is sufficient to collapse byte-identical recoveries.
fn dedup_keep_best(
    mut records: Vec<CarvedRecord>,
    page_table: &std::collections::BTreeMap<u32, String>,
) -> Vec<CarvedRecord> {
    use std::collections::HashSet;
    let content = |r: &CarvedRecord| format!("{}:{:?}", r.rowid, r.values);

    // Identities (rowid, values) that have at least one record attributed to a
    // LIVE table. A copy of such an identity carved from a freed/unattributed page
    // is the SAME deleted row surviving in two places; it is folded into the
    // attributed copy rather than double-listed (roadmap §1.2).
    let attributed: HashSet<String> = records
        .iter()
        .filter(|r| page_table.contains_key(&r.page))
        .map(&content)
        .collect();

    let mut seen: HashSet<String> = HashSet::new();
    let mut kept: Vec<CarvedRecord> = Vec::new();
    // Highest confidence first, so the kept copy of each identity is the best one.
    records.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for rec in records.drain(..) {
        let c = content(&rec);
        // Table-aware identity (roadmap §1.2): a record from a page a live table
        // owns is keyed by (table, rowid, values) — the same row recovered from
        // several free regions of ONE table collapses, but two DIFFERENT live
        // tables sharing (rowid, values) stay distinct. An unattributed copy of an
        // identity that IS attributed elsewhere is the same deleted row → fold it
        // in. Otherwise (no live-table copy at all) keep the plain content identity.
        let key = match page_table.get(&rec.page) {
            Some(table) => format!("T:{table}:{c}"),
            None if attributed.contains(&c) => continue,
            None => format!("U:{c}"),
        };
        if seen.insert(key) {
            kept.push(rec);
        }
    }
    kept
}

/// A row recovered from the prior (pre-transaction) snapshot of a rollback
/// journal (design §5). In the prior state it was a LIVE allocated row that the
/// last transaction DELETED — so it is recovered at full fidelity and is NOT a
/// free-space carve (`allocated` was true in the prior state). Carries full
/// journal provenance in [`RollbackJournalSource`].
#[derive(Debug, Clone, PartialEq)]
pub struct PriorRow {
    /// The table the row belonged to (prior schema).
    pub table: String,
    /// The row's rowid (its INTEGER PRIMARY KEY for a rowid table).
    pub rowid: i64,
    /// The decoded prior column values, in column order.
    pub values: Vec<Value>,
    /// Journal provenance: page / segment / header-state / checksum status.
    pub source: RecoverySource,
}

/// A row present in BOTH the prior snapshot and the live db with DIFFERENT values
/// — the last transaction modified it (design §4). Carries the PRIOR (pre-edit)
/// values and the CURRENT values. `replaced_rowid` is set because a delete +
/// insert reusing the same rowid is indistinguishable from an in-place UPDATE
/// here, so identity may differ — never asserted as "UPDATE".
#[derive(Debug, Clone, PartialEq)]
pub struct PriorVersionRecord {
    /// The table the row belonged to (prior schema).
    pub table: String,
    /// The rowid present in both states.
    pub rowid: i64,
    /// The decoded PRIOR (pre-transaction) column values.
    pub prior_values: Vec<Value>,
    /// The decoded CURRENT (live) column values.
    pub current_values: Vec<Value>,
    /// Always `true`: the rowid is shared but identity may differ (delete+insert
    /// reusing a rowid is indistinguishable from an UPDATE), so the prior value is
    /// edit history, not an asserted in-place update.
    pub replaced_rowid: bool,
    /// Journal provenance for the prior image.
    pub source: RecoverySource,
}

/// Structured counts of what the rollback-journal recovery found — the NIST
/// SFT-03 "number of deleted/modified records" report (design §5/§6.11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct JournalCounts {
    /// Rows deleted by the last transaction (prior \ current by rowid).
    pub deleted: usize,
    /// Rows modified by the last transaction (present in both, values differ).
    pub modified: usize,
}

/// The result of recovering deletions + modifications from a rollback journal
/// (design §5). Deleted rows are full-fidelity prior allocated rows (NOT
/// free-space carves); modified rows carry the prior and current values.
#[derive(Debug, Clone, PartialEq)]
pub struct JournalRecovery {
    /// Rows present in the prior snapshot but absent now — deleted by the last
    /// transaction, recovered at full fidelity.
    pub deleted: Vec<PriorRow>,
    /// Rows present in both, values differ — prior (pre-edit) + current values.
    pub modified: Vec<PriorVersionRecord>,
    /// Structured counts (the NIST source-file report).
    pub counts: JournalCounts,
}

/// Recover the last transaction's **deletions and modifications** from a rollback
/// `-journal` by diffing the pre-transaction snapshot against the live database
/// (design §4) — the temporal inverse of WAL recovery.
///
/// For each prior rowid table, the prior rows (read through `PriorSnapshot`)
/// are diffed against the current rows by rowid:
/// - **deleted** = rowid in prior but not current → a full-fidelity [`PriorRow`];
/// - **modified** = rowid in both with different values → a [`PriorVersionRecord`]
///   flagged `replaced_rowid` (identity may differ; never asserted as UPDATE);
/// - **inserted** (rowid only in current) is a timeline fact, not a recovery target.
///
/// `WITHOUT ROWID` tables are excluded (no integer rowid key; design §4 limit).
/// Read-only and panic-free: a malformed/truncated journal, or a WAL-applied db
/// (the modes are exclusive), yields an empty recovery rather than an error or
/// panic — the journal source is bound to THIS database via
/// [`Database::rollback_prior`].
#[must_use]
pub fn carve_rollback_journal(db: &Database, journal: &[u8]) -> JournalRecovery {
    let mut deleted = Vec::new();
    let mut modified = Vec::new();

    let Ok(prior) = db.rollback_prior(journal) else {
        return JournalRecovery {
            deleted,
            modified,
            counts: JournalCounts::default(),
        };
    };
    // Header state + checksum status are uniform across a single journal; carry
    // them on every recovered row's provenance.
    let Some(header_state) = prior_header_state(journal, db) else {
        // cov:unreachable: rollback_prior above already parsed the same journal,
        // so a re-parse here cannot fail; the arm degrades to an empty recovery.
        return JournalRecovery {
            deleted,
            modified,
            counts: JournalCounts::default(),
        };
    };
    // Per-row checksum status is not threaded through PriorSnapshot in v1, so the
    // verifiable fact recorded here is the journal-level one: Tier A could verify
    // checksums (nonce known), Tier B could not (nonce zeroed). A finer per-image
    // status is a later enhancement; `header_state` carries the Tier either way.
    let checksum_valid = match header_state {
        JournalHeaderState::Valid => Some(true),
        JournalHeaderState::ReconstructedZeroed => None,
    };

    // Index the current (live) rows per table by rowid, so the diff is O(rows).
    let live = db.live_table_rows();

    for ptable in prior.tables() {
        if ptable.without_rowid {
            continue; // design §4 limit: WITHOUT ROWID excluded from the rowid diff.
        }
        let col_count = ptable.columns.len();
        let Ok(prior_rows) = prior.read_table_with_pages(ptable.rootpage, col_count) else {
            continue;
        };
        // Current rows of the same-named table (live schema), by rowid.
        let current: std::collections::BTreeMap<i64, Vec<Value>> = live
            .iter()
            .find(|d| d.name == ptable.name)
            .map(|d| d.rows.iter().map(|r| (r.rowid, r.values.clone())).collect())
            .unwrap_or_default();

        for (rowid, values, leaf_page) in prior_rows {
            let source = RecoverySource::RollbackJournal(RollbackJournalSource {
                segment: 0,
                pgno: leaf_page,
                header_state,
                checksum_valid,
            });
            match current.get(&rowid) {
                None => deleted.push(PriorRow {
                    table: ptable.name.clone(),
                    rowid,
                    values,
                    source,
                }),
                Some(cur) if *cur != values => modified.push(PriorVersionRecord {
                    table: ptable.name.clone(),
                    rowid,
                    prior_values: values,
                    current_values: cur.clone(),
                    replaced_rowid: true,
                    source,
                }),
                Some(_) => {} // unchanged: the page was journaled for another row.
            }
        }
    }

    let counts = JournalCounts {
        deleted: deleted.len(),
        modified: modified.len(),
    };
    JournalRecovery {
        deleted,
        modified,
        counts,
    }
}

/// Determine whether the journal header was intact (Tier A) or zeroed (Tier B),
/// by re-parsing it against the db's authoritative page size.
fn prior_header_state(journal: &[u8], db: &Database) -> Option<JournalHeaderState> {
    let parsed = RollbackJournal::parse(journal, db.header().page_size).ok()?;
    Some(match parsed.header() {
        JournalHeader::Valid { .. } => JournalHeaderState::Valid,
        JournalHeader::ReconstructedZeroed { .. } => JournalHeaderState::ReconstructedZeroed,
    })
}

/// How confidently a carved deleted row has been reattached to a live table.
///
/// The three tiers encode distinct epistemic strengths (observed fact / forensic
/// inference / unattributed), and the renderer keeps them honest: a Tier-2 guess
/// is "consistent with" a table, never asserted as the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attribution {
    /// **Tier 1 — CERTAIN.** The record was carved from a page still part of a
    /// live table's b-tree, so the owning table is known for sure. Carries that
    /// table's name.
    Known(String),
    /// **Tier 2 — INFERRED.** The whole page was freed (linkage cut); the
    /// record's *shape* (column count + per-column affinity) matched one or more
    /// surviving tables. A forensic inference, never asserted as fact.
    Inferred {
        /// The candidate table whose shape the record matches.
        guess: String,
        /// `true` when more than one surviving table matched the shape, so the
        /// guess is one of several equally-consistent candidates.
        ambiguous: bool,
    },
    /// **Tier 3 — UNATTRIBUTED.** Dropped-table residue, or a shape matching no
    /// surviving table.
    Unattributed,
}

/// Whether a carved value is **storage-compatible** with a column of the given
/// declared affinity — the per-cell test the shape matcher applies.
///
/// Rules (permissive enough to survive `SQLite`'s numeric coercions, strict
/// enough to discriminate tables of equal arity but different type layout):
/// a NULL is compatible with anything; INTEGER/REAL are mutually compatible and
/// also satisfy NUMERIC; TEXT requires a text-leaning affinity (TEXT) or the
/// catch-all BLOB/NUMERIC; a BLOB requires BLOB. The BLOB affinity (or a column
/// with no declared type) accepts any value, matching SQLite's "no datatype"
/// semantics.
#[must_use]
pub fn value_fits_affinity(value: &Value, affinity: crate::attribution::Affinity) -> bool {
    use crate::attribution::Affinity;
    // BLOB affinity (incl. no-declared-type columns) is SQLite's catch-all: a
    // value of any storage class can live there.
    if affinity == Affinity::Blob {
        return true;
    }
    match value {
        // A NULL fits any column.
        Value::Null => true,
        Value::Integer(_) | Value::Real(_) => matches!(
            affinity,
            Affinity::Integer | Affinity::Real | Affinity::Numeric
        ),
        // NUMERIC accepts text too (SQLite stores a non-numeric string as text in
        // a NUMERIC column), so a TEXT value is consistent with TEXT or NUMERIC.
        Value::Text(_) => matches!(affinity, Affinity::Text | Affinity::Numeric),
        // A BLOB only sits in a BLOB column (handled above) — never a typed one.
        Value::Blob(_) => false,
    }
}

/// Whether a carved record's values match a live table's shape: equal column
/// count AND every value storage-compatible with the corresponding column
/// affinity ([`value_fits_affinity`]).
#[must_use]
pub fn shape_matches(values: &[Value], table: &crate::attribution::LiveTable) -> bool {
    if values.len() != table.affinities.len() {
        return false;
    }
    values
        .iter()
        .zip(&table.affinities)
        .all(|(v, &aff)| value_fits_affinity(v, aff))
}

/// The names of every live table whose shape a record's values match, in the
/// order the tables were listed. Zero matches → unattributable shape; one →
/// a clean guess; more than one → ambiguous.
#[must_use]
pub fn matching_tables(values: &[Value], tables: &[crate::attribution::LiveTable]) -> Vec<String> {
    tables
        .iter()
        .filter(|t| shape_matches(values, t))
        .map(|t| t.name.clone())
        .collect()
}

/// Attribute one carved record to a tier given the live tables and the
/// page→table map. Pure (no DB access) so it is directly unit-testable.
///
/// - Tier 1 (CERTAIN): `source` is an in-page class ([`RecoverySource::InPageFreeBlock`]
///   or [`RecoverySource::FreeblockReconstructed`]) AND the record's page is in
///   `page_table_map` → [`Attribution::Known`].
/// - Tier 2 (INFERRED): `source` is [`RecoverySource::FreelistPage`] → match the
///   record's shape against `tables`; exactly one match → a guess, more than one
///   → ambiguous, none → [`Attribution::Unattributed`].
/// - Tier 3 (UNATTRIBUTED): [`RecoverySource::DroppedTable`], any other source,
///   or a shape matching no surviving table.
#[must_use]
pub fn attribute_record(
    rec: &CarvedRecord,
    tables: &[crate::attribution::LiveTable],
    page_table_map: &std::collections::BTreeMap<u32, String>,
) -> Attribution {
    // Tier 1 — CERTAIN: an in-page record on a page still owned by a live
    // table's b-tree. The page→table map is the hard linkage.
    if matches!(
        rec.source,
        RecoverySource::InPageFreeBlock | RecoverySource::FreeblockReconstructed
    ) {
        if let Some(name) = page_table_map.get(&rec.page) {
            return Attribution::Known(name.clone());
        }
        // In-page residue on a page that is NOT a live-table page (e.g. the
        // owning table itself was freed): no certain linkage, and in-page is not
        // the freelist class, so it is unattributed rather than inferred.
        return Attribution::Unattributed;
    }

    // Tier 2 — INFERRED: a whole-page freelist record; the linkage is cut, so
    // attribute by shape against the surviving tables.
    if rec.source == RecoverySource::FreelistPage {
        let mut matches = matching_tables(&rec.values, tables);
        return match matches.len() {
            0 => Attribution::Unattributed,
            1 => Attribution::Inferred {
                guess: matches.remove(0),
                ambiguous: false,
            },
            _ => Attribution::Inferred {
                guess: matches.remove(0),
                ambiguous: true,
            },
        };
    }

    // Tier 3 — UNATTRIBUTED: dropped-table residue or any other source.
    Attribution::Unattributed
}

/// Attribute every carved record, reading the live tables and page→table map
/// from `db` once. The returned vector is parallel to `records`.
#[must_use]
pub fn attribute_records(db: &Database, records: &[CarvedRecord]) -> Vec<Attribution> {
    let tables = db.live_tables();
    let page_table_map = db.page_to_table_map();
    records
        .iter()
        .map(|rec| attribute_record(rec, &tables, &page_table_map))
        .collect()
}
