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
//! `app::workflows::snapshot::case_snapshot`.

use std::path::PathBuf;

use crate::host::{AppHandle, State};
use serde::{Deserialize, Serialize};

use crate::app::error::{AppError, ErrorCode};
use crate::app::workflows::snapshot::case_snapshot::snapshot_already_loaded_images;
use crate::core::casework::case::{self, CaseInfo, CaseState, CreateCaseOptions, EvidenceRef};
use crate::core::{casework::case_ledger, casework::case_paths, casework::case_registry};
use crate::state::AppState;

/// One entry of the local case list: registry record enriched with the case
/// ledger profile. `missing` is set when the directory or its ledger can no
/// longer be read (the entry stays listed so it can be removed).
#[derive(Debug, Clone, Serialize)]
pub struct CaseListItem {
    pub dir: String,
    /// Case uid; empty when the ledger cannot be read.
    pub id: String,
    pub case_number: String,
    pub name: String,
    pub examiner: String,
    pub locked: bool,
    pub created_at: String,
    pub updated_at: String,
    pub opened_at: String,
    pub missing: bool,
}

/// One case-level setting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseOption {
    pub key: String,
    pub value: String,
}

fn registry_dir(app: &AppHandle) -> Result<PathBuf, AppError> {
    app.app_data_dir()
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| AppError::new(ErrorCode::Internal, "resolve app data dir: unavailable"))
}

async fn active_case_info(
    state: &State<'_, AppState>,
    context: &str,
) -> Result<CaseInfo, AppError> {
    let guard = state.active_case.lock().await;
    guard.as_ref().map(|case| case.info()).ok_or_else(|| {
        AppError::new(
            ErrorCode::Internal,
            format!("case vanished after {context}"),
        )
    })
}

pub(crate) async fn active_case_dir(state: &State<'_, AppState>) -> Result<PathBuf, AppError> {
    let guard = state.active_case.lock().await;
    guard
        .as_ref()
        .map(|case| case.dir.clone())
        .ok_or_else(|| AppError::invalid_input("no active case"))
}

pub async fn case_create(
    app: AppHandle,
    state: State<'_, AppState>,
    dir: String,
    name: String,
    case_number: Option<String>,
    description: Option<String>,
    examiner: Option<String>,
) -> Result<CaseInfo, AppError> {
    let path = PathBuf::from(&dir);
    let options = CreateCaseOptions {
        name,
        case_number,
        description,
        examiner,
    };
    // Switching away from an already-open case: exit it (unlock) and retire
    // its runtime images BEFORE creating the new one, so
    // `snapshot_already_loaded_images` below doesn't fold the previous case's
    // evidence into the new one. When NO case was active, the loaded images
    // are caseless and folding them in is intended — so the exit is gated on
    // the switch.
    exit_active_case_on_switch(&state).await;
    let case = tokio::task::spawn_blocking(move || case::create_case(&path, options))
        .await
        .map_err(|err| AppError::task_panicked("case_create", err))?
        .map_err(AppError::from)?;
    activate_case(&app, &state, &dir, case).await?;
    active_case_info(&state, "create").await
}

pub async fn case_open(
    app: AppHandle,
    state: State<'_, AppState>,
    dir: String,
) -> Result<CaseInfo, AppError> {
    let path = PathBuf::from(&dir);
    // Switching cases: exit the previous case first so its process lock is
    // released and its runtime images don't fold into the newly opened case.
    // No-op when no case was active — caseless images SHOULD fold in.
    exit_active_case_on_switch(&state).await;
    let case = tokio::task::spawn_blocking(move || case::open_case(&path))
        .await
        .map_err(|err| AppError::task_panicked("case_open", err))?
        .map_err(AppError::from)?;
    activate_case(&app, &state, &dir, case).await?;
    active_case_info(&state, "open").await
}

/// Makes `case` the active case: registry entry, task scheduler binding, and
/// every already-loaded image folded in as evidence.
async fn activate_case(
    app: &AppHandle,
    state: &State<'_, AppState>,
    dir: &str,
    case: CaseState,
) -> Result<(), AppError> {
    let mut load_context = state.image_load_context.lock().await;
    *load_context = load_context.wrapping_add(1);
    state
        .tasks
        .cancel_kind(crate::task_manager::TaskKind::ImageLoad);

    case_registry::record(
        &registry_dir(app)?,
        dir,
        &case.profile().case_number,
        &case.profile().name,
    );
    let ledger_path = case.ledger_path();
    let case_dir = case.dir.clone();
    {
        let mut guard = state.active_case.lock().await;
        *guard = Some(case);
    }
    // 任务调度绑定台账任务表(崩溃恢复、启动调度循环)。失败只记日志:任务
    // 调度不可用不应阻止案件打开。
    if let Err(err) = state.scheduler.bind_case(&ledger_path, case_dir).await {
        tracing::warn!("case: bind task scheduler failed: {err:#}");
    }
    clear_removed_case_evidence(state).await;
    // Folds every already-loaded image in as evidence (writes the ledger rows
    // before spawning background snapshot builds), so callers read `info()`
    // AFTER it to report the freshly-registered evidence.
    snapshot_already_loaded_images(app, state).await;
    Ok(())
}

