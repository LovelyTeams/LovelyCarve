//! Persisted file-type-identification IPC: load / merge-save an evidence's
//! Magika identifications in its `facts.db` (`core::casework::facts_store`). Without an
//! open case (or for an image that is not evidence of it) both commands degrade
//! gracefully — load returns empty, persist reports `false` — and
//! identifications stay session-only.

use std::collections::HashMap;

use crate::host::State;
use anyhow::Context as _;

use crate::core::analysis::identify::magika_ident::TypeIdentReport;
use crate::core::casework::facts_store;
use crate::state::AppState;
use {
    crate::app::access::evidence_case_dir, crate::app::error::AppError,
    crate::commands::bounded_unique_node_ids,
};

/// The persisted identification map for an image, keyed by node id. Empty when
/// no case is open, nothing was persisted yet, or the store is unreadable —
/// browsing must not break over a recomputable cache.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_ftype_load(
    state: State<'_, AppState>,
    image_id: String,
) -> Result<HashMap<String, TypeIdentReport>, AppError> {
    let Some(case_dir) = evidence_case_dir(&state, &image_id).await else {
        return Ok(HashMap::new());
    };
    let loaded = tokio::task::spawn_blocking({
        let image_id = image_id.clone();
        move || facts_store::types_load(&case_dir, &image_id)
    })
    .await
    .map_err(|error| AppError::task_panicked("load ftypes", error))?;
    match loaded {
        Ok(entries) => Ok(entries),
        Err(err) => {
            tracing::warn!("ftype store: load image={image_id} failed: {err:#}");
            Ok(HashMap::new())
        }
    }
}

/// Loads persisted type-identification reports only for the requested node
/// ids. New UI should prefer this over whole-image map hydration when entering
/// a directory or receiving a flat-list batch.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_ftype_load_nodes(
    state: State<'_, AppState>,
    image_id: String,
    node_ids: Vec<String>,
) -> Result<HashMap<String, TypeIdentReport>, AppError> {
    let node_ids = bounded_unique_node_ids(node_ids)?;
    if node_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let Some(case_dir) = evidence_case_dir(&state, &image_id).await else {
        return Ok(HashMap::new());
    };
    let loaded = tokio::task::spawn_blocking({
        let image_id = image_id.clone();
        move || facts_store::types_load_nodes(&case_dir, &image_id, &node_ids)
    })
    .await
    .map_err(|error| AppError::task_panicked("load ftypes", error))?;
    match loaded {
        Ok(entries) => Ok(entries),
        Err(err) => {
            tracing::warn!("ftype store: load nodes image={image_id} failed: {err:#}");
            Ok(HashMap::new())
        }
    }
}

/// Merges freshly identified types into the evidence's store. Returns `false`
/// when no case is open or the image is not evidence of it (results stay
/// session-only); errors only on a failed write.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_ftype_persist(
    state: State<'_, AppState>,
    image_id: String,
    entries: HashMap<String, TypeIdentReport>,
) -> Result<bool, AppError> {
    if entries.is_empty() {
        return Ok(false);
    }
    let added = entries.len();
    let Some(case_dir) = evidence_case_dir(&state, &image_id).await else {
        return Ok(false);
    };
    let persist_image_id = image_id.clone();
    let merged = tokio::task::spawn_blocking(move || -> Result<usize, AppError> {
        facts_store::types_merge(&case_dir, &persist_image_id, entries)
            .context("persist ftypes")
            .map_err(AppError::from_anyhow)
    })
    .await
    .map_err(|error| AppError::task_panicked("persist ftypes", error))??;
    tracing::info!("ftype store: persisted image={image_id} added={added} merged={merged}");
    Ok(true)
}
