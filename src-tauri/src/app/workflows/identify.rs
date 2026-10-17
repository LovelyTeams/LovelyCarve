//! 自动识别平台 IPC:结果回写与任务链触发。
//!
//! 识别完成后回写检材的 platform/type/extra 和分区信息。提供两条路径:
//!
//! - [`autodetect_evidence`]:前端手动触发(`trigger: manually`),返回完整报告;
//! - [`spawn_first_task`]:检材首次登记进案件时由 `register_and_snapshot`
//!   拉起的后台首任务。
//!
//! 两条路径都登记为 `TaskManager` 的 `autoDetect` 任务(id 按检材固定,并发的
//! 两次运行共用一份登记与取消令牌),发出统一的 `task-lifecycle` 事件;登记释放前
//! 以 [`AUTODETECT_FINISHED_EVENT`] 广播报告,或以 [`AUTODETECT_FAILED_EVENT`]
//! 广播失败与取消,任务中心只按这两个领域事件判定终态。有活动案件且检材已登记时
//! 把结论写回台账的检材行(平台、类型与完整报告 JSON);无案件时只返回报告。识别在
//! `spawn_blocking` 上跑——分区根目录列举与归档条目走查都是同步 I/O。

use std::sync::Arc;

use crate::host::{AppHandle, State};
use serde::Serialize;
use tokio::sync::Mutex;

use crate::app::access::loaded_image_app_error;
use crate::app::error::{AppError, ErrorCode};
use crate::core::analysis::identify::autodetect::{self, AutoDetectReport};
use crate::core::casework::case::CaseState;
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::state::AppState;
use crate::task_manager::{TaskKind, TaskManager, TaskRegistration};

/// 识别完成事件,载荷为 [`AutoDetectReport`]。
pub const AUTODETECT_FINISHED_EVENT: &str = "autodetect-finished";
/// 识别失败或取消事件,载荷为 [`AutoDetectFailed`]。
pub const AUTODETECT_FAILED_EVENT: &str = "autodetect-failed";

/// 失败载荷;`error` 沿用命令错误的稳定 IPC 形状,取消为 `cancelled` 码。
#[derive(Clone, Serialize)]
struct AutoDetectFailed<'a> {
    image_id: &'a str,
    error: &'a AppError,
}

/// 手动触发识别。
pub async fn autodetect_evidence(
    app: AppHandle,
    state: State<'_, AppState>,
    image_id: String,
) -> Result<AutoDetectReport, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let task = register(&state.tasks, &image);
    run_autodetect(&app, &state.tasks, &state.active_case, image, task).await
}

/// 检材首次上线的首任务。同步登记,`image_load` 返回前任务中心已能看到它;
/// 失败只进任务中心与日志:识别不成功不影响检材加载。
pub(crate) fn spawn_first_task(
    app: AppHandle,
    tasks: TaskManager,
    active_case: Arc<Mutex<Option<CaseState>>>,
    image: Arc<EvidenceSession>,
) {
    let task = register(&tasks, &image);
    tokio::spawn(async move {
        let image_id = image.id.clone();
        if let Err(err) = run_autodetect(&app, &tasks, &active_case, image, task).await {
            if err.code == ErrorCode::Cancelled {
                tracing::debug!("autodetect: first task for {image_id} cancelled");
            } else {
                tracing::warn!("autodetect: first task for {image_id} failed: {err:?}");
            }
        }
    });
}

/// 任务 id 按检材固定:同时进行的首任务与手动重跑共用一份登记,任务中心只显示一条。
fn register(tasks: &TaskManager, image: &EvidenceSession) -> TaskRegistration {
    tasks.register(
        format!("autodetect:{}", image.id),
        TaskKind::AutoDetect,
        Some(image.id.clone()),
    )
}

/// 识别并回写,再广播结论或失败;登记最后释放,领域事件总在
/// `task-lifecycle` 的 finished 之前。
async fn run_autodetect(
    app: &AppHandle,
    tasks: &TaskManager,
    active_case: &Mutex<Option<CaseState>>,
    image: Arc<EvidenceSession>,
    task: TaskRegistration,
) -> Result<AutoDetectReport, AppError> {
    let image_id = image.id.clone();
    let outcome = detect_and_persist(tasks, active_case, image, &task).await;
    let emitted = match &outcome {
        Ok(report) => app.emit(AUTODETECT_FINISHED_EVENT, report),
        Err(error) => app.emit(
            AUTODETECT_FAILED_EVENT,
            AutoDetectFailed {
                image_id: &image_id,
                error,
            },
        ),
    };
    if let Err(err) = emitted {
        tracing::warn!("autodetect: emit outcome for {image_id}: {err}");
    }
    drop(task);
    outcome
}

async fn detect_and_persist(
    tasks: &TaskManager,
    active_case: &Mutex<Option<CaseState>>,
    image: Arc<EvidenceSession>,
    task: &TaskRegistration,
) -> Result<AutoDetectReport, AppError> {
    // `TaskKind::AutoDetect` 不占后台配额:这里不排队,只把任务标为运行中。
    let _permit = tasks.acquire_resources(task).await?;
    let cancel = task.token();
    let detection = {
        let cancel = cancel.clone();
        tokio::task::spawn_blocking(move || autodetect::detect_image(&image, &cancel))
    };
    let report = detection
        .await
        .map_err(|err| AppError::task_panicked("autodetect_evidence", err))?
        .map_err(|cancelled| AppError::cancelled(cancelled.to_string()))?;
    // 识别收尾之后才到的取消同样不回写。
    if cancel.is_cancelled() {
        return Err(AppError::cancelled(autodetect::DetectCancelled.to_string()));
    }
    persist_report(active_case, &report).await;
    Ok(report)
}

/// 回写:平台、检材类型,以及完整报告(厂商、显示名、备份特征、告警、逐
/// 分区判定)。检材不在案件中时静默跳过。
async fn persist_report(active_case: &Mutex<Option<CaseState>>, report: &AutoDetectReport) {
    let detection_json = match serde_json::to_string(report) {
        Ok(json) => json,
        Err(err) => {
            tracing::warn!("autodetect: encode report for {}: {err}", report.image_id);
            return;
        }
    };
    let mut guard = active_case.lock().await;
    let Some(case) = guard.as_mut() else {
        return;
    };
    if !case.has_evidence(&report.image_id) {
        return;
    }
    match case.record_evidence_detection(
        &report.image_id,
        report.platform.map(|platform| platform.as_str()),
        report.evidence_type.as_str(),
        &detection_json,
    ) {
        Ok(true) => tracing::info!(
            "autodetect: evidence {} platform={} type={}",
            report.image_id,
            report.platform.map_or("", |platform| platform.as_str()),
            report.evidence_type
        ),
        Ok(false) => {}
        Err(err) => tracing::warn!("autodetect: update evidence failed: {err}"),
    }
}
