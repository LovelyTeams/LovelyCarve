//! Filesystem export IPC: single-file export, directory tree / ZIP export,
//! and streaming body-file timeline export.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::host::{AppHandle, State};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::core::filesystem::tree::fs_tree::{self, FsExportResult, FsExportSummary};
use crate::state::AppState;
use crate::task_manager::{TaskCount, TaskFlag, TaskKind, TaskOutcome, TaskProgressUnit};
use {
    crate::app::access::image_read_fault_report, crate::app::access::loaded_image_app_error,
    crate::app::error::AppError,
};

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_export(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    destination: String,
) -> Result<FsExportResult, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let node_for_log = node_id.clone();
    let dest_for_log = destination.clone();
    let started = Instant::now();
    tracing::info!("fs_export begin image={image_id} node={node_for_log} dest={dest_for_log}");
    let image_for_persist = Arc::clone(&image);
    let result = tokio::task::spawn_blocking(move || -> Result<FsExportResult, AppError> {
        fs_tree::export_file_with_fault_report(&image, &node_id, &PathBuf::from(destination))
            .map_err(AppError::from_anyhow)
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_export", err))?;
    if let Err(error) = image_read_fault_report(&state, &image_id, &image_for_persist).await {
        tracing::warn!(
            "fs_export bad-region audit persistence failed: {}",
            error.message
        );
    }
    match &result {
        Ok(summary) => tracing::info!(
            "fs_export done image={image_id} node={node_for_log} dest={dest_for_log} elapsed={:.3?} bytes={} read_faults={} synthetic_bytes={}",
            started.elapsed(),
            summary.bytes_written,
            summary.read_faults.read_faults_detected,
            summary.read_faults.synthetic_bytes,
        ),
        Err(err) => tracing::warn!(
            "fs_export err image={image_id} node={node_for_log} dest={dest_for_log} elapsed={:.3?} err={}",
            started.elapsed(),
            err.message,
        ),
    }
    result
}

#[derive(Debug, Clone, Serialize)]
pub struct FsExportDirProgress {
    pub task_id: String,
    pub files_done: u64,
    pub dirs_done: u64,
    pub bytes_written: u64,
    pub skipped: u64,
    pub skipped_deleted: u64,
    pub current_path: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportDirMode {
    /// Reproduce the source directory tree under `destination` as nested
    /// real filesystem directories.
    Tree,
    /// Stream the source directory tree into a single Deflate-compressed
    /// `.zip` archive at `destination`.
    Zip,
}

const EXPORT_DIR_PROGRESS_THROTTLE: Duration = Duration::from_millis(150);

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_export_dir(
    app: AppHandle,
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    destination: String,
    task_id: String,
    mode: ExportDirMode,
) -> Result<FsExportSummary, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let node_for_log = node_id.clone();
    let dest_for_log = destination.clone();
    let mode_for_log = format!("{mode:?}");
    let started = Instant::now();
    tracing::info!(
        "fs_export_dir begin image={image_id} node={node_for_log} mode={mode_for_log} dest={dest_for_log} task={task_id}"
    );

    let task = state.tasks.register(
        task_id.clone(),
        TaskKind::DirectoryExport,
        Some(image_id.clone()),
    );
    task.update(|view| view.subject = vec![destination.clone()]);
    let resource_permit = state.tasks.acquire_resources(&task).await?;
    let token = task.token();

    let token_for_walk = token.clone();
    let app_for_walk = app.clone();
    let task_id_for_walk = task_id.clone();
    let tasks_for_walk = state.tasks.clone();
    let image_for_persist = Arc::clone(&image);

    let result = tokio::task::spawn_blocking(move || -> Result<FsExportSummary, AppError> {
        let _resource_permit = resource_permit;
        // First per-entry tick may wait up to PROGRESS_THROTTLE; the
        // `current.is_none()` force-flush at start (in `export_dir`) makes
        // sure the toast doesn't sit at "准备中" while we wait for the
        // throttle to elapse.
        let last_emit: Mutex<Instant> = Mutex::new(Instant::now());
        let on_progress = |summary: &FsExportSummary, current: Option<&str>| {
            // Throttle: emit at most once per 150 ms unless the caller
            // passes None (used as a "force flush" at start/end).
            let now = Instant::now();
            let should_emit = if current.is_none() {
                true
            } else if let Ok(mut last) = last_emit.lock() {
                if now.duration_since(*last) >= EXPORT_DIR_PROGRESS_THROTTLE {
                    *last = now;
                    true
                } else {
                    false
                }
            } else {
                false
            };
            if !should_emit {
                return;
            }
            if let Err(error) = app_for_walk.emit(
                "fs-export-dir-progress",
                FsExportDirProgress {
                    task_id: task_id_for_walk.clone(),
                    files_done: summary.file_count,
                    dirs_done: summary.dir_count,
                    bytes_written: summary.bytes_written,
                    skipped: summary.skipped_count,
                    skipped_deleted: summary.skipped_deleted,
                    current_path: current.map(String::from),
                },
            ) {
                tracing::debug!("emit fs-export-dir-progress failed: {error}");
            }
            tasks_for_walk.update(&task_id_for_walk, |view| {
                view.counts.insert(TaskCount::Files, summary.file_count);
            });
            tasks_for_walk.emit_progress(
                &task_id_for_walk,
                TaskProgressUnit::Bytes,
                summary.bytes_written,
                None,
                current.map(String::from),
            );
        };
        match mode {
            ExportDirMode::Tree => fs_tree::export_dir_with_fault_report(
                &image,
                &node_id,
                &PathBuf::from(destination),
                &token_for_walk,
                &on_progress,
            )
            .map_err(AppError::from_anyhow),
            ExportDirMode::Zip => fs_tree::export_dir_zip_with_fault_report(
                &image,
                &node_id,
                &PathBuf::from(destination),
                &token_for_walk,
                &on_progress,
            )
            .map_err(AppError::from_anyhow),
        }
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_export_dir", err))?;
    if let Err(error) = image_read_fault_report(&state, &image_id, &image_for_persist).await {
        tracing::warn!(
            "fs_export_dir bad-region audit persistence failed: {}",
            error.message
        );
    }

    match &result {
        Ok(summary) => tracing::info!(
            "fs_export_dir done image={image_id} node={node_for_log} mode={mode_for_log} dest={dest_for_log} elapsed={:.3?} files={} dirs={} bytes={} skipped={} deleted={} truncated={} cancelled={} read_faults={} synthetic_bytes={}",
            started.elapsed(),
            summary.file_count,
            summary.dir_count,
            summary.bytes_written,
            summary.skipped_count,
            summary.skipped_deleted,
            summary.truncated,
            summary.cancelled,
            summary.read_faults.read_faults_detected,
            summary.read_faults.synthetic_bytes,
        ),
        Err(err) => tracing::warn!(
            "fs_export_dir err image={image_id} node={node_for_log} mode={mode_for_log} dest={dest_for_log} elapsed={:.3?} err={}",
            started.elapsed(),
            err.message,
        ),
    }
    match &result {
        Ok(summary) => {
            task.update(|view| {
                view.counts.insert(TaskCount::Files, summary.file_count);
                if summary.truncated {
                    view.flags.insert(TaskFlag::Truncated);
                }
                crate::app::workflows::task_center::count_read_faults(view, &summary.read_faults);
            });
            task.finish(if summary.cancelled {
                TaskOutcome::Cancelled
            } else {
                TaskOutcome::Succeeded
            });
        }
        Err(_) => task.finish_with(&result),
    }
    result
}

/// Exports a TSK-style mactime body file for the whole image or one subtree.
/// Read-only, streaming: the walk writes one pipe-delimited line per node
/// directly to `destination` so a multi-hundred-thousand-entry tree never
/// materialises in memory.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_timeline_export(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    destination: String,
) -> Result<crate::core::timeline::mactime::MactimeSummary, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let node_for_log = node_id.clone();
    let dest_for_log = destination.clone();
    let started = Instant::now();
    tracing::info!(
        "fs_timeline_export begin image={image_id} node={node_for_log} dest={dest_for_log}"
    );
    let result = tokio::task::spawn_blocking(
        move || -> Result<crate::core::timeline::mactime::MactimeSummary, AppError> {
            let file = std::fs::File::create(&destination)
                .map_err(|err| AppError::from_io(format!("create body file {destination}"), err))?;
            let mut writer = std::io::BufWriter::new(file);
            // An unscoped (whole-image) export passes the empty start id through.
            let start = if node_id.trim().is_empty() {
                ""
            } else {
                node_id.as_str()
            };
            let cancel = CancellationToken::new();
            let summary =
                crate::core::timeline::mactime::export_body_file(&image, start, &cancel, |line| {
                    writer
                        .write_all(line.as_bytes())
                        .and_then(|_| writer.write_all(b"\n"))
                        .map_err(|err| anyhow::anyhow!("write body file: {err}"))?;
                    Ok((line.len() + 1) as u64)
                })
                .map_err(AppError::from_anyhow)?;
            writer
                .flush()
                .map_err(|err| AppError::from_io("flush body file", err))?;
            Ok(summary)
        },
    )
    .await
    .map_err(|err| AppError::task_panicked("fs_timeline_export", err))?;
    match &result {
        Ok(summary) => tracing::info!(
            "fs_timeline_export done image={image_id} node={node_for_log} dest={dest_for_log} elapsed={:.3?} lines={} bytes={}",
            started.elapsed(),
            summary.lines_written,
            summary.bytes_written,
        ),
        Err(err) => tracing::warn!(
            "fs_timeline_export err image={image_id} node={node_for_log} dest={dest_for_log} elapsed={:.3?} err={}",
            started.elapsed(),
            err.message,
        ),
    }
    result
}
