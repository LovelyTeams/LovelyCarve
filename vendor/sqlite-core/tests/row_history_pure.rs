//! Pure-logic unit tests for the per-rowid version-history builder (no sqlite3).
//!
//! These pin the two correctness-critical pure transforms the WAL-temporal
//! history rests on, in isolation from any b-tree / WAL parsing:
//!   1. collapse identical consecutive values into one version, labelled by the
//!      EARLIEST view it appeared in, and classify each version's [`ViewState`]
//!      from EVIDENCE (present in final view / changed later / absent in final);
//!   2. rowid-reuse gap detection — present, then absent for ≥1 view, then
//!      present again with a DIFFERENT record is a delete+reinsert, not an update.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use sqlite_core::row_history::{build_rowid_versions, RowView, VersionOrigin, ViewState};
use sqlite_core::{CommitId, Value, WalSegmentId};

/// Shorthand: a one-column TEXT row.
fn txt(s: &str) -> Vec<Value> {
    vec![Value::Text(s.to_string())]
}

/// A committed view at logical `seq` mapping the single rowid `1` to `values`
/// (or absent when `values` is `None`).
fn commit_view(seq: u32, values: Option<Vec<Value>>) -> RowView {
    let mut rows = std::collections::BTreeMap::new();
    if let Some(v) = values {
        rows.insert(1_i64, v);
    }
    RowView {
        commit_seq: Some(seq),
        is_final: false,
        checksum_valid: true,
        schema_known: true,
        origin: VersionOrigin::Commit(CommitId {
            segment: WalSegmentId(0),
            commit_frame_index: seq as usize,
            db_size_after_commit: 1,
        }),
        rows,
    }
}

/// The final live view mapping rowid `1` to `values` (or absent).
fn final_view(values: Option<Vec<Value>>) -> RowView {
    let mut rows = std::collections::BTreeMap::new();
    if let Some(v) = values {
        rows.insert(1_i64, v);
    }
    RowView {
        commit_seq: None,
        is_final: true,
        checksum_valid: true,
        schema_known: true,
        origin: VersionOrigin::Live,
        rows,
    }
}

#[test]
fn collapses_identical_consecutive_values_to_earliest_view() {
    // 'a' @C1, 'a' @C2 (unchanged), 'a' in the final live view → ONE version,
    // labelled by the earliest view (C1), state PresentInFinalView.
    let views = vec![
        commit_view(1, Some(txt("a"))),
        commit_view(2, Some(txt("a"))),
        final_view(Some(txt("a"))),
    ];
    let versions = build_rowid_versions(1, &views);
    assert_eq!(
        versions.len(),
        1,
        "identical values collapse to one version"
    );
    assert_eq!(versions[0].values, txt("a"));
    assert_eq!(
        versions[0].commit_seq,
        Some(1),
        "labelled by the EARLIEST view it appeared in"
    );
    assert_eq!(versions[0].view_state, ViewState::PresentInFinalView);
    assert!(!versions[0].is_deleted);
}

#[test]
fn value_changed_later_then_present_in_final() {
    // 'a' @C1, then 'A' @C2 which survives to the final view.
    let views = vec![
        commit_view(1, Some(txt("a"))),
        commit_view(2, Some(txt("A"))),
        final_view(Some(txt("A"))),
    ];
    let versions = build_rowid_versions(1, &views);
    assert_eq!(versions.len(), 2, "a then A are two distinct versions");

    assert_eq!(versions[0].values, txt("a"));
    assert_eq!(versions[0].commit_seq, Some(1));
    assert_eq!(
        versions[0].view_state,
        ViewState::ValueChangedLater,
        "the earlier value was replaced by a different one"
    );
    assert!(!versions[0].is_deleted);

    assert_eq!(versions[1].values, txt("A"));
    assert_eq!(versions[1].commit_seq, Some(2));
    assert_eq!(versions[1].view_state, ViewState::PresentInFinalView);
    assert!(!versions[1].is_deleted);
}

#[test]
fn absent_in_final_view_is_a_deletion() {
    // 'b' @C1, then deleted by C2 and never reappears: last value AbsentInFinalView.
    let views = vec![
        commit_view(1, Some(txt("b"))),
        commit_view(2, None),
        final_view(None),
    ];
    let versions = build_rowid_versions(1, &views);
    assert_eq!(versions.len(), 1, "one value, then gone");
    assert_eq!(versions[0].values, txt("b"));
    assert_eq!(versions[0].commit_seq, Some(1));
    assert_eq!(versions[0].view_state, ViewState::AbsentInFinalView);
    assert!(
        versions[0].is_deleted,
        "absent in the final view => deleted"
    );
    assert!(!versions[0].rowid_reused);
}

