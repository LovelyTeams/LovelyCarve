//! 原类型视图「二维码识别」命令。读取所选文件节点的字节，跑 OpenCV 的
//! WeChat 检测器 + 暴力变换扫描（见 `core::preview::qrcode`），进度走 `qr-scan-progress`
//! 事件，可经 `task_cancel` 协作取消。结果同步作为返回值回给前端。

use crate::host::{AppHandle, State};
use serde::Serialize;

use crate::core::filesystem::tree::fs_tree;
use crate::core::preview::qrcode::{self, QrScanOutcome};
use crate::state::AppState;
use crate::task_manager::{TaskKind, TaskProgressUnit};
use {crate::app::access::loaded_image_app_error, crate::app::error::AppError};

/// `qr-scan-progress` 事件载荷。前端按 `task_id` 过滤，忽略并行任务。
#[derive(Debug, Clone, Serialize)]
pub struct QrScanProgress {
    pub task_id: String,
    /// 已尝试的变换组合数。
    pub attempts: u32,
    /// 变换组合总数。
    pub total: u32,
    /// 当前已找到的二维码数量。
    pub found: u32,
}

/// 扫描指定文件节点中的二维码。`expected_count` 达到即提前停止；`task_id` 由前端
/// 生成，用于进度过滤与取消。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn qr_scan(
    app: AppHandle,
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    expected_count: u32,
    task_id: String,
) -> Result<QrScanOutcome, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;

    let task = state
        .tasks
        .register(task_id.clone(), TaskKind::QrScan, Some(image_id.clone()));
    let resource_permit = state.tasks.acquire_resources(&task).await?;
    let token = task.token();

    let expected = expected_count.max(1);
    let app_for_blk = app.clone();
    let task_for_blk = task_id.clone();
    let tasks_for_blk = state.tasks.clone();

    tokio::task::spawn_blocking(move || -> Result<QrScanOutcome, AppError> {
        let _resource_permit = resource_permit;
        let bytes = fs_tree::read_file(&image, &node_id).map_err(AppError::from_anyhow)?;
        let mut last_emit = 0u32;
        qrcode::scan(
            &bytes,
            expected,
            |attempts, total, found| {
                // 节流：首尾各一次、其余每 8 个组合一次，避免事件风暴。
                if attempts == 1 || attempts >= total || attempts.saturating_sub(last_emit) >= 8 {
                    last_emit = attempts;
                    let _ = app_for_blk.emit(
                        "qr-scan-progress",
                        QrScanProgress {
                            task_id: task_for_blk.clone(),
                            attempts,
                            total,
                            found,
                        },
                    );
                    tasks_for_blk.emit_progress(
                        &task_for_blk,
                        TaskProgressUnit::Attempts,
                        attempts as u64,
                        Some(total as u64),
                        Some(format!("found={found}")),
                    );
                }
            },
            || token.is_cancelled(),
        )
        .map_err(|err| {
            if token.is_cancelled() {
                AppError::cancelled("二维码扫描已取消")
            } else {
                AppError::from_anyhow(err)
            }
        })
    })
    .await
    .map_err(|err| AppError::task_panicked("qr_scan", err))?
}
