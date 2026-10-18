//! Read-only BCP catalog and bounded record pages for exported evidence.

use std::path::PathBuf;

use crate::host::State;

use crate::{
    commands::app_error::AppError,
    core::analysis::bcp::{self, BcpCatalog, BcpCursor, BcpPage},
    state::AppState,
};

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn bcp_catalog(
    state: State<'_, AppState>,
    image_id: String,
) -> Result<BcpCatalog, AppError> {
    let image = crate::app::access::loaded_image_app_error(&state, &image_id).await?;
    if image.folder.is_none() {
        return Err(AppError::invalid_input("BCP 目录必须作为文件夹检材加载"));
    }
    let path = PathBuf::from(&image.path);
    tokio::task::spawn_blocking(move || bcp::catalog(&path))
        .await
        .map_err(|error| AppError::from_message(format!("BCP 索引任务失败：{error}")))?
        .map_err(AppError::from_anyhow)
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn bcp_rows(
    state: State<'_, AppState>,
    image_id: String,
    code: String,
    cursor: Option<BcpCursor>,
) -> Result<BcpPage, AppError> {
    let image = crate::app::access::loaded_image_app_error(&state, &image_id).await?;
    if image.folder.is_none() {
        return Err(AppError::invalid_input("BCP 目录必须作为文件夹检材加载"));
    }
    let path = PathBuf::from(&image.path);
    tokio::task::spawn_blocking(move || bcp::page(&path, &code, cursor))
        .await
        .map_err(|error| AppError::from_message(format!("BCP 读取任务失败：{error}")))?
        .map_err(AppError::from_anyhow)
}
