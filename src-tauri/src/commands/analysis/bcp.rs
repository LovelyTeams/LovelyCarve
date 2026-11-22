//! Read-only BCP catalog and bounded record pages for exported evidence.

use std::sync::Arc;

use crate::core::common::ids::ImageId;
use crate::host::State;

use crate::{
    commands::app_error::AppError,
    core::{
        analysis::bcp::{import::BcpImportResult, BcpCatalog, BcpCursor, BcpPackage, BcpPage},
        filesystem::evidence_session::EvidenceSession,
    },
    state::AppState,
};

/// Imports and publishes BCP through the normal analysis records store.
/// Progress is delivered through the import's native task-center entry.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn bcp_import(
    state: State<'_, AppState>,
    image_id: ImageId,
    replace: Option<bool>,
) -> Result<BcpImportResult, AppError> {
    crate::app::workflows::bcp::import_bcp(state, image_id, replace.unwrap_or(false)).await
}

fn package(image: &EvidenceSession) -> Result<Arc<BcpPackage>, AppError> {
    if image.folder.is_none() && image.zip.is_none() {
        return Err(AppError::invalid_input("BCP 必须作为文件夹或 ZIP 检材加载"));
    }
    let mut cached = image.bcp_package.lock();
    if let Some(package) = cached.as_ref() {
        return Ok(Arc::clone(package));
    }
    let package = Arc::new(BcpPackage::open(&image.path, image.zip.clone())?);
    *cached = Some(Arc::clone(&package));
    Ok(package)
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn bcp_catalog(
    state: State<'_, AppState>,
    image_id: ImageId,
) -> Result<BcpCatalog, AppError> {
    let image = crate::app::access::loaded_image_app_error(&state, &image_id).await?;
    tokio::task::spawn_blocking(move || Ok(package(&image)?.catalog().clone()))
        .await
        .map_err(|error| AppError::task_panicked("BCP 索引", error))?
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn bcp_rows(
    state: State<'_, AppState>,
    image_id: ImageId,
    code: String,
    cursor: Option<BcpCursor>,
    start_row: Option<u64>,
) -> Result<BcpPage, AppError> {
    let image = crate::app::access::loaded_image_app_error(&state, &image_id).await?;
    tokio::task::spawn_blocking(move || {
        package(&image)?
            .page(&code, cursor, start_row)
            .map_err(AppError::from)
    })
    .await
    .map_err(|error| AppError::task_panicked("BCP 读取", error))?
}