/// Exit the active case: journal the close, release the ledger connection and
/// the process lock, and unload the case's runtime images.
pub async fn case_close(state: State<'_, AppState>) -> Result<(), AppError> {
    exit_active_case(&state).await;
    Ok(())
}

async fn exit_active_case(state: &State<'_, AppState>) {
    let mut load_context = state.image_load_context.lock().await;
    *load_context = load_context.wrapping_add(1);
    state
        .tasks
        .cancel_kind(crate::task_manager::TaskKind::ImageLoad);

    // 任务调度先解绑:运行中任务置 offline、终止插件 worker,再释放案件连接。
    state.scheduler.unbind_case().await;
    let closing = {
        let mut guard = state.active_case.lock().await;
        guard.take()
    };
    if let Some(case) = closing {
        if let Err(err) = tokio::task::spawn_blocking(move || case.close()).await {
            tracing::warn!("case: close task crashed: {err}");
        }
    }
    clear_removed_case_evidence(state).await;
    // Drop the case's runtime images too: a case's evidence is its working
    // set, so closing it must not leave those images loaded for the next case
    // to absorb (via `snapshot_already_loaded_images`) or for a stale tab to
    // re-register. Done after clearing `active_case` so the unload runs in the
    // caseless state and can't re-touch a case.
    unload_all_images(state).await;
}

/// Exit the previous case on a case *switch* — only when a case is currently
/// active. With no case open, the loaded images are caseless and are meant to
/// fold into the case being opened, so this is a no-op then.
async fn exit_active_case_on_switch(state: &State<'_, AppState>) {
    let switching = { state.active_case.lock().await.is_some() };
    if switching {
        exit_active_case(state).await;
    }
}

async fn clear_removed_case_evidence(state: &State<'_, AppState>) {
    let mut removed = state.removed_case_evidence.lock().await;
    removed.clear();
}

/// Unload every runtime image and its derived caches. Mirrors what
/// `image_unload` does per image — Dokan mounts, preview/media caches, and
/// in-flight session-snapshot builds all get torn down.
async fn unload_all_images(state: &State<'_, AppState>) {
    let ids: Vec<String> = {
        let images = state.images.lock().await;
        images.keys().cloned().collect()
    };
    for image_id in ids {
        if let Err(err) =
            crate::app::workflows::evidence::remove_image_runtime(state, &image_id).await
        {
            tracing::warn!(
                "case switch: unload image {image_id} failed: {}",
                err.message
            );
        }
    }
}

pub async fn case_active(state: State<'_, AppState>) -> Result<Option<CaseInfo>, AppError> {
    let guard = state.active_case.lock().await;
    Ok(guard.as_ref().map(|case| case.info()))
}

/// Locally registered cases, most recently opened first, each with its ledger
/// profile.
pub async fn case_list(app: AppHandle) -> Result<Vec<CaseListItem>, AppError> {
    let registry = registry_dir(&app)?;
    tokio::task::spawn_blocking(move || {
        case_registry::list(&registry)
            .into_iter()
            .map(|entry| {
                let dir = PathBuf::from(&entry.dir);
                match case::peek_case(&dir) {
                    Ok(profile) => CaseListItem {
                        dir: entry.dir,
                        id: profile.case_uid,
                        case_number: profile.case_number,
                        name: profile.name,
                        examiner: profile.examiner,
                        locked: profile.locked,
                        created_at: profile.created_at,
                        updated_at: profile.updated_at,
                        opened_at: entry.opened_at,
                        missing: false,
                    },
                    Err(err) => {
                        tracing::debug!("case list: {} unreadable: {err:#}", entry.dir);
                        CaseListItem {
                            dir: entry.dir,
                            id: String::new(),
                            case_number: entry.case_number,
                            name: entry.name,
                            examiner: String::new(),
                            locked: false,
                            created_at: entry.created_at,
                            updated_at: String::new(),
                            opened_at: entry.opened_at,
                            missing: true,
                        }
                    }
                }
            })
            .collect()
    })
    .await
    .map_err(|err| AppError::task_panicked("case_list", err))
}

/// Removes a case: closes it first when it is the active one, deletes only the
/// case's own files (ledger, lock, evidence and staging directories) and drops
/// the registry entry. A directory that no longer exists is only
/// unregistered; a directory without a case ledger is refused — arbitrary
/// directories are never deleted.
pub async fn case_remove(
    app: AppHandle,
    state: State<'_, AppState>,
    dir: String,
) -> Result<(), AppError> {
    let path = PathBuf::from(&dir);
    let is_active = {
        let guard = state.active_case.lock().await;
        guard.as_ref().is_some_and(|case| case.dir == path)
    };
    if is_active {
        exit_active_case(&state).await;
    }
    if path.exists() {
        let remove_path = path.clone();
        tokio::task::spawn_blocking(move || case::remove_case(&remove_path))
            .await
            .map_err(|err| AppError::task_panicked("case_remove", err))?
            .map_err(AppError::from)?;
    }
    case_registry::remove(&registry_dir(&app)?, &dir);
    Ok(())
}

