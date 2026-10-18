//! 时间线 IPC:投影物化视图的命令面。
//!
//! 命令职责：
//!
//! - `timeline_status`：状态机
//!   `buildRequired` / `running` / `stale`(有新数据未处理) /
//!   `ready` + 全库时间范围 startTime/endTime；
//! - `timeline_start`：创建重建任务（遍历存量源数据
//!   整体重投影，`TaskKind::TimelineBuild` 可 `taskCancel`），进度经
//!   `timeline-progress` 事件与通用 `task-progress` 双通道上抛；
//! - `timeline_types`：`group by from_app,type` 的
//!   两级（应用 → 数据类型）维度清单；
//! - `timeline_count`：strftime 动态时间桶聚合（时区
//!   参数化，带 Limit/Offset 桶分页）；
//! - `timeline_datas`：明细分页，默认 `tl_time desc`；
//! - `timeline_export`：导出复用与明细表一致的 `TimelineQuery` 过滤条件。
//!
//! 时间线按检材存放（检材目录的 `timeline.db`），所有查询都带 `image_id`；
//! 检材还没有建过时间线时按空结果返回，不建库。插件任务收尾的写路径增量
//! 投影在 `task_scheduler::executor`（按 run_id 级联删 + 重投影），不经命令层。

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crate::host::{AppHandle, State};
use serde::Serialize;

use crate::app::error::AppError;
use crate::commands::quote_csv_field;
use crate::core::timeline::timeline_project::{
    self, compute_status, TimelineBuildSummary, TimelineStatusInfo,
};
use crate::core::timeline::timeline_store::{
    self, TimelineBucket, TimelineDataPage, TimelineQuery, TimelineRow, TimelineTypeGroup,
};
use crate::state::{AppState, TimelineBuildRunState};
use crate::task_manager::{TaskKind, TaskProgressUnit};

/// 进度事件（前端按 `image_id` 过滤）。
pub const TIMELINE_PROGRESS_EVENT: &str = "timeline-progress";

/// 明细分页上限（与全局搜索命中分页一致）。
const PAGE_MAX_LIMIT: u64 = 2000;
const PAGE_DEFAULT_LIMIT: u64 = 200;

#[derive(Debug, Clone, Serialize)]
pub struct TimelineProgress {
    pub image_id: String,
    pub done: u64,
    pub total: u64,
    pub rows: u64,
}

#[derive(Debug, Serialize)]
pub struct TimelineExportResult {
    pub output_path: String,
    pub record_count: u64,
}

/// 全库时间范围（`timeline_range` 响应；无行返回 `None`）。
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct TimelineRange {
    pub start_time: i64,
    pub end_time: i64,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

async fn active_case_dir(state: &AppState) -> Result<PathBuf, AppError> {
    let active = state.active_case.lock().await;
    active
        .as_ref()
        .map(|case| case.dir.clone())
        .ok_or_else(|| AppError::invalid_input("时间线需要先打开案件"))
}

/// 查询投影状态、时间范围与计数。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn timeline_status(
    state: State<'_, AppState>,
    image_id: String,
) -> Result<TimelineStatusInfo, AppError> {
    let case_dir = active_case_dir(&state).await?;
    let running = {
        let builds = state.timeline_builds.lock().await;
        builds.contains_key(&image_id)
    };
    tokio::task::spawn_blocking(move || compute_status(&case_dir, &image_id, running))
        .await
        .map_err(|err| AppError::task_panicked("timeline_status", err))?
        .map_err(|err| AppError::from_anyhow(err.context("读取时间线状态失败")))
}

