//! Image lifecycle (load / info / unload / list / raw export) and RAID
//! auto-detection. The exporter emits `image-export-progress` events so
//! the frontend renders a progress bar without polling.

use crate::host::AppHandle;
use crate::host::State;

use crate::app::error::AppError;
use crate::core::evidence::image::ImageInfo;
use crate::core::evidence::image::ReadFaultPolicy;
use crate::core::evidence::image::ReadFaultReport;
use crate::core::evidence::physical_disk::PhysicalDisk;
use crate::core::evidence::raid;
use crate::core::evidence::raid::RaidStack;
use crate::core::filesystem::evidence_session::ImageExportMode;
use crate::state::AppState;

#[cfg_attr(feature = "desktop", tauri::command)]
/// Opens on a blocking worker; `task_id` opts into a caller-owned task identity.
/// Emits `image-load-progress` for the legacy loader and `task-progress` for
/// the task center. `task_cancel` interrupts materialisation and drops tempfiles.
pub async fn image_load(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    qcow_password: Option<String>,
    qcow_key_hex: Option<String>,
    task_id: Option<String>,
    case_dir: Option<String>,
) -> Result<ImageInfo, AppError> {
    let password = qcow_password.map(zeroize::Zeroizing::new);
    let key_hex = qcow_key_hex.map(zeroize::Zeroizing::new);
    let key = match (password, key_hex) {
        (Some(_), Some(_)) => {
            return Err(AppError::invalid_input(
                "choose QCOW password or key, not both",
            ))
        }
        (Some(password), None) => Some(
            qcow2::EncryptionKey::from_password(&password)
                .map_err(|error| AppError::invalid_input(error.to_string()))?,
        ),
        (None, Some(key_hex)) => {
            let bytes = zeroize::Zeroizing::new(hex::decode(key_hex.as_str()).map_err(|_| {
                AppError::invalid_input("QCOW key must use hexadecimal characters")
            })?);
            let key: [u8; 16] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| AppError::invalid_input("QCOW AES key must be exactly 16 bytes"))?;
            Some(qcow2::EncryptionKey::from_bytes(key))
        }
        (None, None) => None,
    };
    crate::app::workflows::evidence::image_load(app, state, path, key, task_id, case_dir).await
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn image_info(
    state: State<'_, AppState>,
    image_id: String,
) -> Result<ImageInfo, AppError> {
    crate::app::workflows::evidence::image_info(state, image_id).await
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn image_unload(state: State<'_, AppState>, image_id: String) -> Result<(), AppError> {
    crate::app::workflows::evidence::image_unload(state, image_id).await
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn image_list(state: State<'_, AppState>) -> Result<Vec<ImageInfo>, AppError> {
    crate::app::workflows::evidence::image_list(state).await
}

/// Detect LVM volume groups and Linux md arrays (metadata 0.90 / 1.x, linear
/// and RAID0/1/4/5/6/10) across every loaded image, plus metadata-less
/// RAID0/5 member sets, and register each assembled array — and each LV that
/// spans several physical volumes — as a synthetic `DiskImage` so it shows up
/// in the FS tree, hex view and image switcher like a regular image.
///
/// `degraded_sets` lists md set UUIDs the examiner chose to assemble without
/// the members their metadata still expects. Idempotent: re-running on the
/// same image set reuses the previously registered synthetic images.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn raid_auto_detect(
    state: State<'_, AppState>,
    degraded_sets: Option<Vec<String>>,
) -> Result<RaidStack, AppError> {
    crate::app::workflows::evidence::raid_auto_detect(state, degraded_sets.unwrap_or_default())
        .await
}

/// Assemble a RAID device from an explicit, user-supplied geometry (the manual
/// "RAID builder" fallback for arrays the automatic heuristic cannot crack) and
/// register it as a synthetic image so the rest of the app can browse / export
/// it. Idempotent on the geometry: re-running with the same parameters reuses
/// the previously registered synthetic image.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn raid_manual_assemble(
    state: State<'_, AppState>,
    params: raid::ManualRaidParams,
) -> Result<ManualRaidResult, AppError> {
    crate::app::workflows::evidence::raid_manual_assemble(state, params).await
}

/// Stream an entire image (regular or assembled) to a raw `.dd` file at
/// `destination`. Emits `image-export-progress` events with a
/// `{ task_id, bytes_done, bytes_total }` payload at every chunk.
///
/// Cancellation: the caller passes a `task_id` it has just registered via
/// the existing `task_cancel` IPC. The exporter checks that token between
/// chunks and aborts cleanly with a partial file (the partial result is
/// not deleted — leaving artefact recovery to the user).
/// `mode: unlockedPartitions` snapshots all currently unlocked partitions and
/// writes their plaintext into the original disk layout. Omitted mode is source.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn image_export_raw(
    app: AppHandle,
    state: State<'_, AppState>,
    image_id: String,
    destination: String,
    task_id: String,
    read_fault_policy: Option<ReadFaultPolicy>,
    mode: Option<ImageExportMode>,
) -> Result<ImageExportResult, AppError> {
    crate::app::workflows::evidence::image_export_raw(
        app,
        state,
        image_id,
        destination,
        task_id,
        read_fault_policy,
        mode,
    )
    .await
}

/// Return every source span known to have been synthesized as zero bytes.
/// When a case is active this also loads/merges the persisted audit, so the
/// report survives app restarts and evidence reloads.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn image_bad_regions(
    state: State<'_, AppState>,
    image_id: String,
) -> Result<ReadFaultReport, AppError> {
    crate::app::workflows::evidence::image_bad_regions(state, image_id).await
}

/// Enumerate physical disks attached to the host. Returned as a best-effort
/// list — disks that fail to open (permission denied, removed mid-scan) are
/// silently skipped so the UI still shows the rest. The enumeration runs on
/// a blocking thread because Windows' IOCTL calls are synchronous.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn physical_disks_list() -> Result<Vec<PhysicalDisk>, AppError> {
    crate::app::workflows::evidence::physical_disks_list().await
}

pub use crate::app::workflows::evidence::{
    ImageExportProgress, ImageExportResult, ImageLoadProgress, ManualPartitionInfo,
    ManualRaidResult,
};