/// Lock the active case (logical lock).
pub async fn case_lock(state: State<'_, AppState>) -> Result<CaseInfo, AppError> {
    set_active_case_locked(&state, true).await
}

/// Unlock the active case.
pub async fn case_unlock(state: State<'_, AppState>) -> Result<CaseInfo, AppError> {
    set_active_case_locked(&state, false).await
}

async fn set_active_case_locked(
    state: &State<'_, AppState>,
    locked: bool,
) -> Result<CaseInfo, AppError> {
    let mut guard = state.active_case.lock().await;
    let Some(case) = guard.as_mut() else {
        return Err(AppError::invalid_input("no active case"));
    };
    case.set_locked(locked).map_err(AppError::from)?;
    Ok(case.info())
}

/// Rename the active case and its registry entry. Locked cases refuse edits.
pub async fn case_rename(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
) -> Result<CaseInfo, AppError> {
    let (info, dir) = {
        let mut guard = state.active_case.lock().await;
        let Some(case) = guard.as_mut() else {
            return Err(AppError::invalid_input("no active case"));
        };
        if case.is_locked() {
            return Err(AppError::invalid_input(
                "case is locked; unlock it before editing",
            ));
        }
        case.rename(&name).map_err(AppError::from)?;
        (case.info(), case.dir.display().to_string())
    };
    case_registry::rename(&registry_dir(&app)?, &dir, &info.name);
    Ok(info)
}

/// Case-level settings; an empty `prefix` returns all of them.
pub async fn case_option_query(
    state: State<'_, AppState>,
    prefix: Option<String>,
) -> Result<Vec<CaseOption>, AppError> {
    let dir = active_case_dir(&state).await?;
    let prefix = prefix.unwrap_or_default();
    tokio::task::spawn_blocking(move || {
        let ledger = case_ledger::open(&case_paths::ledger_path(&dir))?;
        case_ledger::settings_query(&ledger, &prefix)
    })
    .await
    .map_err(|err| AppError::task_panicked("case_option_query", err))?
    .map(|entries| {
        entries
            .into_iter()
            .map(|(key, value)| CaseOption { key, value })
            .collect()
    })
    .map_err(AppError::from)
}

/// Upsert case-level settings.
pub async fn case_option_save(
    state: State<'_, AppState>,
    entries: Vec<CaseOption>,
) -> Result<(), AppError> {
    let dir = active_case_dir(&state).await?;
    let pairs: Vec<(String, String)> = entries
        .into_iter()
        .map(|entry| (entry.key, entry.value))
        .collect();
    tokio::task::spawn_blocking(move || {
        let ledger = case_ledger::open(&case_paths::ledger_path(&dir))?;
        case_ledger::settings_save(&ledger, &pairs)
    })
    .await
    .map_err(|err| AppError::task_panicked("case_option_save", err))?
    .map_err(AppError::from)
}

pub async fn case_evidence_list(state: State<'_, AppState>) -> Result<Vec<EvidenceRef>, AppError> {
    let guard = state.active_case.lock().await;
    Ok(guard
        .as_ref()
        .map(|case| case.evidence_refs())
        .unwrap_or_default())
}

pub async fn case_evidence_remove(
    state: State<'_, AppState>,
    image_id: String,
) -> Result<CaseInfo, AppError> {
    {
        let mut removed = state.removed_case_evidence.lock().await;
        removed.insert(image_id.clone());
    }
    let refusal = {
        let guard = state.active_case.lock().await;
        match guard.as_ref() {
            None => Some(AppError::invalid_input("no active case")),
            Some(case) if case.is_locked() => Some(AppError::invalid_input(
                "case is locked; unlock it before removing evidence",
            )),
            Some(_) => None,
        }
    };
    if let Some(refusal) = refusal {
        let mut removed = state.removed_case_evidence.lock().await;
        removed.remove(&image_id);
        return Err(refusal);
    }
    // 检材下线:先取消它的全部未终结任务(调度器持独立台账连接,不与案件锁
    // 互斥),再删检材行——台账级联带走它的任务、卷记录与标签。
    let withdrawal = match state.scheduler.withdraw_evidence(&image_id).await {
        Ok(withdrawal) => {
            if withdrawal.cancelled > 0 {
                tracing::info!(
                    "case: evidence {image_id} offline, {} tasks cancelled",
                    withdrawal.cancelled
                );
            }
            Some(withdrawal)
        }
        Err(err) => {
            tracing::warn!("case: cancel tasks for evidence {image_id}: {err}");
            None
        }
    };
    let (info, cleanup) = {
        let mut guard = state.active_case.lock().await;
        let Some(case) = guard.as_mut() else {
            return Err(AppError::invalid_input("no active case"));
        };
        let cleanup = case.detach_evidence(&image_id).map_err(AppError::from)?;
        (case.info(), cleanup)
    };
    if let Some(withdrawal) = &withdrawal {
        state.scheduler.forget_evidence(withdrawal);
    }
    crate::app::workflows::evidence::remove_image_runtime(&state, &image_id).await?;
    tokio::task::spawn_blocking(move || cleanup.run());
    Ok(info)
}
