//! Persisted file-tag IPC: load / save an evidence's tags in the active case
//! ledger (`core::casework::tag_store`). Ordinary edits are node-level; full replacement
//! stays as a compatibility path. Without an open case (or for an image that is
//! not evidence of it) commands degrade gracefully — load returns empty, writes
//! report `false` — and tags live only in the frontend's localStorage copy.

use std::collections::{HashMap, HashSet};

use crate::host::State;
use anyhow::Context as _;

use crate::core::casework::tag_store;
use crate::state::AppState;
use {
    crate::app::error::AppError, crate::commands::bounded_unique_node_ids,
    crate::commands::MAX_DERIVED_NODE_QUERY,
};

/// The persisted tag map for an image, keyed by node id. Empty when no case
/// is open, nothing was tagged yet, or the store is unreadable — browsing
/// must not break over a re-creatable bookmark list.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_tags_load(
    state: State<'_, AppState>,
    image_id: String,
) -> Result<HashMap<String, String>, AppError> {
    let case_dir = state
        .active_case
        .lock()
        .await
        .as_ref()
        .map(|case| case.dir.clone());
    let Some(case_dir) = case_dir else {
        return Ok(HashMap::new());
    };
    let loaded = tokio::task::spawn_blocking({
        let image_id = image_id.clone();
        move || tag_store::load(&case_dir, &image_id)
    })
    .await
    .map_err(|error| AppError::task_panicked("load tags", error))?;
    match loaded {
        Ok(entries) => Ok(entries),
        Err(err) => {
            tracing::warn!("tag store: load image={image_id} failed: {err}");
            Ok(HashMap::new())
        }
    }
}

/// Loads persisted tags only for the requested node ids. The existing
/// whole-image load remains available for full replacement semantics; this
/// endpoint is the scalable read side for current-directory or page hydration.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_tags_load_nodes(
    state: State<'_, AppState>,
    image_id: String,
    node_ids: Vec<String>,
) -> Result<HashMap<String, String>, AppError> {
    let node_ids = bounded_unique_node_ids(node_ids)?;
    if node_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let case_dir = state
        .active_case
        .lock()
        .await
        .as_ref()
        .map(|case| case.dir.clone());
    let Some(case_dir) = case_dir else {
        return Ok(HashMap::new());
    };
    let loaded = tokio::task::spawn_blocking({
        let image_id = image_id.clone();
        move || tag_store::load_nodes(&case_dir, &image_id, &node_ids)
    })
    .await
    .map_err(|error| AppError::task_panicked("load tags", error))?;
    match loaded {
        Ok(entries) => Ok(entries),
        Err(err) => {
            tracing::warn!("tag store: load nodes image={image_id} failed: {err}");
            Ok(HashMap::new())
        }
    }
}

/// Applies node-level tag changes without replacing the whole image map.
/// Upserts win over deletes if the same node id is present in both lists.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_tags_apply_changes(
    state: State<'_, AppState>,
    image_id: String,
    upserts: HashMap<String, String>,
    delete_node_ids: Vec<String>,
) -> Result<bool, AppError> {
    let mut delete_node_ids = bounded_unique_node_ids(delete_node_ids)?;
    let total_requested = upserts.len() + delete_node_ids.len();
    if total_requested > MAX_DERIVED_NODE_QUERY {
        return Err(AppError::resource_limit(format!(
            "单次最多写入 {MAX_DERIVED_NODE_QUERY} 个节点标签变更"
        )));
    }
    if upserts.is_empty() && delete_node_ids.is_empty() {
        return Ok(false);
    }

    let upsert_node_ids = upserts.keys().map(String::as_str).collect::<HashSet<_>>();
    delete_node_ids.retain(|node_id| !upsert_node_ids.contains(node_id.as_str()));

    let changed = upserts.len() + delete_node_ids.len();
    if changed == 0 {
        return Ok(false);
    }

    let case_dir = state
        .active_case
        .lock()
        .await
        .as_ref()
        .map(|case| case.dir.clone());
    let Some(case_dir) = case_dir else {
        return Ok(false);
    };
    let persist_image_id = image_id.clone();
    let total = tokio::task::spawn_blocking(move || -> Result<Option<usize>, AppError> {
        tag_store::apply_changes(&case_dir, &persist_image_id, upserts, delete_node_ids)
            .context("persist tag changes")
            .map_err(AppError::from_anyhow)
    })
    .await
    .map_err(|error| AppError::task_panicked("persist tag changes", error))??;
    let Some(total) = total else {
        return Ok(false);
    };
    tracing::info!("tag store: applied image={image_id} changed={changed} total={total}");
    Ok(true)
}

/// Replaces the evidence's tags with the frontend's complete map (an empty map
/// clears them — unlike digests, removal must stick). Returns `false` when no
/// case is open or the image is not evidence of it; errors only on a failed
/// write to the case ledger.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_tags_save(
    state: State<'_, AppState>,
    image_id: String,
    entries: HashMap<String, String>,
) -> Result<bool, AppError> {
    let count = entries.len();
    let case_dir = state
        .active_case
        .lock()
        .await
        .as_ref()
        .map(|case| case.dir.clone());
    let Some(case_dir) = case_dir else {
        return Ok(false);
    };
    let persist_image_id = image_id.clone();
    let saved = tokio::task::spawn_blocking(move || -> Result<Option<usize>, AppError> {
        tag_store::save(&case_dir, &persist_image_id, entries)
            .context("persist tags")
            .map_err(AppError::from_anyhow)
    })
    .await
    .map_err(|error| AppError::task_panicked("persist tags", error))??;
    if saved.is_none() {
        return Ok(false);
    }
    tracing::info!("tag store: saved image={image_id} entries={count}");
    Ok(true)
}
