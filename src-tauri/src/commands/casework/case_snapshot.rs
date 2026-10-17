//! Volume-snapshot orchestration for the active case.
//!
//! When evidence joins an active case every browsable volume is walked into a
//! `.dirz` snapshot in the case `staging/` directory, published into the
//! evidence's volume directory, then imported into a standalone index file
//! that is published the same way (`core::casework::case`, `core::casework::case_ledger_volumes`).
//! The walk and the import run outside the case lock; only the publications
//! take it, briefly, on a blocking thread. Case lifecycle commands live in
//! `commands::casework::case`.
//!
//! Snapshot builds are long, cancellable background tasks: each registers a
//! task in `AppState::tasks` under a deterministic id
//! (`snapshot:<image_id>:<volume_key>` for auto-builds, or the caller's
//! `task_id` for a manual rebuild) and emits [`SNAPSHOT_PROGRESS_EVENT`]
//! events. Cancel via the existing `task_cancel` command.

use crate::host::AppHandle;
use crate::host::State;

use crate::app::error::AppError;
use crate::core::casework::case::SnapshotInfo;
use crate::state::AppState;

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn snapshot_build(
    app: AppHandle,
    state: State<'_, AppState>,
    image_id: String,
    volume_key: String,
    task_id: String,
) -> Result<(), AppError> {
    crate::app::workflows::snapshot::case_snapshot::snapshot_build(
        app, state, image_id, volume_key, task_id,
    )
    .await
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn snapshot_list(state: State<'_, AppState>) -> Result<Vec<SnapshotInfo>, AppError> {
    crate::app::workflows::snapshot::case_snapshot::snapshot_list(state).await
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn snapshot_drop(
    state: State<'_, AppState>,
    image_id: String,
    volume_key: String,
) -> Result<(), AppError> {
    crate::app::workflows::snapshot::case_snapshot::snapshot_drop(state, image_id, volume_key).await
}

pub use crate::app::workflows::snapshot::case_snapshot::{
    SnapshotProgressPayload, SNAPSHOT_PROGRESS_EVENT,
};
