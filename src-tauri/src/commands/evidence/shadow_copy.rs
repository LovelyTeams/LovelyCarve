//! List an image's Volume Shadow Copies and open one as a session image.

use crate::host::State;
use std::sync::Arc;

use crate::core::evidence::image::ImageInfo;
use crate::core::evidence::vss_image::{self, ShadowVolumeInfo};
use crate::state::AppState;
use crate::task_manager::TaskKind;
use {
    crate::app::access::loaded_image_app_error, crate::app::access::reject_if_folder,
    crate::app::error::AppError,
};

/// Shadow copies of every NTFS volume of the image, oldest first, with any
/// session image already open for each.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn shadow_copy_list(
    state: State<'_, AppState>,
    image_id: String,
) -> Result<Vec<ShadowVolumeInfo>, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    reject_if_folder(&image)?;
    let mut volumes = tokio::task::spawn_blocking(move || vss_image::list(&image))
        .await
        .map_err(|error| AppError::task_panicked("shadow copy listing", error))?;
    let images = state.images.lock().await;
    for volume in &mut volumes {
        for copy in &mut volume.shadow_copies {
            let prefix =
                vss_image::session_path_prefix(&image_id, volume.partition_id, &copy.store_id);
            copy.open_image_id = images
                .values()
                .find(|image| image.path.to_string_lossy().starts_with(&prefix))
                .map(|image| image.id.clone());
        }
    }
    Ok(volumes)
}

/// Reconstruct shadow copy `index` of the NTFS volume in `partition_id` and
/// register it as a read-only session image. Reuses an already open session
/// for the same shadow copy.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn shadow_copy_open(
    state: State<'_, AppState>,
    image_id: String,
    partition_id: u8,
    index: usize,
    task_id: Option<String>,
) -> Result<ImageInfo, AppError> {
    let source = loaded_image_app_error(&state, &image_id).await?;
    reject_if_folder(&source)?;
    let task_id = task_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let task = state
        .tasks
        .register_if_absent(task_id, TaskKind::ShadowCopyOpen, Some(image_id.clone()))
        .ok_or_else(|| AppError::invalid_input("task id is already in use"))?;
    let permit = state.tasks.acquire_resources(&task).await?;
    let token = task.token();
    let source_for_open = Arc::clone(&source);
    let token_for_open = token.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let _task = task;
        vss_image::open(&source_for_open, partition_id, index, &token_for_open)
    })
    .await
    .map_err(|error| AppError::task_panicked("shadow copy open", error))?;
    if token.is_cancelled() {
        return Err(AppError::cancelled("shadow copy open cancelled"));
    }
    let snapshot = result.map_err(AppError::from_anyhow)?;
    let mut images = state.images.lock().await;
    if !images
        .get(&image_id)
        .is_some_and(|current| Arc::ptr_eq(current, &source))
    {
        return Err(AppError::source_changed(
            "source image was unloaded while opening the shadow copy",
        ));
    }
    if let Some(open) = images.values().find(|image| image.path == snapshot.path) {
        return Ok(open.info());
    }
    let info = snapshot.info();
    images.insert(
        snapshot.id.clone(),
        Arc::new(crate::core::filesystem::evidence_session::EvidenceSession::from(snapshot)),
    );
    Ok(info)
}
