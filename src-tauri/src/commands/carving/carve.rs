//! Background recovery scans: optional filesystem recovery, then raw-signature carving.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::host::{AppHandle, State};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::app::error::AppError;
use crate::app::workflows::task_center;
use crate::commands::carving::carve_filesystem::{run_filesystem_phase, sector_ranges};
use crate::core::carving::carve_session::{
    wait_while_paused, CarveScanPhase, CarveScanSession, CarveScanSnapshot, CarveSessionStatus,
};
use crate::core::carving::carver::{
    self, CarveArtifact, CarveRegion, CarveScanConfig, CarveScanError, CarveScanEvent,
    CarveScanOutput, Carver, DEFAULT_MAX_FILE_SIZE, DEFAULT_MAX_RESULTS,
};
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::recovery::{RecoveryOptions, SectorRange};
use crate::state::AppState;
use crate::task_manager::{TaskKind, TaskManager, TaskProgressUnit};
use {crate::app::access::loaded_image_app_error, crate::app::access::reject_if_folder};

pub const CARVE_SCAN_PROGRESS_EVENT: &str = "carve-scan-progress";
const MAX_PAGE_SIZE: usize = 500;
const DEFAULT_PAGE_SIZE: usize = 200;
const MAX_PREVIEW_BYTES: u64 = 8 * 1024 * 1024;
const RETAINED_SCANS_PER_IMAGE: usize = 3;

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CarveScope {
    Whole,
    Unallocated,
    Partition,
    /// `start_sector` and `sector_count` of the options.
    Range,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CarveStartOptions {
    #[serde(default)]
    pub format_ids: Vec<String>,
    #[serde(default = "default_scope")]
    pub scope: CarveScope,
    #[serde(default)]
    pub partition_id: Option<u8>,
    #[serde(default)]
    pub start_sector: Option<u64>,
    #[serde(default)]
    pub sector_count: Option<u64>,
    #[serde(default = "default_max_file_size")]
    pub max_file_size: u64,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    /// Filesystem families recovered before carving; all off skips that phase.
    #[serde(default)]
    pub filesystems: RecoveryOptions,
    /// Raw-signature carving; off recovers filesystem records only.
    #[serde(default = "default_signatures")]
    pub signatures: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CarveStartResult {
    pub scan_id: String,
    pub task_id: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CarveResultFilter {
    #[serde(default)]
    pub format_id: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub min_confidence: Option<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CarveResultsPage {
    pub items: Vec<CarveArtifact>,
    pub next_cursor: Option<String>,
    pub resume_cursor: Option<String>,
    pub total: usize,
    pub truncated: bool,
    pub scan_finished: bool,
}

fn default_scope() -> CarveScope {
    CarveScope::Whole
}

fn default_max_file_size() -> u64 {
    DEFAULT_MAX_FILE_SIZE
}

fn default_max_results() -> usize {
    DEFAULT_MAX_RESULTS
}

fn default_signatures() -> bool {
    true
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub fn carve_formats() -> Vec<carver::CarveFormatInfo> {
    carver::available_formats()
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn carve_start(
    app: AppHandle,
    state: State<'_, AppState>,
    image_id: String,
    options: CarveStartOptions,
) -> Result<CarveStartResult, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    reject_if_folder(&image)?;
    let filesystem = options.filesystems.enabled();
    if !filesystem && !options.signatures {
        return Err(AppError::invalid_input("请至少选择一种恢复方式"));
    }
    let (regions, scope_label) = resolve_regions(&image, &options)?;
    let bytes_total = regions.iter().map(|region| region.length).sum();
    if bytes_total == 0 {
        return Err(AppError::invalid_input("所选扫描范围为空"));
    }
    let ranges = sector_ranges(&regions, image.sector_size as u64);
    if filesystem && ranges.is_empty() {
        return Err(AppError::invalid_input(
            "所选范围不足一个扇区，无法扫描文件系统",
        ));
    }
    let carver = if options.signatures {
        let config = CarveScanConfig {
            format_ids: options.format_ids,
            max_file_size: options.max_file_size,
            max_results: options.max_results,
            worker_threads: None,
        };
        Some(Carver::try_new(&config).map_err(AppError::from_anyhow)?)
    } else {
        None
    };
    let scan_id = uuid::Uuid::new_v4().to_string();
    let task_id = uuid::Uuid::new_v4().to_string();
    let mut created = CarveScanSession::new(
        scan_id.clone(),
        task_id.clone(),
        image_id.clone(),
        scope_label,
        bytes_total,
    );
    if filesystem {
        created = created.with_filesystem_recovery(options.filesystems, options.signatures);
    }
    let session = Arc::new(RwLock::new(created));

    {
        let mut scans = state.carve_scans.lock().await;
        prune_finished_scans(&mut scans, &image_id);
        scans.insert(scan_id.clone(), Arc::clone(&session));
    }

    let task = state
        .tasks
        .register(task_id.clone(), TaskKind::CarveScan, Some(image_id));
    let token = task.token();
    let tasks = state.tasks.clone();
    tokio::spawn(async move {
        let resource_permit = match tasks.acquire_resources(&task).await {
            Ok(permit) => permit,
            Err(error) => {
                session.write().finish(
                    if token.is_cancelled() {
                        CarveSessionStatus::Cancelled
                    } else {
                        CarveSessionStatus::Failed
                    },
                    Some(error.message()),
                );
                emit_scan_snapshot(&app, &tasks, &session);
                return;
            }
        };
        session.write().status = CarveSessionStatus::Running;
        emit_scan_snapshot(&app, &tasks, &session);

        let worker = ScanWorker {
            app: app.clone(),
            session: Arc::clone(&session),
            tasks: tasks.clone(),
            task_id: task.record().id().to_string(),
            token: token.clone(),
        };
        let outcome = tokio::task::spawn_blocking(move || {
            let _resource_permit = resource_permit;
            worker.run(&image, carver, &regions, ranges)
        })
        .await;

        let mut scan = session.write();
        match outcome {
            Ok(Ok(output)) => {
                if let Some(output) = output {
                    scan.bytes_scanned = output.bytes_scanned;
                    scan.bytes_total = output.bytes_total;
                    scan.signatures_found = output.signatures_found;
                    scan.truncated = output.truncated;
                }
                let error = scan.error.take();
                scan.finish(CarveSessionStatus::Completed, error);
            }
            Ok(Err(CarveScanError::Cancelled)) => {
                scan.finish(CarveSessionStatus::Cancelled, None);
            }
            Ok(Err(CarveScanError::Failed(error))) => {
                scan.finish(CarveSessionStatus::Failed, Some(format!("{error:#}")));
            }
            Err(error) => {
                scan.finish(
                    CarveSessionStatus::Failed,
                    Some(format!("恢复扫描任务异常退出：{error}")),
                );
            }
        }
        drop(scan);
        // The scan's task center entry is its session; let the registration
        // go only after the session shows how it ended.
        emit_scan_snapshot(&app, &tasks, &session);
        drop(task);
    });

    Ok(CarveStartResult { scan_id, task_id })
}

/// Blocking side of one session: filesystem recovery, then signature carving.
struct ScanWorker {
    app: AppHandle,
    session: Arc<RwLock<CarveScanSession>>,
    tasks: TaskManager,
    task_id: String,
    token: tokio_util::sync::CancellationToken,
}

impl ScanWorker {
    /// Returns the carving summary, or `None` when the session skips carving.
    fn run(
        &self,
        image: &Arc<EvidenceSession>,
        carver: Option<Carver>,
        regions: &[CarveRegion],
        ranges: Vec<SectorRange>,
    ) -> Result<Option<CarveScanOutput>, CarveScanError> {
        image
            .validate_source_file_stable()
            .map_err(CarveScanError::from)?;
        let mut last_emit = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        run_filesystem_phase(
            image,
            &self.session,
            ranges,
            &self.token,
            &mut |done, total| {
                if last_emit.elapsed() >= Duration::from_millis(120) {
                    let unit = if total.is_some() {
                        TaskProgressUnit::Bytes
                    } else {
                        TaskProgressUnit::Entries
                    };
                    self.tasks.emit_progress(
                        &self.task_id,
                        unit,
                        done,
                        total,
                        Some("扫描文件系统记录".to_string()),
                    );
                    emit_scan_snapshot(&self.app, &self.tasks, &self.session);
                    last_emit = Instant::now();
                }
            },
        )?;
        let Some(carver) = carver else {
            image
                .validate_source_file_stable()
                .map_err(CarveScanError::from)?;
            return Ok(None);
        };
        self.session.write().phase = CarveScanPhase::Signature;
        emit_scan_snapshot(&self.app, &self.tasks, &self.session);
        let output = carver.scan_regions(image.disk_image(), regions, &self.token, |event| {
            wait_while_paused(&self.session, &self.token);
            match event {
                CarveScanEvent::Artifact(artifact) => {
                    self.session.write().artifacts.push(artifact);
                }
                CarveScanEvent::Progress(progress) => {
                    self.session.write().apply_progress(&progress);
                    if last_emit.elapsed() >= Duration::from_millis(120)
                        || progress.bytes_scanned >= progress.bytes_total
                    {
                        self.tasks.emit_progress(
                            &self.task_id,
                            TaskProgressUnit::Bytes,
                            progress.bytes_scanned,
                            Some(progress.bytes_total),
                            Some(format!(
                                "命中 {} 个签名，验证 {} 个文件",
                                progress.signatures_found, progress.artifacts_found
                            )),
                        );
                        emit_scan_snapshot(&self.app, &self.tasks, &self.session);
                        last_emit = Instant::now();
                    }
                }
            }
        })?;
        image
            .validate_source_file_stable()
            .map_err(CarveScanError::from)?;
        Ok(Some(output))
    }
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn carve_scan_status(
    state: State<'_, AppState>,
    scan_id: String,
) -> Result<CarveScanSnapshot, AppError> {
    let session = scan_session(&state, &scan_id).await?;
    let snapshot = session.read().snapshot();
    Ok(snapshot)
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn carve_scan_pause(
    app: AppHandle,
    state: State<'_, AppState>,
    scan_id: String,
) -> Result<CarveScanSnapshot, AppError> {
    let session = scan_session(&state, &scan_id).await?;
    let snapshot = {
        let mut scan = session.write();
        match scan.status {
            CarveSessionStatus::Running => scan.status = CarveSessionStatus::Paused,
            CarveSessionStatus::Paused => {}
            CarveSessionStatus::Queued => {
                return Err(AppError::invalid_input("排队中的雕刻扫描暂不能暂停"));
            }
            CarveSessionStatus::Completed
            | CarveSessionStatus::Cancelled
            | CarveSessionStatus::Failed => {
                return Err(AppError::invalid_input("已结束的雕刻扫描不能暂停"));
            }
        }
        scan.snapshot()
    };
    emit_scan_snapshot(&app, &state.tasks, &session);
    Ok(snapshot)
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn carve_scan_resume(
    app: AppHandle,
    state: State<'_, AppState>,
    scan_id: String,
) -> Result<CarveScanSnapshot, AppError> {
    let session = scan_session(&state, &scan_id).await?;
    let snapshot = {
        let mut scan = session.write();
        match scan.status {
            CarveSessionStatus::Paused => scan.status = CarveSessionStatus::Running,
            CarveSessionStatus::Running => {}
            CarveSessionStatus::Queued => {
                return Err(AppError::invalid_input("排队中的雕刻扫描无需继续"));
            }
            CarveSessionStatus::Completed
            | CarveSessionStatus::Cancelled
            | CarveSessionStatus::Failed => {
                return Err(AppError::invalid_input("已结束的雕刻扫描不能继续"));
            }
        }
        scan.snapshot()
    };
    emit_scan_snapshot(&app, &state.tasks, &session);
    Ok(snapshot)
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn carve_latest_scan(
    state: State<'_, AppState>,
    image_id: String,
) -> Result<Option<CarveScanSnapshot>, AppError> {
    let sessions = {
        let scans = state.carve_scans.lock().await;
        scans.values().cloned().collect::<Vec<_>>()
    };
    Ok(sessions
        .into_iter()
        .filter_map(|session| {
            let scan = session.read();
            (scan.image_id == image_id).then(|| scan.snapshot())
        })
        .max_by_key(|snapshot| snapshot.created_unix_ms))
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn carve_results_page(
    state: State<'_, AppState>,
    scan_id: String,
    cursor: Option<String>,
    limit: Option<usize>,
    filter: Option<CarveResultFilter>,
) -> Result<CarveResultsPage, AppError> {
    let after_sequence = cursor
        .as_deref()
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| AppError::invalid_input("无效的雕刻结果游标"))
        })
        .transpose()?
        .unwrap_or(0);
    let limit = limit.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE);
    let filter = filter.unwrap_or_default();
    let session = scan_session(&state, &scan_id).await?;
    let scan = session.read();
    let matches = |artifact: &&CarveArtifact| {
        artifact.sequence > after_sequence
            && filter
                .format_id
                .as_deref()
                .is_none_or(|format| artifact.format_id == format)
            && filter
                .category
                .as_deref()
                .is_none_or(|category| artifact.category == category)
            && filter
                .min_confidence
                .is_none_or(|minimum| artifact.confidence >= minimum)
    };
    let total = scan
        .artifacts
        .iter()
        .filter(|artifact| {
            filter
                .format_id
                .as_deref()
                .is_none_or(|format| artifact.format_id == format)
                && filter
                    .category
                    .as_deref()
                    .is_none_or(|category| artifact.category == category)
                && filter
                    .min_confidence
                    .is_none_or(|minimum| artifact.confidence >= minimum)
        })
        .count();
    let mut page = scan
        .artifacts
        .iter()
        .filter(matches)
        .take(limit + 1)
        .cloned()
        .collect::<Vec<_>>();
    let has_more = page.len() > limit;
    if has_more {
        page.truncate(limit);
    }
    let resume_cursor = page
        .last()
        .map(|artifact| artifact.sequence.to_string())
        .or(cursor);
    let next_cursor = has_more.then(|| {
        page.last()
            .map(|artifact| artifact.sequence.to_string())
            .unwrap_or_default()
    });
    Ok(CarveResultsPage {
        items: page,
        next_cursor,
        resume_cursor,
        total,
        truncated: scan.truncated,
        scan_finished: scan.status.is_finished(),
    })
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn carve_preview(
    state: State<'_, AppState>,
    scan_id: String,
    artifact_id: String,
    max_bytes: Option<u64>,
) -> Result<crate::host::Response, AppError> {
    let session = scan_session(&state, &scan_id).await?;
    let (image_id, artifact) = {
        let scan = session.read();
        let artifact = scan
            .artifacts
            .iter()
            .find(|artifact| artifact.id == artifact_id)
            .cloned()
            .ok_or_else(|| AppError::not_found("雕刻结果不存在"))?;
        (scan.image_id.clone(), artifact)
    };
    let image = loaded_image_app_error(&state, &image_id).await?;
    let length = max_bytes
        .unwrap_or(MAX_PREVIEW_BYTES)
        .clamp(1, MAX_PREVIEW_BYTES)
        .min(artifact.length);
    let length = usize::try_from(length)
        .map_err(|_| AppError::resource_limit("当前平台无法分配雕刻预览缓冲区"))?;
    let data = tokio::task::spawn_blocking(move || image.read_range(artifact.offset, length))
        .await
        .map_err(|error| AppError::task_panicked("carve_preview", error))?
        .map_err(AppError::from_anyhow)?;
    Ok(crate::host::Response::new(data))
}

pub(crate) async fn scan_session(
    state: &State<'_, AppState>,
    scan_id: &str,
) -> Result<Arc<RwLock<CarveScanSession>>, AppError> {
    let scans = state.carve_scans.lock().await;
    scans
        .get(scan_id)
        .cloned()
        .ok_or_else(|| AppError::not_found("恢复扫描会话不存在"))
}

fn prune_finished_scans(
    scans: &mut std::collections::HashMap<String, Arc<RwLock<CarveScanSession>>>,
    image_id: &str,
) {
    let mut finished = scans
        .iter()
        .filter_map(|(id, session)| {
            let scan = session.read();
            (scan.image_id == image_id && scan.status.is_finished())
                .then(|| (id.clone(), scan.created_unix_ms))
        })
        .collect::<Vec<_>>();
    finished.sort_by_key(|(_, created)| std::cmp::Reverse(*created));
    for (id, _) in finished.into_iter().skip(RETAINED_SCANS_PER_IMAGE - 1) {
        scans.remove(&id);
    }
}

/// Tells the carving page and the task center how a session stands.
fn emit_scan_snapshot(
    app: &AppHandle,
    tasks: &TaskManager,
    session: &Arc<RwLock<CarveScanSession>>,
) {
    let snapshot = session.read().snapshot();
    task_center::publish_carve(app, tasks, &snapshot);
    if let Err(error) = app.emit(CARVE_SCAN_PROGRESS_EVENT, snapshot) {
        tracing::debug!("emit carve scan progress failed: {error}");
    }
}

fn resolve_regions(
    image: &EvidenceSession,
    options: &CarveStartOptions,
) -> Result<(Vec<CarveRegion>, String), AppError> {
    match options.scope {
        CarveScope::Whole => Ok((
            vec![CarveRegion {
                offset: 0,
                length: image.len(),
                label: "整个镜像".to_string(),
            }],
            "整个镜像".to_string(),
        )),
        CarveScope::Partition => {
            let partition_id = options
                .partition_id
                .ok_or_else(|| AppError::invalid_input("指定分区扫描必须提供 partition_id"))?;
            let partition = image
                .partitions
                .iter()
                .find(|partition| partition.id == partition_id)
                .ok_or_else(|| AppError::not_found("指定分区不存在"))?;
            let sector_size = image.sector_size as u64;
            let offset = partition
                .start_lba
                .checked_mul(sector_size)
                .ok_or_else(|| AppError::resource_limit("分区起始偏移溢出"))?;
            let length = partition
                .length_lba
                .checked_mul(sector_size)
                .ok_or_else(|| AppError::resource_limit("分区长度溢出"))?
                .min(image.len().saturating_sub(offset.min(image.len())));
            let label = partition
                .label
                .clone()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| format!("分区 {partition_id}"));
            Ok((
                vec![CarveRegion {
                    offset,
                    length,
                    label: label.clone(),
                }],
                label,
            ))
        }
        CarveScope::Unallocated => {
            let regions = unallocated_regions(image);
            if regions.is_empty() {
                return Err(AppError::invalid_input("当前分区表没有可扫描的分区间隙"));
            }
            let label = format!("分区间隙（{} 段）", regions.len());
            Ok((regions, label))
        }
        CarveScope::Range => {
            let (Some(start_sector), Some(sector_count)) =
                (options.start_sector, options.sector_count)
            else {
                return Err(AppError::invalid_input(
                    "扇区范围扫描必须提供 start_sector 与 sector_count",
                ));
            };
            let sector_size = image.sector_size as u64;
            let total_sectors = image.len() / sector_size;
            let end_sector = start_sector
                .checked_add(sector_count)
                .filter(|end| sector_count > 0 && *end <= total_sectors)
                .ok_or_else(|| AppError::invalid_input("扇区范围超出检材"))?;
            let label = format!("扇区 {start_sector}–{}", end_sector - 1);
            Ok((
                vec![CarveRegion {
                    offset: start_sector * sector_size,
                    length: sector_count * sector_size,
                    label: label.clone(),
                }],
                label,
            ))
        }
    }
}

fn unallocated_regions(image: &EvidenceSession) -> Vec<CarveRegion> {
    let sector_size = image.sector_size as u64;
    let mut occupied = image
        .partitions
        .iter()
        .filter_map(|partition| {
            let start = partition
                .start_lba
                .checked_mul(sector_size)?
                .min(image.len());
            let length = partition.length_lba.checked_mul(sector_size)?;
            let end = start.saturating_add(length).min(image.len());
            (end > start).then_some((start, end))
        })
        .collect::<Vec<_>>();
    occupied.sort_unstable_by_key(|(start, _)| *start);
    let mut cursor = 0u64;
    let mut regions = Vec::new();
    for (start, end) in occupied {
        if start > cursor {
            regions.push(CarveRegion {
                offset: cursor,
                length: start - cursor,
                label: format!("间隙 0x{cursor:016X}-0x{start:016X}"),
            });
        }
        cursor = cursor.max(end);
    }
    if cursor < image.len() {
        regions.push(CarveRegion {
            offset: cursor,
            length: image.len() - cursor,
            label: format!("间隙 0x{cursor:016X}-0x{:016X}", image.len()),
        });
    }
    regions
}
