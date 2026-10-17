//! Read-only iOS backup unlock and upstream anomaly audit.
use std::sync::Arc;

use crate::host::State;

use crate::app::{access::loaded_image_app_error, error::AppError};
use crate::core::evidence::image::ImageInfo;
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::state::AppState;
use crate::task_manager::TaskKind;

/// Unlock into a child session. Cancellation is checked around the upstream
/// key derivation, which cannot be interrupted inside a PBKDF2 round.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn ios_backup_unlock(
    state: State<'_, AppState>,
    image_id: String,
    password: String,
    task_id: Option<String>,
) -> Result<ImageInfo, AppError> {
    let password = ios_backup_core::Password::from_bytes(password.into_bytes());
    let source = loaded_image_app_error(&state, &image_id).await?;
    let task = state
        .tasks
        .register_if_absent(
            task_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            TaskKind::IosBackupUnlock,
            Some(image_id.clone()),
        )
        .ok_or_else(|| AppError::invalid_input("task id is already in use"))?;
    let permit = state.tasks.acquire_resources(&task).await?;
    let token = task.token();
    let source_for_unlock = Arc::clone(&source);
    let token_for_unlock = token.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let _task = task;
        if token_for_unlock.is_cancelled() {
            anyhow::bail!("iOS backup unlock cancelled");
        }
        source_for_unlock.unlock_ios(password, &token_for_unlock)
    })
    .await
    .map_err(|error| AppError::task_panicked("iOS backup unlock", error))?;
    if token.is_cancelled() {
        return Err(AppError::cancelled("iOS backup unlock cancelled"));
    }
    let image = result.map_err(AppError::from_anyhow)?;
    let mut images = state.images.lock().await;
    if !images
        .get(&image_id)
        .is_some_and(|current| Arc::ptr_eq(current, &source))
    {
        return Err(AppError::source_changed(
            "iOS evidence unloaded during unlock",
        ));
    }
    let info = image.info();
    images.insert(image.id.clone(), Arc::new(EvidenceSession::from(image)));
    Ok(info)
}

/// Return native upstream findings (code, severity, evidence, source and note).
/// Cancellation is polled during the manifest and blob directory traversals.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn ios_backup_audit(
    state: State<'_, AppState>,
    image_id: String,
    task_id: Option<String>,
) -> Result<serde_json::Value, AppError> {
    let source = loaded_image_app_error(&state, &image_id).await?;
    let folder = source
        .folder
        .clone()
        .ok_or_else(|| AppError::invalid_input("not a folder evidence"))?;
    let task = state
        .tasks
        .register_if_absent(
            task_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            TaskKind::IosBackupAudit,
            Some(image_id.clone()),
        )
        .ok_or_else(|| AppError::invalid_input("task id is already in use"))?;
    let permit = state.tasks.acquire_resources(&task).await?;
    let token = task.token();
    let token_for_audit = token.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let _task = task;
        if token_for_audit.is_cancelled() {
            anyhow::bail!("iOS backup audit cancelled");
        }
        folder.audit_ios(&token_for_audit)
    })
    .await
    .map_err(|error| AppError::task_panicked("iOS backup audit", error))?;
    if token.is_cancelled() {
        return Err(AppError::cancelled("iOS backup audit cancelled"));
    }
    if !state
        .images
        .lock()
        .await
        .get(&image_id)
        .is_some_and(|current| Arc::ptr_eq(current, &source))
    {
        return Err(AppError::source_changed(
            "iOS evidence unloaded during audit",
        ));
    }
    result.map_err(AppError::from_anyhow)
}
