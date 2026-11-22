//! Filesystem named-stream and directory-stat IPC commands.
//!
//! These commands stay separate from the broad filesystem tree command module
//! because they share a small backend surface: NTFS / ReFS streams and HFS forks
//! listing/read/export and recursive directory statistics.

use crate::core::common::ids::{ImageId, NodeId};
use std::path::PathBuf;

use crate::host::State;

use crate::core::filesystem::tree::fs_tree::{self, FsDirStats};
use crate::core::filesystem::volumes::ntfs::NtfsDataStream;
use crate::state::AppState;
use {crate::app::access::loaded_image_app_error, crate::app::error::AppError};

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_streams(
    state: State<'_, AppState>,
    image_id: ImageId,
    node_id: NodeId,
) -> Result<Vec<NtfsDataStream>, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    tokio::task::spawn_blocking(move || -> Result<Vec<NtfsDataStream>, AppError> {
        fs_tree::list_data_streams(&image, &node_id).map_err(AppError::from)
    })
    .await
    .map_err(|error| AppError::task_panicked("fs_streams", error))?
}

/// Read up to `max_bytes` (hard-capped at 2 MiB) of one named data stream as a
/// raw binary response (`ArrayBuffer` in the WebView, binary CLI output), like
/// `fs_preview`, instead of a JSON integer array several times its size.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_stream_preview(
    state: State<'_, AppState>,
    image_id: ImageId,
    node_id: NodeId,
    stream_name: String,
    max_bytes: usize,
) -> Result<crate::host::Response, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let data = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, AppError> {
        fs_tree::read_named_stream(
            &image,
            &node_id,
            &stream_name,
            max_bytes.min(2 * 1024 * 1024),
        )
        .map_err(AppError::from)
    })
    .await
    .map_err(|error| AppError::task_panicked("fs_stream_preview", error))??;
    Ok(crate::host::Response::new(data))
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_stream_export(
    state: State<'_, AppState>,
    image_id: ImageId,
    node_id: NodeId,
    stream_name: String,
    destination: String,
) -> Result<u64, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    tokio::task::spawn_blocking(move || -> Result<u64, AppError> {
        fs_tree::export_named_stream(&image, &node_id, &stream_name, &PathBuf::from(destination))
            .map_err(AppError::from)
    })
    .await
    .map_err(|error| AppError::task_panicked("fs_stream_export", error))?
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_dir_stats(
    state: State<'_, AppState>,
    image_id: ImageId,
    node_id: NodeId,
    path_hint: String,
) -> Result<FsDirStats, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    tokio::task::spawn_blocking(move || -> Result<FsDirStats, AppError> {
        fs_tree::directory_stats(&image, &node_id, &path_hint, 20_000).map_err(AppError::from)
    })
    .await
    .map_err(|error| AppError::task_panicked("fs_dir_stats", error))?
}