#[test]
fn rowid_reuse_after_a_gap_is_delete_plus_reinsert() {
    // 'x' @C1, absent @C2 (deleted), then a DIFFERENT 'y' @C3 reusing the rowid.
    let views = vec![
        commit_view(1, Some(txt("x"))),
        commit_view(2, None),
        commit_view(3, Some(txt("y"))),
        final_view(Some(txt("y"))),
    ];
    let versions = build_rowid_versions(1, &views);
    assert_eq!(versions.len(), 2, "two distinct entities under one rowid");

    // The pre-gap 'x' is a completed deletion, flagged as reused.
    assert_eq!(versions[0].values, txt("x"));
    assert_eq!(versions[0].commit_seq, Some(1));
    assert_eq!(versions[0].view_state, ViewState::AbsentInFinalView);
    assert!(versions[0].is_deleted);
    assert!(
        versions[0].rowid_reused,
        "a gap followed by a different record marks the rowid reused"
    );

    // The post-gap 'y' is a fresh insert that survives to the final view.
    assert_eq!(versions[1].values, txt("y"));
    assert_eq!(versions[1].commit_seq, Some(3));
    assert_eq!(versions[1].view_state, ViewState::PresentInFinalView);
    assert!(!versions[1].is_deleted);
    assert!(
        versions[1].rowid_reused,
        "the post-gap reinsert is also flagged reused"
    );
}

#[test]
fn reappearing_with_the_same_value_after_a_gap_is_not_reuse() {
    // 'k' @C1, absent @C2, 'k' again @C3 with the SAME value. This is not a
    // different entity by evidence (same record), so it is NOT flagged reused;
    // it collapses to a single present version. (A gap with identical bytes is
    // indistinguishable from a transient absence — evidence does not support a
    // reuse claim.)
    let views = vec![
        commit_view(1, Some(txt("k"))),
        commit_view(2, None),
        commit_view(3, Some(txt("k"))),
        final_view(Some(txt("k"))),
    ];
    let versions = build_rowid_versions(1, &views);
    assert!(
        versions.iter().all(|v| !v.rowid_reused),
        "same value across the gap is not evidence of a different entity"
    );
    assert_eq!(
        versions.last().unwrap().view_state,
        ViewState::PresentInFinalView
    );
}

#[test]
fn same_value_after_a_gap_records_the_absence_as_evidence() {
    // roadmap §4.2: 'k' @C1, absent @C2, 'k' again @C3 — a delete+reinsert of an
    // IDENTICAL value. It still collapses (NOT reuse — same record by evidence),
    // but the absence itself is evidence (a message deleted and re-sent), so the
    // surviving version records `reinserted_after_gap` as a low-confidence signal.
    let views = vec![
        commit_view(1, Some(txt("k"))),
        commit_view(2, None),
        commit_view(3, Some(txt("k"))),
        final_view(Some(txt("k"))),
    ];
    let versions = build_rowid_versions(1, &views);
    assert!(
        versions.iter().all(|v| !v.rowid_reused),
        "same value across the gap is still not reuse"
    );
    assert!(
        versions.iter().any(|v| v.reinserted_after_gap),
        "the collapsed version must record that an absence gap occurred"
    );
}

#[test]
fn plain_unchanged_value_is_not_flagged_reinserted() {
    // No absence ever occurred, so reinserted_after_gap stays false.
    let views = vec![
        commit_view(1, Some(txt("a"))),
        commit_view(2, Some(txt("a"))),
        final_view(Some(txt("a"))),
    ];
    let versions = build_rowid_versions(1, &views);
    assert!(
        versions.iter().all(|v| !v.reinserted_after_gap),
        "an unbroken value must not be flagged reinserted"
    );
}

#[test]
fn residue_view_marks_attribution_uncertain() {
    // A checksum-invalid (residue) commit contributes its row but flags the
    // version attribution_uncertain.
    let mut residue = commit_view(1, Some(txt("r")));
    residue.checksum_valid = false;
    let views = vec![residue, final_view(Some(txt("r")))];
    let versions = build_rowid_versions(1, &views);
    assert!(
        versions[0].attribution_uncertain,
        "a row first seen in a residue (checksum-invalid) view is uncertain"
    );
}

#[test]
fn unknown_schema_view_marks_attribution_uncertain() {
    let mut noschema = commit_view(1, Some(txt("s")));
    noschema.schema_known = false;
    let views = vec![noschema, final_view(Some(txt("s")))];
    let versions = build_rowid_versions(1, &views);
    assert!(
        versions[0].attribution_uncertain,
        "a row attributed to a view whose schema could not be reconstructed is uncertain"
    );
}
