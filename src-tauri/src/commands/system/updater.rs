//! Fixed-repository update IPC. The desktop host emits `app-update-progress`.
use crate::host::{AppHandle, State};

pub use crate::app::updater::ReleaseUpdateInfo;
use crate::{
    app::{error::AppError, updater},
    state::AppState,
};

/// Checks the latest published stable GitHub release, including legacy releases.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn release_update_check(app: AppHandle) -> Result<ReleaseUpdateInfo, AppError> {
    let client = updater::client().map_err(AppError::from)?;
    let mut info = updater::latest_release(&client)
        .await
        .map_err(AppError::from)?
        .info(&updater::current_version().map_err(AppError::from)?)
        .map_err(AppError::from)?;
    info.can_install &= app.is_desktop();
    Ok(info)
}

/// Verifies the selected release before starting a quiet update and restart.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn release_update_install(
    app: AppHandle,
    state: State<'_, AppState>,
    version: String,
) -> Result<(), AppError> {
    let _guard = state
        .update_install
        .try_lock()
        .map_err(|_| AppError::invalid_input("an update is already in progress"))?;
    updater::install(&app, &version).await
}