/// 创建重建任务并等待完成（长任务，取消走
/// `taskCancel({taskId: 'timeline-build-<imageId>'})`）。取消保留已落库的
/// 部分投影（`status=cancelled`）。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn timeline_start(
    app: AppHandle,
    state: State<'_, AppState>,
    image_id: String,
) -> Result<TimelineBuildSummary, AppError> {
    let case_dir = active_case_dir(&state).await?;
    let image = crate::app::access::loaded_image_app_error(&state, &image_id).await?;
    // 同检材重建串行化：确定性任务 id + register_if_absent 拦截重复 start。
    let task_id = format!("timeline-build-{image_id}");
    let Some(task) = state.tasks.register_if_absent(
        task_id.clone(),
        TaskKind::TimelineBuild,
        Some(image_id.clone()),
    ) else {
        return Err(AppError::invalid_input("时间线正在构建，请等待完成后再试"));
    };
    let resource_permit = state.tasks.acquire_resources(&task).await?;
    let token = task.token();
    let started = Instant::now();
    let started_ms = now_ms();
    tracing::info!("timeline_start begin image={image_id}");

    state.timeline_builds.lock().await.insert(
        image_id.clone(),
        TimelineBuildRunState {
            started_ms,
            done: 0,
            total: 0,
            rows: 0,
        },
    );

    let runs = Arc::clone(&state.timeline_builds);
    let tasks = state.tasks.clone();
    let app_for_progress = app.clone();
    let scan_image_id = image_id.clone();
    let scan_case_dir = case_dir.clone();
    let scan_task_id = task_id.clone();
    let outcome = tokio::task::spawn_blocking(move || -> anyhow::Result<TimelineBuildSummary> {
        let _resource_permit = resource_permit;
        let on_progress = |done: u64, total: u64, rows: u64| {
            let progress = TimelineProgress {
                image_id: scan_image_id.clone(),
                done,
                total,
                rows,
            };
            if let Err(err) = app_for_progress.emit(TIMELINE_PROGRESS_EVENT, &progress) {
                tracing::warn!("emit {TIMELINE_PROGRESS_EVENT} failed: {err}");
            }
            tasks.emit_progress(
                &scan_task_id,
                TaskProgressUnit::Entries,
                done,
                (total > 0).then_some(total),
                None,
            );
            if let Ok(mut runs) = runs.try_lock() {
                if let Some(run) = runs.get_mut(&progress.image_id) {
                    run.done = done;
                    run.total = total;
                    run.rows = rows;
                }
            }
        };
        timeline_project::run_rebuild_with_image(&scan_case_dir, &image, &token, &on_progress)
    })
    .await;

    state.timeline_builds.lock().await.remove(&image_id);
    let summary = outcome
        .map_err(|err| AppError::task_panicked("timeline_start", err))?
        .map_err(|err| AppError::from_anyhow(err.context("时间线重建失败")))?;
    tracing::info!(
        "timeline_start done image={image_id} status={} elapsed={:.3?} files={}/{} events={}/{} skipped={}",
        summary.status,
        started.elapsed(),
        summary.file_rows,
        summary.files_total,
        summary.event_rows,
        summary.events_total,
        summary.skipped_invalid_time
    );
    Ok(summary)
}

/// 两级维度清单。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn timeline_types(
    state: State<'_, AppState>,
    image_id: String,
) -> Result<Vec<TimelineTypeGroup>, AppError> {
    let case_dir = active_case_dir(&state).await?;
    tokio::task::spawn_blocking(
        move || match timeline_store::open_reader(&case_dir, &image_id)? {
            Some(conn) => timeline_store::types(&conn),
            None => Ok(Vec::new()),
        },
    )
    .await
    .map_err(|err| AppError::task_panicked("timeline_types", err))?
    .map_err(|err| AppError::from_anyhow(err.context("读取时间线维度失败")))
}

/// strftime 时间桶聚合（`limit` 为 `None` = 全部桶）。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn timeline_count(
    state: State<'_, AppState>,
    image_id: String,
    query: TimelineQuery,
    limit: Option<u64>,
    offset: Option<u64>,
) -> Result<Vec<TimelineBucket>, AppError> {
    let case_dir = active_case_dir(&state).await?;
    let offset = offset.unwrap_or(0);
    tokio::task::spawn_blocking(
        move || match timeline_store::open_reader(&case_dir, &image_id)? {
            Some(conn) => timeline_store::count_buckets(&conn, &query, limit, offset),
            None => Ok(Vec::new()),
        },
    )
    .await
    .map_err(|err| AppError::task_panicked("timeline_count", err))?
    .map_err(|err| AppError::from_anyhow(err.context("时间线聚合失败")))
}

