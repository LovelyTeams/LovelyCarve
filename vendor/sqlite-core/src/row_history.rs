//! Per-table, per-rowid VERSION HISTORY over a `SQLite` database's WAL temporal
//! model — the evidence-based "row history" (Phase 1 of the WAL-temporal work).
//!
//! [`Database::row_histories`](crate::Database::row_histories) walks each salt
//! epoch's [`CommitSnapshot`](crate::CommitSnapshot)s in commit order, then the
//! final live view, and for every rowid emits the sequence of distinct record
//! values it held — an `INSERT`/`UPDATE`/`DELETE`/reinsert history reconstructed
//! purely from the bytes, with NO timestamps (SQLite WAL carries no wall-clock
//! time; `commit_seq` is LOGICAL order within an epoch only).
//!
//! The two correctness-critical transforms are pure and unit-testable here:
//! [`build_rowid_versions`] collapses identical consecutive values into one
//! version (labelled by the EARLIEST view it appeared in) and classifies each
//! version's [`ViewState`] from evidence; the same walk detects rowid REUSE (a
//! present→absent→present-with-a-different-record gap is a delete+reinsert, two
//! entities, never one continuous update).

use crate::Value;
use std::collections::BTreeMap;

/// Where a [`RowVersion`] came from. Logical provenance only — never a timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionOrigin {
    /// The current on-disk ⊕ WAL live view (the FINAL state).
    Live,
    /// A materializable WAL commit, addressed by its [`CommitId`](crate::CommitId).
    Commit(crate::CommitId),
    /// Free-space carved residue with no precise temporal position (freeblocks
    /// persist across commits, so the carve is ORDER-UNKNOWN).
    CarvedResidue,
}

/// The EVIDENCE-based state of a row version relative to the FINAL live view.
///
/// Strictly evidence, never intent: it records what the bytes show (present /
/// changed-later / absent / carved), not what the user meant to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewState {
    /// This exact value is present and unchanged in the final live view — the
    /// current row.
    PresentInFinalView,
    /// An earlier value that a later view replaced with a DIFFERENT value for the
    /// same rowid.
    ValueChangedLater,
    /// The rowid's last value before it disappeared: absent from the final live
    /// view (a deletion).
    AbsentInFinalView,
    /// A free-space carve — order-unknown residue, not a positioned commit value.
    CarvedResidue,
}

/// One version of one row: a distinct record value the rowid held at some point,
/// with its evidence-based classification and logical provenance.
// The boolean flags are independent EVIDENCE bits (deleted / guessed / reused /
// uncertain), each separately consumed by a renderer — not a hidden state machine
// that a two-variant enum would clarify, so the excessive-bools lint is waived.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq)]
pub struct RowVersion {
    /// The rowid this version belongs to. `None` only for a carved residue whose
    /// rowid was destroyed.
    pub rowid: Option<i64>,
    /// The decoded column values of this version, in column order.
    pub values: Vec<Value>,
    /// Logical provenance of the version.
    pub origin: VersionOrigin,
    /// The LOGICAL commit sequence (per salt epoch, monotonic WITHIN an epoch
    /// only) of the EARLIEST view this value appeared in. `None` for the live
    /// view and for carved residue (order-unknown).
    pub commit_seq: Option<u32>,
    /// Evidence-based view state.
    pub view_state: ViewState,
    /// Whether this version's rowid is absent from the final live view (a
    /// deletion or a completed pre-reuse entity).
    pub is_deleted: bool,
    /// Whether this version was reconstructed/guessed rather than read directly
    /// (carved residue with inferred attribution).
    pub is_guessed: bool,
    /// Whether this rowid was REUSED — present, then absent for ≥1 view, then
    /// present again with a DIFFERENT record. Set on every version of a reused
    /// rowid so a renderer never presents two entities as one continuous update.
    pub rowid_reused: bool,
    /// Whether this version's attribution is uncertain — its source view was
    /// checksum-invalid residue, or its schema could not be reconstructed.
    pub attribution_uncertain: bool,
    /// Whether this value went ABSENT from ≥1 intermediate view and then
    /// reappeared with the IDENTICAL record. Such a run collapses (it is not
    /// rowid reuse — the same record by evidence), but the absence itself is
    /// forensic evidence: a delete-then-reinsert of the same value (e.g. a message
    /// deleted and re-sent). A LOW-CONFIDENCE signal — a transient absence in the
    /// materialized commit views is indistinguishable from a genuine round-trip,
    /// so it records what the bytes show, never intent (§4.2).
    pub reinserted_after_gap: bool,
}

/// The version history of one user table.
#[derive(Debug, Clone, PartialEq)]
pub struct TableHistory {
    /// The table's `sqlite_master.name`.
    pub table: String,
    /// The table's column names (real names where the schema parsed, else generic).
    pub columns: Vec<String>,
    /// Whether this is a `WITHOUT ROWID` table — such tables have no rowid and are
    /// not version-tracked here; `versions` is then empty (presence recorded only),
    /// and the live rows are carried in [`without_rowid_rows`](Self::without_rowid_rows).
    pub without_rowid: bool,
    /// Every row version, sorted by `rowid` (`None` last), then `commit_seq`
    /// ascending, with carved-residue (order-unknown) versions grouped after the
    /// ordered versions of their rowid.
    pub versions: Vec<RowVersion>,
    /// For a `WITHOUT ROWID` table (§1.4): its live rows read from the index
    /// b-tree, in the table's column order. Empty for an ordinary table and for a
    /// `WITHOUT ROWID` table whose rows could not be read. These have no version
    /// history (the model is rowid-keyed); they are the present live state.
    pub without_rowid_rows: Vec<Vec<Value>>,
}

