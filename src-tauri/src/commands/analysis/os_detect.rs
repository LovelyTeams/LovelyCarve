//! OS detection IPC. Walks every partition (and LVM logical volume) of
//! a loaded image, runs the seven `core::analysis::identify::os_detect` detectors against
//! each filesystem, and returns the structured per-partition report.
//! The work is heavy enough (NTFS root listings, registry hive parsing)
//! that it runs on `tokio::task::spawn_blocking` so we never block the
//! tokio runtime while the user waits.

use crate::host::State;

use crate::core::analysis::identify::os_detect_adapters::{self, OsDetectionResult};
use crate::state::AppState;
use {crate::app::access::loaded_image_app_error, crate::app::error::AppError};

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn os_detect(
    state: State<'_, AppState>,
    image_id: String,
) -> Result<OsDetectionResult, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    tokio::task::spawn_blocking(move || {
        os_detect_adapters::detect_image(&image)
            .map(|partitions| OsDetectionResult {
                image_id,
                partitions,
            })
            .map_err(AppError::from)
    })
    .await
    .map_err(|err| AppError::task_panicked("os_detect", err))?
}
