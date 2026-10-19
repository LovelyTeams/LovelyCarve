//! Cross-cutting IPC: the task center (every task, plugin jobs included),
//! task cancellation, module catalogue.

use std::path::PathBuf;

use crate::host::State;
use serde::Serialize;
use serde_json::json;

use crate::app::error::AppError;
use crate::app::module_catalog::{self as module_summary, ModuleSummary};
use crate::app::workflows::task_center::{self, TaskEntry};
use crate::state::AppState;

/// Cancels a task: a native task id, or `plugin:<job id>`.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn task_cancel(state: State<'_, AppState>, task_id: String) -> Result<(), AppError> {
    task_center::cancel(&state, &task_id).await
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn task_status(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Option<TaskEntry>, AppError> {
    Ok(task_center::list(&state, None)
        .await?
        .into_iter()
        .find(|entry| entry.id == task_id))
}

/// The task center's tasks of `image_id` (of every evidence when absent);
/// tasks of no evidence are always included. Changes follow on the
/// `task-entry` event.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn task_list(
    state: State<'_, AppState>,
    image_id: Option<String>,
) -> Result<Vec<TaskEntry>, AppError> {
    task_center::list(&state, image_id.as_deref()).await
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn module_summary() -> Result<Vec<ModuleSummary>, AppError> {
    Ok(module_summary::summary())
}

#[derive(Debug, Serialize)]
pub struct PathInfo {
    pub exists: bool,
    pub is_file: bool,
    pub is_directory: bool,
    pub is_symlink: bool,
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn system_path_info(path: String) -> Result<PathInfo, AppError> {
    let worker_path = path.clone();
    tokio::task::spawn_blocking(move || match std::fs::symlink_metadata(&worker_path) {
        Ok(metadata) => {
            let file_type = metadata.file_type();
            Ok(PathInfo {
                exists: true,
                is_file: file_type.is_file(),
                is_directory: file_type.is_dir(),
                is_symlink: file_type.is_symlink(),
            })
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(PathInfo {
            exists: false,
            is_file: false,
            is_directory: false,
            is_symlink: false,
        }),
        Err(err) => Err(AppError::from_io(format!("stat {worker_path}"), err)
            .with_context_value(json!({ "path": worker_path }))),
    })
    .await
    .map_err(|error| AppError::task_panicked("system_path_info", error))?
}

/// Opens the OS file manager at `path`. When `select` is true the entry is
/// highlighted inside its parent (Explorer `/select`, Finder `-R`); when
/// false, directories open in place. Used by the FsTree export-complete
/// dialog to jump to the destination.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn reveal_path(path: String, select: bool) -> Result<(), AppError> {
    let target = PathBuf::from(&path);
    if !target.exists() {
        return Err(AppError::not_found(format!("路径不存在：{path}"))
            .with_context_value(json!({ "path": path })));
    }
    spawn_reveal(&target, select)
        .map_err(|error| AppError::from_io(format!("reveal {}", target.display()), error))
}

#[cfg(target_os = "windows")]
fn spawn_reveal(target: &std::path::Path, select: bool) -> std::io::Result<()> {
    use std::process::Command;
    let arg = if select {
        format!("/select,{}", target.display())
    } else {
        target.display().to_string()
    };
    Command::new("explorer.exe").arg(arg).spawn()?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn spawn_reveal(target: &std::path::Path, select: bool) -> std::io::Result<()> {
    use std::process::Command;
    let mut cmd = Command::new("open");
    if select {
        cmd.arg("-R");
    }
    cmd.arg(target).spawn()?;
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn spawn_reveal(target: &std::path::Path, select: bool) -> std::io::Result<()> {
    use std::process::Command;
    let opened = if select && target.is_file() {
        target.parent().unwrap_or(target)
    } else {
        target
    };
    Command::new("xdg-open").arg(opened).spawn()?;
    Ok(())
}