/// One chronological VIEW of a single table: a rowid→values map plus the view's
/// logical position and trust flags. The ordered list of these (commit views in
/// epoch order, then the final live view last) is the input to
/// [`build_rowid_versions`].
// `is_final` / `checksum_valid` / `schema_known` are independent per-view trust
// bits, not a state machine; the excessive-bools lint is waived for clarity.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq)]
pub struct RowView {
    /// LOGICAL commit sequence within the salt epoch (monotonic within an epoch
    /// only). `None` for the final live view.
    pub commit_seq: Option<u32>,
    /// Whether this is the final live view (the current state).
    pub is_final: bool,
    /// Whether the source commit's checksum chain validated. `false` = residue:
    /// rows first attributed here are flagged `attribution_uncertain`.
    pub checksum_valid: bool,
    /// Whether the table's schema could be reconstructed for this view. `false`
    /// flags rows attributed here `attribution_uncertain`.
    pub schema_known: bool,
    /// This view's logical provenance — [`VersionOrigin::Live`] for the final
    /// view, [`VersionOrigin::Commit`] (carrying the real [`CommitId`](crate::CommitId))
    /// for a WAL commit view. Copied verbatim onto every version first seen here.
    pub origin: VersionOrigin,
    /// This view's rows: rowid → decoded values. A rowid absent from the map is
    /// absent in this view.
    pub rows: BTreeMap<i64, Vec<Value>>,
}

/// Build the ordered [`RowVersion`]s for ONE `rowid` by walking `views` (already
/// in chronological order: epoch commits ascending, then the final live view).
///
/// Emits a version only when the value CHANGES (insert / update / delete /
/// reinsert), collapsing identical consecutive values into one version labelled
/// by the EARLIEST view it appeared in. View-state is evidence-based:
/// `PresentInFinalView` when the value is the final live value; `ValueChangedLater`
/// when a later view replaced it with a different value; `AbsentInFinalView` when
/// the rowid's last value disappeared. Rowid REUSE (present → absent → present
/// with a different record) flags every version of the rowid `rowid_reused` and
/// treats the post-gap value as a fresh insert.
///
/// Pure: no I/O. The result is in chronological order for this rowid.
#[must_use]
pub fn build_rowid_versions(rowid: i64, views: &[RowView]) -> Vec<RowVersion> {
    // ---- 1. Collapse the per-view presence sequence into maximal runs --------
    // A "present run" is a maximal stretch of views holding the SAME value with
    // no intervening absence; an absence (the rowid missing from a view's map)
    // breaks a run even when the value later reappears identically. We record
    // each present run's value, the EARLIEST view it appeared in (the label),
    // that view's trust flags, and whether an absence GAP preceded it.
    struct Run<'a> {
        values: &'a [Value],
        earliest_seq: Option<u32>,
        origin: VersionOrigin,
        uncertain: bool,
        gap_before: bool,
        // The value went absent then reappeared IDENTICAL within this run (§4.2):
        // collapsed as one record, but the absence is recorded as evidence.
        reappeared_after_gap: bool,
    }
    let mut runs: Vec<Run> = Vec::new();
    let mut seen_present = false;
    let mut pending_gap = false;
    for view in views {
        match view.rows.get(&rowid) {
            None => {
                // An absence only counts as a gap once the rowid has appeared.
                if seen_present {
                    pending_gap = true;
                }
            }
            Some(values) => {
                let uncertain = !view.checksum_valid || !view.schema_known;
                let extends = matches!(runs.last(), Some(r) if r.values == values.as_slice());
                if extends && !pending_gap {
                    // Same value, no gap: extend the current run (collapse).
                } else if extends && pending_gap {
                    // Same value across a gap: still the same record by evidence —
                    // collapse into the existing run (not a reuse), clearing the
                    // gap so it is not treated as a delete+reinsert. Record the
                    // absence as a low-confidence evidence bit (§4.2) rather than
                    // discarding it silently.
                    if let Some(r) = runs.last_mut() {
                        r.reappeared_after_gap = true;
                    }
                } else {
                    runs.push(Run {
                        values,
                        earliest_seq: view.commit_seq,
                        origin: view.origin.clone(),
                        uncertain,
                        gap_before: pending_gap,
                        reappeared_after_gap: false,
                    });
                }
                seen_present = true;
                pending_gap = false;
            }
        }
    }

    if runs.is_empty() {
        return Vec::new();
    }

    // ---- 2. Final-view facts -------------------------------------------------
    // The rowid's value in the final live view (None if absent there). Whether
    // the LAST run reaches the final view (no trailing gap after it).
    let final_values: Option<&[Value]> = views
        .iter()
        .rev()
        .find(|v| v.is_final)
        .and_then(|v| v.rows.get(&rowid))
        .map(Vec::as_slice);
    let present_in_final = final_values.is_some();

    // ---- 3. Reuse detection --------------------------------------------------
    // Reuse = a present run, a gap, then a DIFFERENT value. `gap_before` on any
    // run past the first is exactly that signal (a same-value-across-gap run was
    // collapsed in step 1, so it never carries gap_before here).
    let rowid_reused = runs.iter().skip(1).any(|r| r.gap_before);

    // ---- 4. Emit one RowVersion per run --------------------------------------
    let last_idx = runs.len() - 1;
    // A run is "ended by deletion" iff the rowid DISAPPEARED right after it — the
    // NEXT run begins after a gap (reuse), or it is the last run and the rowid is
    // absent in the final view. A run replaced by a different value with NO gap is
    // a direct UPDATE (ValueChangedLater), not a deletion.
    let next_gap: Vec<bool> = (0..runs.len())
        .map(|i| runs.get(i + 1).is_some_and(|r| r.gap_before))
        .collect();
    runs.iter()
        .enumerate()
        .map(|(i, run)| {
            let is_last = i == last_idx;
            // The final value reaches the last run iff that run is present in the
            // final view AND its value equals the final value.
            let is_final_value = is_last && present_in_final && final_values == Some(run.values);
            let deleted_here = next_gap[i] || (is_last && !present_in_final);
            let view_state = if is_final_value {
                ViewState::PresentInFinalView
            } else if deleted_here {
                // The rowid disappeared after this value — its last value before
                // the gap/end is AbsentInFinalView (a completed deletion).
                ViewState::AbsentInFinalView
            } else {
                // An earlier value later replaced by a different one (direct update).
                ViewState::ValueChangedLater
            };
            let is_deleted = matches!(view_state, ViewState::AbsentInFinalView);
            RowVersion {
                rowid: Some(rowid),
                values: run.values.to_vec(),
                // The version's origin is the EARLIEST view it appeared in — the
                // same view that supplies its `commit_seq` label.
                origin: run.origin.clone(),
                commit_seq: run.earliest_seq,
                view_state,
                is_deleted,
                is_guessed: false,
                rowid_reused,
                attribution_uncertain: run.uncertain,
                reinserted_after_gap: run.reappeared_after_gap,
            }
        })
        .collect()
}

