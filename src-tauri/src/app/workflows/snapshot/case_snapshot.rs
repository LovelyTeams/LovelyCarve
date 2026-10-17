//! Volume-snapshot orchestration for the active case.
//!
//! When evidence joins an active case every browsable volume is walked into a
//! `.dirz` snapshot in the case `staging/` directory, published into the
//! evidence's volume directory, then imported into a standalone index file
//! that is published the same way (`core::casework::case`, `core::casework::case_ledger_volumes`).
//! The walk and the import run outside the case lock; only the publications
//! take it, briefly, on a blocking thread. Case lifecycle commands live in
//! `app::workflows::casework`.
//!
//! Snapshot builds are long, cancellable background tasks: each registers a
//! task in `AppState::tasks` under a deterministic id
//! (`snapshot:<image_id>:<volume_key>` for auto-builds, or the caller's
//! `task_id` for a manual rebuild) and emits [`SNAPSHOT_PROGRESS_EVENT`]
//! events. Cancel via the existing `task_cancel` command.

mod progress;
pub(crate) use progress::{emit_progress, emit_unified_snapshot_progress, report};
mod registration;
#[cfg(test)]
use registration::*;
pub(crate) use registration::{
    register_and_snapshot, snapshot_already_loaded_images, snapshot_unlocked_volume,
};
mod build;
pub(crate) use build::{remove_staged_file, spawn_volume_snapshot};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::host::{AppHandle, State};
use serde::Serialize;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::app::access::loaded_image_app_error;
use crate::app::error::AppError;
use crate::app::workflows::snapshot::snapshot_install::{index_published_snapshot, IndexJob};
use crate::core::casework::case::{
    self, CaseState, EvidenceRef, EvidenceRegistration, SnapshotInfo, SnapshotPublish,
};
use crate::core::casework::case_ledger;
use crate::core::casework::case_ledger_volumes::{SnapshotPublication, VolumeRecord};
use crate::core::casework::case_paths;
use crate::core::catalog::snapshot::volume_snapshot::{
    self, volume_size_for_key, DEFAULT_MAX_SNAPSHOT_ENTRIES,
};
use crate::core::evidence::image::{ImageFormat, PartitionKind};
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::fs_all;
use crate::state::AppState;
use crate::task_manager::{TaskKind, TaskManager, TaskProgressUnit, TaskRegistration};

/// Event name carrying snapshot-build progress to the frontend.
pub const SNAPSHOT_PROGRESS_EVENT: &str = "snapshot-build-progress";

/// Throttle for `walking` / `indexing` progress emits during a build.
pub(crate) const PROGRESS_THROTTLE: Duration = Duration::from_millis(150);

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotProgressPayload {
    pub task_id: String,
    pub image_id: String,
    pub volume_key: String,
    /// One of: `started` | `reused` | `walking` | `indexing` | `done` |
    /// `cancelled` | `error`.
    pub phase: String,
    pub entries: u64,
    pub truncated: bool,
    pub message: Option<String>,
}

impl SnapshotProgressPayload {
    pub(crate) fn new(task_id: &str, image_id: &str, volume_key: &str, phase: &str) -> Self {
        Self {
            task_id: task_id.to_string(),
            image_id: image_id.to_string(),
            volume_key: volume_key.to_string(),
            phase: phase.to_string(),
            entries: 0,
            truncated: false,
            message: None,
        }
    }
}

/// How a loaded image was admitted into the active case.
enum Admission {
    /// No case is open.
    Caseless,
    /// Registered (or already registered) as evidence of the active case.
    Admitted { first_online: bool },
    /// The user removed this image from the case while it was loading.
    Removed,
    /// The same source is already another evidence of the case.
    SourceTaken { existing_image_id: String },
}

/// The published state of one volume, captured under the case lock.
struct PublishedVolume {
    record: VolumeRecord,
    snapshot_path: PathBuf,
    index_path: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

pub async fn snapshot_build(
    app: AppHandle,
    state: State<'_, AppState>,
    image_id: String,
    volume_key: String,
    task_id: String,
) -> Result<(), AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    image.require_source()?;
    {
        let guard = state.active_case.lock().await;
        match guard.as_ref() {
            None => {
                return Err(AppError::invalid_input(
                    "no active case — open or create a case first",
                ));
            }
            Some(case) if case.is_locked() => {
                return Err(AppError::invalid_input(
                    "case is locked; unlock it before rebuilding snapshots",
                ));
            }
            Some(_) => {}
        }
    }
    spawn_volume_snapshot(
        app,
        Arc::clone(&state.active_case),
        state.tasks.clone(),
        image,
        volume_key,
        task_id,
        true,
    );
    Ok(())
}

pub async fn snapshot_list(state: State<'_, AppState>) -> Result<Vec<SnapshotInfo>, AppError> {
    let images = {
        let images = state.images.lock().await;
        images.clone()
    };
    let guard = state.active_case.lock().await;
    let Some(case) = guard.as_ref() else {
        return Ok(Vec::new());
    };
    Ok(case
        .volumes()
        .iter()
        .map(|record| SnapshotInfo {
            snapshot: case::snapshot_ref(record),
            stale: is_stale(record, &case.snapshot_path(record), &images),
        })
        .collect())
}

pub async fn snapshot_drop(
    state: State<'_, AppState>,
    image_id: String,
    volume_key: String,
) -> Result<(), AppError> {
    {
        let guard = state.active_case.lock().await;
        if guard.is_none() {
            return Err(AppError::invalid_input("no active case"));
        }
    }
    let active_case = Arc::clone(&state.active_case);
    tokio::task::spawn_blocking(move || {
        let mut guard = active_case.blocking_lock();
        match guard.as_mut() {
            Some(case) => case.drop_volume(&image_id, &volume_key),
            None => Ok(()),
        }
    })
    .await
    .map_err(|err| AppError::task_panicked("snapshot_drop", err))?
    .map_err(AppError::from)
}

/// A snapshot is stale if its `.dirz` is gone, or the source volume of a
/// currently-loaded image no longer matches the size captured at build time.
/// When the source image isn't loaded we can't compare, so report fresh.
fn is_stale(
    record: &VolumeRecord,
    snapshot_path: &Path,
    images: &HashMap<String, Arc<EvidenceSession>>,
) -> bool {
    if !snapshot_path.is_file() {
        return true;
    }
    match images.get(&record.image_id) {
        Some(image) => {
            !image.is_index_only()
                && volume_size_for_key(image, &record.volume_key) != record.source_size
        }
        None => false,
    }
}

#[cfg(test)]
mod tests;