/// 明细分页（默认 `tl_time desc`；`order` = `asc|desc`）。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn timeline_datas(
    state: State<'_, AppState>,
    image_id: String,
    query: TimelineQuery,
    skip: Option<u64>,
    limit: Option<u64>,
    order: Option<String>,
) -> Result<TimelineDataPage, AppError> {
    let case_dir = active_case_dir(&state).await?;
    let ascending = match order.as_deref() {
        None | Some("desc") => false,
        Some("asc") => true,
        Some(other) => return Err(AppError::invalid_input(format!("未知排序方向：{other}"))),
    };
    let page_query = timeline_store::TimelinePageQuery {
        query,
        skip: skip.unwrap_or(0),
        limit: limit.unwrap_or(PAGE_DEFAULT_LIMIT).clamp(1, PAGE_MAX_LIMIT),
        ascending,
    };
    tokio::task::spawn_blocking(
        move || match timeline_store::open_reader(&case_dir, &image_id)? {
            Some(conn) => timeline_store::page(&conn, &image_id, &page_query),
            None => Ok(TimelineDataPage {
                items: Vec::new(),
                total: 0,
            }),
        },
    )
    .await
    .map_err(|err| AppError::task_panicked("timeline_datas", err))?
    .map_err(|err| AppError::from_anyhow(err.context("读取时间线明细失败")))
}

/// 检材时间线的时间范围 `min/max(tl_time)`。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn timeline_range(
    state: State<'_, AppState>,
    image_id: String,
) -> Result<Option<TimelineRange>, AppError> {
    let case_dir = active_case_dir(&state).await?;
    tokio::task::spawn_blocking(move || -> anyhow::Result<Option<TimelineRange>> {
        let Some(conn) = timeline_store::open_reader(&case_dir, &image_id)? else {
            return Ok(None);
        };
        Ok(
            timeline_store::time_range(&conn)?.map(|(start_time, end_time)| TimelineRange {
                start_time,
                end_time,
            }),
        )
    })
    .await
    .map_err(|err| AppError::task_panicked("timeline_range", err))?
    .map_err(|err| AppError::from_anyhow(err.context("读取时间线范围失败")))
}

/// 导出为 UTF-8 BOM CSV，沿用明细表当前过滤（`tl_time asc` 流式逐行）。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn timeline_export(
    state: State<'_, AppState>,
    image_id: String,
    query: TimelineQuery,
    destination: String,
) -> Result<TimelineExportResult, AppError> {
    let case_dir = active_case_dir(&state).await?;
    tokio::task::spawn_blocking(move || -> Result<TimelineExportResult, AppError> {
        let conn = timeline_store::open_reader(&case_dir, &image_id)
            .map_err(|err| AppError::from_anyhow(err.context("导出时间线失败")))?
            .ok_or_else(|| AppError::invalid_input("该检材还没有创建时间线"))?;
        let file = std::fs::File::create(&destination)
            .map_err(|err| AppError::from_io("导出时间线", err))?;
        let mut writer = std::io::BufWriter::new(file);
        writer
            .write_all("\u{FEFF}序号,时间,类型,内容,时间类型,来源应用,检材,已删除\r\n".as_bytes())
            .map_err(|err| AppError::from_io("导出时间线", err))?;
        let mut count = 0u64;
        timeline_store::export_rows(&conn, &image_id, &query, |row: &TimelineRow| {
            let fields = [
                (count + 1).to_string(),
                format_epoch_utc(row.tl_time),
                timeline_store::data_type_name(row.data_type),
                row.content.clone(),
                row.tl_flag.clone(),
                row.from_app.clone(),
                row.eid.clone(),
                if row.deleted { "是" } else { "否" }.to_string(),
            ];
            let line = fields
                .iter()
                .map(|field| quote_csv_field(field))
                .collect::<Vec<_>>()
                .join(",");
            writer
                .write_all(line.as_bytes())
                .and_then(|_| writer.write_all(b"\r\n"))
                .map_err(|err| anyhow::anyhow!("导出时间线失败：{err}"))?;
            count += 1;
            Ok(())
        })
        .map_err(|err| AppError::from_anyhow(err.context("导出时间线失败")))?;
        writer
            .flush()
            .map_err(|err| AppError::from_io("导出时间线", err))?;
        Ok(TimelineExportResult {
            output_path: destination,
            record_count: count,
        })
    })
    .await
    .map_err(|err| AppError::task_panicked("timeline_export", err))?
}

fn format_epoch_utc(epoch: i64) -> String {
    use chrono::TimeZone;
    chrono::Utc
        .timestamp_opt(epoch, 0)
        .single()
        .map(|datetime| datetime.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| epoch.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_formats_as_utc_display_string() {
        assert_eq!(format_epoch_utc(1704067200), "2024-01-01 00:00:00");
        // 超出 chrono 可表达范围的值退回原始秒数，不 panic。
        assert_eq!(format_epoch_utc(i64::MAX), i64::MAX.to_string());
    }
}