/// Assemble the [`TableHistory`] for ONE table from its chronological `views`
/// (commit views in epoch order, then the final live view last) and its schema.
///
/// `WITHOUT ROWID` tables have no rowid to key a history on, so they are recorded
/// with `without_rowid = true` and NO versions (a renderer annotates their
/// presence). For an ordinary table, every rowid that appears in any view is run
/// through [`build_rowid_versions`]; the resulting versions are sorted by `rowid`
/// then `commit_seq` ascending (a `None` `commit_seq` — the live or order-unknown
/// value — sorts LAST within a rowid).
///
/// Pure: no I/O.
#[must_use]
pub fn table_history(
    table: String,
    columns: Vec<String>,
    without_rowid: bool,
    views: &[RowView],
) -> TableHistory {
    if without_rowid {
        // No rowid → no version history. The live rows (from the index b-tree) are
        // filled in by the caller (see `Database::row_histories`), which has the
        // reader; the pure builder leaves them empty.
        return TableHistory {
            table,
            columns,
            without_rowid: true,
            versions: Vec::new(),
            without_rowid_rows: Vec::new(),
        };
    }
    // Every rowid seen across all views, ascending.
    let mut rowids: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();
    for v in views {
        rowids.extend(v.rows.keys().copied());
    }
    let mut versions: Vec<RowVersion> = Vec::new();
    for rowid in rowids {
        versions.extend(build_rowid_versions(rowid, views));
    }
    sort_versions(&mut versions);
    TableHistory {
        table,
        columns,
        without_rowid: false,
        versions,
        without_rowid_rows: Vec::new(),
    }
}

/// Re-sort a table's versions into the canonical order after a caller has
/// appended more (e.g. the forensic layer merging carved residue): by `rowid`
/// (`None` last), then `commit_seq` ascending with `None` (live / order-unknown
/// residue) grouped LAST within the rowid. Stable.
pub fn sort_table_versions(versions: &mut [RowVersion]) {
    sort_versions(versions);
}

/// Sort versions by `rowid` (`None` last), then `commit_seq` ascending with a
/// `None` `commit_seq` (live / order-unknown carved residue) grouped LAST within
/// the rowid. Stable, so equal keys keep chronological insertion order.
fn sort_versions(versions: &mut [RowVersion]) {
    versions.sort_by(|a, b| {
        // rowid: Some(_) before None; within Some, ascending by id.
        let ra = a.rowid;
        let rb = b.rowid;
        let rowid_ord = match (ra, rb) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        };
        rowid_ord.then_with(|| {
            // commit_seq: Some(_) (positioned) before None (live/residue), then
            // ascending within Some.
            match (a.commit_seq, b.commit_seq) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        })
    });
}
