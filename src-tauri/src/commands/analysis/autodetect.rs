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

use crate::host::AppHandle;
use crate::host::State;

use crate::app::error::AppError;
use crate::core::analysis::identify::autodetect::AutoDetectReport;
use crate::state::AppState;

/// 手动触发识别。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn autodetect_evidence(
    app: AppHandle,
    state: State<'_, AppState>,
    image_id: String,
) -> Result<AutoDetectReport, AppError> {
    crate::app::workflows::identify::autodetect_evidence(app, state, image_id).await
}

pub use crate::app::workflows::identify::{AUTODETECT_FAILED_EVENT, AUTODETECT_FINISHED_EVENT};
