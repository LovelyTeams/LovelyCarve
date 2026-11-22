//! Filtered directory pages, using the volume indexes before the snapshot/live
//! fallback, and the snapshot-backed "is this subtree empty?" lookup that the
//! "hide empty directories" filter shares with `fs_subtree_empty_dirs`.
use std::collections::{HashMap, HashSet};

use crate::host::State;
use crate::{
    core::{
        catalog::directory_query::{self, DirectoryQuery},
        catalog::filedb::filedb_query::NodePage,
        common::ids::{ImageId, NodeId},
    },
    state::AppState,
};
use {
    crate::app::access::loaded_image_app_error,
    crate::app::error::AppError,
    crate::commands::filesystem::filesystem_snapshot_index::{
        index_targets, query_snapshot_index_subtree_empties, IndexCoverage,
    },
};

pub(crate) async fn page(
    state: State<'_, AppState>,
    image_id: ImageId,
    parent_id: NodeId,
    query: DirectoryQuery,
    cursor: Option<String>,
    page_size: Option<usize>,
) -> Result<NodePage, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let query = DirectoryQuery {
        filter: query.filter.normalized(),
        ..query
    };
    let limit =
        crate::commands::filesystem::filesystem_page::normalize_fs_node_page_size(page_size);
    let targets = index_targets(&state, &image, &image_id, IndexCoverage::Any).await;
    let indexed = tokio::task::spawn_blocking({
        let parent_id = parent_id.clone();
        let query = query.clone();
        let cursor = cursor.clone();
        move || -> anyhow::Result<Option<NodePage>> {
            for target in targets {
                if let Some(page) = directory_query::indexed_page(
                    &target.index,
                    &parent_id,
                    &query,
                    cursor.as_deref(),
                    limit,
                )? {
                    return Ok(Some(page));
                }
            }
            Ok(None)
        }
    })
    .await
    .map_err(|error| AppError::task_panicked("directory_page", error))?
    .map_err(AppError::from)?;
    if let Some(page) = indexed {
        return Ok(page);
    }
    let nodes = crate::commands::filesystem::filesystem::fs_children(
        state,
        image_id.clone(),
        parent_id.clone(),
    )
    .await?;
    let hidden_dirs: HashSet<String> = if query.hides_empty_dirs_under(&parent_id) {
        let candidates = directory_query::empty_dir_candidates(&nodes);
        subtree_empty_dirs(&state, &image_id, candidates)
            .await?
            .into_iter()
            .filter_map(|(id, empty)| empty.then_some(id))
            .collect()
    } else {
        HashSet::new()
    };
    tokio::task::spawn_blocking(move || {
        directory_query::memory_page(nodes, &query, &hidden_dirs, cursor.as_deref(), limit)
    })
    .await
    .map_err(|error| AppError::task_panicked("directory_page", error))?
    .map_err(AppError::from)
}

/// Whether each directory's subtree holds no file, from the active case's
/// in-memory snapshot bodies, then the volume indexes, then (without a case)
/// the session snapshot bodies. Directories no source can judge are omitted.
/// Reads snapshots and indexes only, never the filesystem.
pub(crate) async fn subtree_empty_dirs(
    state: &AppState,
    image_id: &ImageId,
    parent_ids: Vec<String>,
) -> Result<HashMap<String, bool>, AppError> {
    let image = {
        let images = state.images.lock().await;
        images.get(image_id).cloned()
    };
    let mut empties = HashMap::with_capacity(parent_ids.len());
    // In-memory case bodies first (volumes whose index could not be built).
    let mut missing_parent_ids = Vec::with_capacity(parent_ids.len());
    let has_case = {
        let active = state.active_case.lock().await;
        match active.as_ref() {
            Some(case) => {
                for parent_id in parent_ids {
                    match case.snapshot_subtree_empty(image_id, &parent_id) {
                        Some(empty) => {
                            empties.insert(parent_id, empty);
                        }
                        None => missing_parent_ids.push(parent_id),
                    }
                }
                true
            }
            None => {
                missing_parent_ids = parent_ids;
                false
            }
        }
    };
    // Then the volume indexes (case or caseless session).
    if let Some(image) = image.as_deref() {
        let targets = index_targets(state, image, image_id, IndexCoverage::Any).await;
        if !targets.is_empty() && !missing_parent_ids.is_empty() {
            let parent_ids_for_index = missing_parent_ids.clone();
            let indexed_empties = tokio::task::spawn_blocking(move || {
                query_snapshot_index_subtree_empties(&targets, &parent_ids_for_index)
            })
            .await
            .map_err(|err| AppError::task_panicked("fs_subtree_empty_dirs index", err))?;
            match indexed_empties {
                Ok(indexed_empties) => {
                    empties.extend(indexed_empties);
                    missing_parent_ids.retain(|parent_id| !empties.contains_key(parent_id));
                }
                Err(err) => {
                    tracing::warn!("fs_subtree_empty_dirs index err image={image_id}: {err}");
                }
            }
        }
    }
    // Caseless in-memory session bodies last.
    if !has_case {
        let sessions = state.session_snapshots.lock().await;
        for parent_id in missing_parent_ids {
            if let Some(empty) =
                crate::app::workflows::snapshot::snapshot_session::session_subtree_empty(
                    &sessions, image_id, &parent_id,
                )
            {
                empties.insert(parent_id, empty);
            }
        }
    }
    Ok(empties)
}
