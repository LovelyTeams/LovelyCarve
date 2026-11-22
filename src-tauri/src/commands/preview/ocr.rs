//! 原类型视图「文字识别（OCR）」命令。读取所选图片文件节点的字节，跑 PP-OCRv6 OCR
//! （见 `core::preview::ocr`），结果同步作为返回值回给前端。`predict` 是单次不可中断调用，故不发进度
//! 事件；命令仍注册到 `TaskManager` 以参与后台 CPU/GPU 资源限流。登记任务前先加载推理组件：
//! 组件的 OCR 执行方式决定任务占 GPU 还是 CPU 槽位，组件缺失或不带 OCR 时直接报错。

use crate::core::common::ids::{ImageId, NodeId};
use crate::host::State;

use crate::core::filesystem::tree::fs_tree;
use crate::core::preview::ocr::{self, OcrOutcome};
use crate::state::AppState;
use crate::task_manager::{TaskKind, TaskProgressUnit};
use {crate::app::access::loaded_image_app_error, crate::app::error::AppError};

/// 识别指定图片文件节点中的文字。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn ocr_scan(
    state: State<'_, AppState>,
    image_id: ImageId,
    node_id: NodeId,
    task_id: Option<String>,
) -> Result<OcrOutcome, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    tokio::task::spawn_blocking(ocr::ensure_available)
        .await
        .map_err(|err| AppError::task_panicked("ocr_scan", err))?
        .map_err(AppError::from)?;
    let task_id = task_id.unwrap_or_else(|| format!("ocr:{}", uuid::Uuid::new_v4()));
    let task_id_for_progress = task_id.clone();
    let task = state
        .tasks
        .register(task_id, TaskKind::OcrScan, Some(image_id));
    let resource_permit = state.tasks.acquire_resources(&task).await?;
    state.tasks.emit_progress(
        &task_id_for_progress,
        TaskProgressUnit::Attempts,
        0,
        Some(1),
        Some("ocr:recognizing".to_string()),
    );
    let tasks = state.tasks.clone();

    tokio::task::spawn_blocking(move || -> Result<OcrOutcome, AppError> {
        let _resource_permit = resource_permit;
        let bytes = fs_tree::read_file(&image, &node_id).map_err(AppError::from)?;
        let outcome = ocr::recognize(&bytes).map_err(AppError::from);
        if outcome.is_ok() {
            tasks.emit_progress(
                &task_id_for_progress,
                TaskProgressUnit::Attempts,
                1,
                Some(1),
                Some("ocr:done".to_string()),
            );
        }
        outcome
    })
    .await
    .map_err(|err| AppError::task_panicked("ocr_scan", err))?
}
