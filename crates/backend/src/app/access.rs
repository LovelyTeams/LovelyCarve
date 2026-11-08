//! Runtime evidence queries shared by application workflows and commands.
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::host::State;
use crate::{app::error as app_error, core::evidence::image::ReadFaultReport, state::AppState};
use std::{path::PathBuf, sync::Arc};

pub(crate) async fn loaded_image_app_error(
    state: &State<'_, AppState>,
    image_id: &str,
) -> Result<Arc<EvidenceSession>, app_error::AppError> {
    let images = state.images.lock().await;
    images.get(image_id).cloned().ok_or_else(|| {
        app_error::AppError::not_found("image not loaded")
            .with_context_value(serde_json::json!({ "image_id": image_id }))
    })
}

/// The active case directory when `image_id` is evidence of it. Per-evidence
/// writes (digests, type identifications) go nowhere otherwise: a loaded
/// image that is not part of the case keeps its results session-only.
pub(crate) async fn evidence_case_dir(state: &AppState, image_id: &str) -> Option<PathBuf> {
    let active_case = state.active_case.lock().await;
    active_case
        .as_ref()
        .filter(|case| case.has_evidence(image_id))
        .map(|case| case.dir.clone())
}

/// Folder-backed evidence has no underlying byte stream — commands that
/// scan / parse raw bytes (hex, BitLocker) must short-
/// circuit before touching the image. Returns an `Err` that the IPC layer
/// surfaces to the frontend; views are also expected to hide the entry
/// points but this is the backend safety net.
pub(crate) fn reject_if_folder(image: &EvidenceSession) -> Result<(), app_error::AppError> {
    if image.folder.is_some() {
        return Err(app_error::AppError::new(
            app_error::ErrorCode::UnsupportedFormat,
            "folder evidence does not support this operation",
        ));
    }
    Ok(())
}

/// Merge the image's session fault registry with its active-case audit store.
/// The async case lock is held only long enough to clone the directory path;
/// all filesystem I/O runs on the blocking pool.
pub(crate) async fn image_read_fault_report(
    state: &State<'_, AppState>,
    image_id: &str,
    image: &EvidenceSession,
) -> Result<ReadFaultReport, app_error::AppError> {
    let live_regions = image.bad_regions();
    let case_dir = {
        let active_case = state.active_case.lock().await;
        active_case.as_ref().map(|case| case.dir.clone())
    };
    let Some(case_dir) = case_dir else {
        return Ok(ReadFaultReport::from_regions(live_regions));
    };
    let image_id = image_id.to_string();
    tokio::task::spawn_blocking(move || {
        if live_regions.is_empty() {
            crate::core::casework::bad_region_store::load(&case_dir, &image_id)
        } else {
            crate::core::casework::bad_region_store::merge_save(&case_dir, &image_id, live_regions)
        }
    })
    .await
    .map_err(|error| app_error::AppError::task_panicked("bad-region store", error))?
    .map_err(app_error::AppError::from)
}
