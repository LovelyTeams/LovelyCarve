//! Image lifecycle (load / info / unload / list / raw export) and RAID
//! auto-detection. The exporter emits `image-export-progress` events so
//! the frontend renders a progress bar without polling.

use std::{
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::host::{AppHandle, State};
use serde::Serialize;

use crate::core::evidence::image::{
    DerivedImage, ImageInfo, Partition, ReadFaultPolicy, ReadFaultReport,
};
use crate::core::evidence::physical_disk::{self, PhysicalDisk};
use crate::core::evidence::raid::{self, RaidStack};
use crate::state::AppState;
use crate::task_manager::{TaskKind, TaskProgressUnit};
use {
    crate::app::access::image_read_fault_report,
    crate::app::access::loaded_image_app_error,
    crate::app::error::{AppError, ErrorCode},
};

#[derive(Debug, Clone, Serialize)]
pub struct ImageExportProgress {
    pub task_id: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImageExportResult {
    pub bytes_written: u64,
    #[serde(flatten)]
    pub read_faults: ReadFaultReport,
}

const EXPORT_CHUNK_BYTES: usize = 16 * 1024 * 1024;
const EXPORT_PROGRESS_THROTTLE: Duration = Duration::from_millis(150);

/// Phase + progress of an in-flight `image_load`, emitted as `image-load-progress`
/// so the loading overlay shows what's happening instead of a blank spinner.
/// `path` lets a tab filter to the load it kicked off; `current`/`total` are
/// bytes (decode) or partition counts, and `total == 0` means "no percentage".
#[derive(Debug, Clone, Serialize)]
pub struct ImageLoadProgress {
    pub path: String,
    pub stage: String,
    pub current: u64,
    pub total: u64,
}

/// Throttle window for byte-level decode updates so a long decompress emits a
/// dozen events per second rather than thousands. Phase changes and the final
/// update bypass it so the label/percentage never lags.
const LOAD_PROGRESS_THROTTLE: std::time::Duration = std::time::Duration::from_millis(80);

pub async fn image_info(
    state: State<'_, AppState>,
    image_id: String,
) -> Result<ImageInfo, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    Ok(image.info())
}

pub async fn image_list(state: State<'_, AppState>) -> Result<Vec<ImageInfo>, AppError> {
    // Every loaded image lives in `state.images` — real images and synthetic
    // assembled-RAID images alike — so a flat snapshot is the full list.
    let images = state.images.lock().await;
    Ok(images.values().map(|img| img.info()).collect())
}

/// One partition of a manually-assembled RAID device, for the builder preview.
#[derive(serde::Serialize)]
pub struct ManualPartitionInfo {
    pub kind: String,
    pub start_lba: u64,
    pub length_lba: u64,
    pub label: Option<String>,
}

/// Result of `raid_manual_assemble`: the registered synthetic image plus a
/// preview of the partitions the geometry yields.
#[derive(serde::Serialize)]
pub struct ManualRaidResult {
    pub image_id: String,
    pub size_bytes: u64,
    pub partitions: Vec<ManualPartitionInfo>,
}

/// Return every source span known to have been synthesized as zero bytes.
/// When a case is active this also loads/merges the persisted audit, so the
/// report survives app restarts and evidence reloads.
pub async fn image_bad_regions(
    state: State<'_, AppState>,
    image_id: String,
) -> Result<ReadFaultReport, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    image_read_fault_report(&state, &image_id, &image).await
}

/// Enumerate physical disks attached to the host. Returned as a best-effort
/// list — disks that fail to open (permission denied, removed mid-scan) are
/// silently skipped so the UI still shows the rest. The enumeration runs on
/// a blocking thread because Windows' IOCTL calls are synchronous.
pub async fn physical_disks_list() -> Result<Vec<PhysicalDisk>, AppError> {
    tokio::task::spawn_blocking(physical_disk::enumerate)
        .await
        .map_err(|err| AppError::task_panicked("physical_disks_list", err))
}

mod indexed;
mod load;
pub(crate) use load::image_load;

mod unload;
pub(crate) use unload::{image_unload, purge_image_scans, remove_image_runtime};

mod assembly;
pub(crate) use assembly::{raid_auto_detect, raid_manual_assemble};

mod export;
pub(crate) use export::image_export_raw;
