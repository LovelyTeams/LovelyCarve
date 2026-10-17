//! Filesystem named-stream and directory-stat IPC commands.
//!
//! These commands stay separate from the broad filesystem tree command module
//! because they share a small, self-contained backend surface: NTFS ADS
//! listing/read/export and recursive directory statistics.

use std::path::PathBuf;

use crate::host::State;

use crate::core::filesystem::tree::fs_tree::{self, FsDirStats};
use crate::core::filesystem::volumes::ntfs::NtfsDataStream;
use crate::state::AppState;
use {crate::app::access::loaded_image_app_error, crate::app::error::AppError};

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_streams(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
) -> Result<Vec<NtfsDataStream>, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    tokio::task::spawn_blocking(move || -> Result<Vec<NtfsDataStream>, AppError> {
        fs_tree::list_data_streams(&image, &node_id).map_err(AppError::from_anyhow)
    })
    .await
    .map_err(|error| AppError::task_panicked("fs_streams", error))?
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_stream_preview(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    stream_name: String,
    max_bytes: usize,
) -> Result<Vec<u8>, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    tokio::task::spawn_blocking(move || -> Result<Vec<u8>, AppError> {
        fs_tree::read_named_stream(
            &image,
            &node_id,
            &stream_name,
            max_bytes.min(2 * 1024 * 1024),
        )
        .map_err(AppError::from_anyhow)
    })
    .await
    .map_err(|error| AppError::task_panicked("fs_stream_preview", error))?
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_stream_export(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    stream_name: String,
    destination: String,
) -> Result<u64, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    tokio::task::spawn_blocking(move || -> Result<u64, AppError> {
        fs_tree::export_named_stream(&image, &node_id, &stream_name, &PathBuf::from(destination))
            .map_err(AppError::from_anyhow)
    })
    .await
    .map_err(|error| AppError::task_panicked("fs_stream_export", error))?
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_dir_stats(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    path_hint: String,
) -> Result<FsDirStats, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    tokio::task::spawn_blocking(move || -> Result<FsDirStats, AppError> {
        fs_tree::directory_stats(&image, &node_id, &path_hint, 20_000)
            .map_err(AppError::from_anyhow)
    })
    .await
    .map_err(|error| AppError::task_panicked("fs_dir_stats", error))?
}
