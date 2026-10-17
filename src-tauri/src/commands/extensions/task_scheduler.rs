//! 插件任务调度 IPC。
//!
//! | 命令 | 用途 |
//! |---|---|
//! | `sched_task_functions` | 列出可调度功能 |
//! | `sched_task_list` | 查询全部任务或指定检材的任务 |
//! | `sched_task_create` | 创建任务 |
//! | `sched_task_cancel` / `pause` / `start` / `restart` / `remove` | 控制任务生命周期 |
//! | `sched_evidence_post_actions_set` | 检材收尾动作写入 |
//!
//! 事件:`sched-task-event`(载荷 [`SchedEvent`]:任务变更 / 删除 / 通知)。
//!
//! 读写台账的同步调度器方法都放到阻塞线程池执行([`on_blocking`]),
//! 插件任务密集提交时不会占住异步运行时。

use crate::host::State;
use serde::Serialize;

use crate::{
    commands::app_error::AppError,
    core::extensions::task_scheduler::{
        InsertOutcome, SchedError, SchedFunction, SchedResult, SchedTaskRecord, TaskParams,
        TaskScheduler,
    },
    state::AppState,
};

/// `sched-task-event` 事件名。
pub use crate::core::extensions::task_scheduler::SCHED_TASK_EVENT;

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

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn sched_task_list(
    state: State<'_, AppState>,
    image_id: Option<String>,
    include_internal: Option<bool>,
) -> Result<Vec<SchedTaskRecord>, AppError> {
    on_blocking(&state, "sched_task_list", move |scheduler| {
        scheduler.list_for_image(image_id.as_deref(), include_internal.unwrap_or(false))
    })
    .await
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn sched_task_create(
    state: State<'_, AppState>,
    function: String,
    image_id: String,
    node_id: Option<String>,
    file_name: Option<String>,
    platform: Option<String>,
    sub_items: Option<Vec<String>>,
) -> Result<SchedTaskCreateResponse, AppError> {
    let params = TaskParams {
        node_id,
        file_name,
        platform,
        sub_items: sub_items.unwrap_or_default(),
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
    state
        .scheduler
        .cancel(task_id)
        .await
        .map_err(sched_app_error)
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn sched_task_pause(
    state: State<'_, AppState>,
    task_id: i64,
) -> Result<SchedTaskRecord, AppError> {
    state
        .scheduler
        .pause(task_id)
        .await
        .map_err(sched_app_error)
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn sched_task_start(
    state: State<'_, AppState>,
    task_id: i64,
) -> Result<SchedTaskRecord, AppError> {
    state
        .scheduler
        .start(task_id)
        .await
        .map_err(sched_app_error)
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn sched_task_restart(
    state: State<'_, AppState>,
    task_id: i64,
) -> Result<SchedTaskRecord, AppError> {
    state
        .scheduler
        .restart(task_id)
        .await
        .map_err(sched_app_error)
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn sched_task_remove(
    state: State<'_, AppState>,
    task_id: i64,
) -> Result<SchedTaskRecord, AppError> {
    state
        .scheduler
        .remove(task_id)
        .await
        .map_err(sched_app_error)
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
    tokio::task::spawn_blocking(move || call(&scheduler))
        .await
        .map_err(|error| AppError::task_panicked(command, error))?
        .map_err(sched_app_error)
}

fn sched_app_error(error: SchedError) -> AppError {
    match error {
        SchedError::NoCase | SchedError::TaskNotFound(_) => AppError::not_found(error.to_string()),
        SchedError::InvalidState { .. }
        | SchedError::FunctionUnknown(_)
        | SchedError::EvidenceUnknown(_) => AppError::invalid_input(error.to_string()),
        SchedError::Store(inner) => AppError::from_anyhow(inner),
    }
}
