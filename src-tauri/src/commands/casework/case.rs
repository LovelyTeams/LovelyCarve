//! Case lifecycle IPC.
//!
//! | 命令 | 语义 |
//! |---|---|
//! | `case_create` | 建案件目录与台账 `case.db`,加进程锁 |
//! | `case_open` | 打开案件(进程锁 + 自愈)并登记到本地案件列表 |
//! | `case_close` | 记审计后释放台账连接与进程锁,卸载运行态镜像 |
//! | `case_lock` / `case_unlock` | 翻转逻辑锁 |
//! | `case_remove` | 删除案件自己的文件并注销登记 |
//! | `case_list` | 本地登记簿 + 台账档案摘要 |
//! | `case_rename` | 改名并同步登记簿 |
//! | `case_option_query` / `case_option_save` | 台账 `setting` 表的案件级 KV |
//!
//! Snapshot orchestration and the `snapshot_*` commands live in
//! `commands::casework::case_snapshot`.

use crate::host::AppHandle;
use crate::host::State;

use crate::app::error::AppError;
use crate::core::casework::case::CaseInfo;
use crate::core::casework::case::EvidenceRef;
use crate::state::AppState;

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn case_create(
    app: AppHandle,
    state: State<'_, AppState>,
    dir: String,
    name: String,
    case_number: Option<String>,
    description: Option<String>,
    examiner: Option<String>,
) -> Result<CaseInfo, AppError> {
    crate::app::workflows::casework::case_create(
        app,
        state,
        dir,
        name,
        case_number,
        description,
        examiner,
    )
    .await
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn case_open(
    app: AppHandle,
    state: State<'_, AppState>,
    dir: String,
) -> Result<CaseInfo, AppError> {
    crate::app::workflows::casework::case_open(app, state, dir).await
}

/// Exit the active case: journal the close, release the ledger connection and
/// the process lock, and unload the case's runtime images.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn case_close(state: State<'_, AppState>) -> Result<(), AppError> {
    crate::app::workflows::casework::case_close(state).await
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn case_active(state: State<'_, AppState>) -> Result<Option<CaseInfo>, AppError> {
    crate::app::workflows::casework::case_active(state).await
}

/// Locally registered cases, most recently opened first, each with its ledger
/// profile.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn case_list(app: AppHandle) -> Result<Vec<CaseListItem>, AppError> {
    crate::app::workflows::casework::case_list(app).await
}

/// Removes a case: closes it first when it is the active one, deletes only the
/// case's own files (ledger, lock, evidence and staging directories) and drops
/// the registry entry. A directory that no longer exists is only
/// unregistered; a directory without a case ledger is refused — arbitrary
/// directories are never deleted.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn case_remove(
    app: AppHandle,
    state: State<'_, AppState>,
    dir: String,
) -> Result<(), AppError> {
    crate::app::workflows::casework::case_remove(app, state, dir).await
}

/// Lock the active case (logical lock).
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn case_lock(state: State<'_, AppState>) -> Result<CaseInfo, AppError> {
    crate::app::workflows::casework::case_lock(state).await
}

/// Unlock the active case.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn case_unlock(state: State<'_, AppState>) -> Result<CaseInfo, AppError> {
    crate::app::workflows::casework::case_unlock(state).await
}

/// Rename the active case and its registry entry. Locked cases refuse edits.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn case_rename(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
) -> Result<CaseInfo, AppError> {
    crate::app::workflows::casework::case_rename(app, state, name).await
}

/// Case-level settings; an empty `prefix` returns all of them.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn case_option_query(
    state: State<'_, AppState>,
    prefix: Option<String>,
) -> Result<Vec<CaseOption>, AppError> {
    crate::app::workflows::casework::case_option_query(state, prefix).await
}

/// Upsert case-level settings.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn case_option_save(
    state: State<'_, AppState>,
    entries: Vec<CaseOption>,
) -> Result<(), AppError> {
    crate::app::workflows::casework::case_option_save(state, entries).await
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn case_evidence_list(state: State<'_, AppState>) -> Result<Vec<EvidenceRef>, AppError> {
    crate::app::workflows::casework::case_evidence_list(state).await
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn case_evidence_remove(
    state: State<'_, AppState>,
    image_id: String,
) -> Result<CaseInfo, AppError> {
    crate::app::workflows::casework::case_evidence_remove(state, image_id).await
}

pub use crate::app::workflows::casework::{CaseListItem, CaseOption};
