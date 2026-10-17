//! Docker preparation command adapter.
pub use crate::app::workflows::docker::DockerIndexStatus;
use crate::host::State;
use crate::{app::error::AppError, core::catalog::host_docker_index::PrepareMode, state::AppState};

/// Status only validates/restores existing data; build composes and saves the
/// selected container. No sibling containers are analyzed and no events are
/// emitted: callers await the complete operation to finish their task row.
/// The build runs outside the case lock; its snapshot and index are then
/// published into the case like any other volume.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_docker_prepare(
    state: State<'_, AppState>,
    image_id: String,
    root_id: String,
    case_dir: Option<String>,
    mode: PrepareMode,
) -> Result<DockerIndexStatus, AppError> {
    crate::app::workflows::docker::fs_docker_prepare(state, image_id, root_id, case_dir, mode).await
}
