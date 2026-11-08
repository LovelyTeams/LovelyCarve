//! Shared helpers for serving filesystem browsing commands from volume index
//! files: the active case's published indexes, or the caseless session
//! indexes. Both are standalone read-only SQLite files (`core::catalog::filedb::filedb_file`).

use std::collections::HashMap;
use std::path::PathBuf;

use crate::core::casework::case::CaseState;
use crate::core::catalog::filedb::filedb_query;
use crate::core::catalog::snapshot::volume_snapshot::volume_size_for_key;
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::tree::fs_tree::FsNode;
use crate::state::AppState;

/// One volume index a browsing query can be served from.
#[derive(Clone, Debug)]
pub struct SnapshotIndexTarget {
    pub index: PathBuf,
    pub volume_key: String,
    pub vol_id: i64,
}

/// Which published volumes may serve a query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexCoverage {
    /// Any indexed volume, including walks cut short by the entry cap.
    Any,
    /// Only complete walks (全显 must not silently miss files).
    Complete,
}

/// The active case's index targets for `image_id`, or `None` without a case.
pub(crate) async fn case_index_targets_of(
    state: &AppState,
    image: &EvidenceSession,
    image_id: &str,
    coverage: IndexCoverage,
) -> Option<Vec<SnapshotIndexTarget>> {
    let active = state.active_case.lock().await;
    let case = active.as_ref()?;
    Some(case_index_targets(case, image, image_id, coverage))
}

/// Published volume indexes of `image_id` whose source volume still matches
/// the loaded image.
pub(crate) fn case_index_targets(
    case: &CaseState,
    image: &EvidenceSession,
    image_id: &str,
    coverage: IndexCoverage,
) -> Vec<SnapshotIndexTarget> {
    case.volumes()
        .iter()
        .filter(|record| {
            record.image_id == image_id
                && (coverage == IndexCoverage::Any || !record.truncated)
                && if let Some(roots) = image.indexed_roots() {
                    record.index_is_current()
                        && roots.iter().any(|root| root.id == record.volume_key)
                } else {
                    record.source_size == volume_size_for_key(image, &record.volume_key)
                }
        })
        .filter_map(|record| {
            Some(SnapshotIndexTarget {
                index: case.index_path(record)?,
                volume_key: record.volume_key.clone(),
                vol_id: record.vol_id,
            })
        })
        .collect()
}

/// Caseless session index targets of `image_id`; empty while a case is open.
pub(crate) async fn session_index_targets(
    state: &AppState,
    image: &EvidenceSession,
    image_id: &str,
    coverage: IndexCoverage,
) -> Vec<SnapshotIndexTarget> {
    if state.active_case.lock().await.is_some() {
        return Vec::new();
    }
    let sessions = state.session_snapshots.lock().await;
    sessions
        .iter()
        .filter_map(|((session_image_id, _), snapshot)| {
            if session_image_id != image_id
                || snapshot.source_volume_size != volume_size_for_key(image, &snapshot.volume_key)
                || (coverage == IndexCoverage::Complete && snapshot.truncated)
            {
                return None;
            }
            snapshot
                .index
                .as_ref()
                .filter(|index| index.path.is_file())
                .map(|index| SnapshotIndexTarget {
                    index: index.path.clone(),
                    volume_key: snapshot.volume_key.clone(),
                    vol_id: index.vol_id,
                })
        })
        .collect()
}

/// The case targets when a case is open, otherwise the session targets.
pub async fn index_targets(
    state: &AppState,
    image: &EvidenceSession,
    image_id: &str,
    coverage: IndexCoverage,
) -> Vec<SnapshotIndexTarget> {
    match case_index_targets_of(state, image, image_id, coverage).await {
        Some(targets) => targets,
        None => session_index_targets(state, image, image_id, coverage).await,
    }
}

pub(crate) fn query_snapshot_index_children(
    targets: &[SnapshotIndexTarget],
    parent_id: &str,
) -> anyhow::Result<Option<Vec<FsNode>>> {
    for target in targets {
        if let Some(nodes) = filedb_query::children(&target.index, parent_id)? {
            return Ok(Some(nodes));
        }
    }
    Ok(None)
}

pub(crate) fn query_snapshot_index_child_counts(
    targets: &[SnapshotIndexTarget],
    parent_ids: &[String],
) -> anyhow::Result<HashMap<String, u64>> {
    let mut counts = HashMap::with_capacity(parent_ids.len());
    for target in targets {
        let indexed = filedb_query::child_counts(&target.index, parent_ids)?;
        for parent_id in parent_ids {
            if counts.contains_key(parent_id) {
                continue;
            }
            let lookup_key = filedb_query::parent_key(parent_id);
            if let Some(count) = indexed.get(lookup_key) {
                counts.insert(parent_id.clone(), count.count);
            }
        }
    }
    Ok(counts)
}

pub(crate) fn query_snapshot_index_subtree_empties(
    targets: &[SnapshotIndexTarget],
    parent_ids: &[String],
) -> anyhow::Result<HashMap<String, bool>> {
    let mut empties = HashMap::with_capacity(parent_ids.len());
    for parent_id in parent_ids {
        for target in targets {
            if let Some(has_file) = filedb_query::subtree_has_file(&target.index, parent_id)? {
                empties.insert(parent_id.clone(), !has_file);
                break;
            }
        }
    }
    Ok(empties)
}
