//! 插件任务调度 IPC。
//!
//! | 命令 | 用途 |
//! |---|---|
//! | `sched_task_functions` | 列出可调度功能 |
//! | `sched_systems_list` | 检材上识别出的操作系统(系统作用域任务的目标) |
//! | `sched_task_create` | 创建任务 |
//! | `sched_task_cancel` / `start` / `restart` / `remove` | 控制任务生命周期 |
//! | `sched_evidence_post_actions_set` | 检材收尾动作写入 |
//!
//! 插件任务与内置任务一起由任务中心列出(`task_list`,
//! [`crate::app::workflows::task_center`]),变更走统一的 `task-entry` 事件。
//!
//! 读写台账的同步调度器方法都放到阻塞线程池执行([`on_blocking`]),
//! 插件任务密集提交时不会占住异步运行时。

use crate::host::State;
use serde::Serialize;

use crate::{
    app::access::loaded_image_app_error,
    commands::app_error::AppError,
    core::{
        analysis::identify::systems::DetectedSystem,
        extensions::task_scheduler::{
            InsertOutcome, SchedFunction, SchedResult, SchedTaskRecord, TaskParams, TaskScheduler,
        },
    },
    state::AppState,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedTaskCreateResponse {
    pub task: SchedTaskRecord,
    /// false 表示命中单实例约束,返回的是已存在的任务。
    pub created: bool,
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn sched_task_functions(
    state: State<'_, AppState>,
) -> Result<Vec<SchedFunction>, AppError> {
    Ok(state.scheduler.registry().all().to_vec())
}

/// The operating systems detected on a loaded image, each with the volumes
/// it mounts. Detection reads the evidence once per image.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn sched_systems_list(
    state: State<'_, AppState>,
    image_id: String,
) -> Result<Vec<DetectedSystem>, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let systems = std::sync::Arc::clone(&state.systems);
    tokio::task::spawn_blocking(move || systems.get_or_discover(&image).as_ref().clone())
        .await
        .map_err(|error| AppError::task_panicked("sched_systems_list", error))
}

/// Creates a task: `systemId` for a system-scope analyzer, `nodeId` (and
/// `fileName`) for a file-scope one; `options` by the analyzer's option ids.
#[cfg_attr(feature = "desktop", tauri::command)]
#[allow(clippy::too_many_arguments)] // Flat IPC fields also generate the CLI options.
pub async fn sched_task_create(
    state: State<'_, AppState>,
    function: String,
    image_id: String,
    node_id: Option<String>,
    file_name: Option<String>,
    system_id: Option<String>,
    options: Option<serde_json::Value>,
) -> Result<SchedTaskCreateResponse, AppError> {
    let options = match options {
        None | Some(serde_json::Value::Null) => serde_json::Map::new(),
        Some(serde_json::Value::Object(options)) => options,
        Some(_) => return Err(AppError::invalid_input("options 必须是对象")),
    };
    let params = TaskParams {
        node_id,
        file_name,
        system_id,
        target_label: None,
        options,
    };
    let outcome = on_blocking(&state, "sched_task_create", move |scheduler| {
        scheduler.create_task(&function, &image_id, params)
    })
    .await?;
    let created = matches!(outcome, InsertOutcome::Created(_));
    Ok(SchedTaskCreateResponse {
        task: outcome.into_record(),
        created,
    })
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn sched_task_cancel(
    state: State<'_, AppState>,
    task_id: i64,
) -> Result<SchedTaskRecord, AppError> {
    Ok(state.scheduler.cancel(task_id).await?)
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn sched_task_start(
    state: State<'_, AppState>,
    task_id: i64,
) -> Result<SchedTaskRecord, AppError> {
    Ok(state.scheduler.start(task_id).await?)
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn sched_task_restart(
    state: State<'_, AppState>,
    task_id: i64,
) -> Result<SchedTaskRecord, AppError> {
    Ok(state.scheduler.restart(task_id).await?)
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn sched_task_remove(
    state: State<'_, AppState>,
    task_id: i64,
) -> Result<SchedTaskRecord, AppError> {
    Ok(state.scheduler.remove(task_id).await?)
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn sched_evidence_post_actions_set(
    state: State<'_, AppState>,
    image_id: String,
    actions_json: Option<String>,
) -> Result<(), AppError> {
    on_blocking(
        &state,
        "sched_evidence_post_actions_set",
        move |scheduler| scheduler.set_evidence_post_actions(&image_id, actions_json.as_deref()),
    )
    .await
}

/// Runs a synchronous scheduler call, which reads or writes the case
/// ledger, on the blocking pool.
async fn on_blocking<T: Send + 'static>(
    state: &State<'_, AppState>,
    command: &'static str,
    call: impl FnOnce(&TaskScheduler) -> SchedResult<T> + Send + 'static,
) -> Result<T, AppError> {
    let scheduler = state.scheduler.clone();
    Ok(tokio::task::spawn_blocking(move || call(&scheduler))
        .await
        .map_err(|error| AppError::task_panicked(command, error))??)
}
