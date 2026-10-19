//! The task center: one list and one event stream for every task the
//! application runs — the scheduler's plugin jobs and the registry's native
//! tasks (`task_manager`). Native kinds decide how long they stay listed
//! ([`TaskListing`]): this session's history of the registry, or their own
//! store (search history, timeline status, carving sessions) after they end.
//!
//! Entry ids are the native task ids (a carving scan is its task id, a
//! search its search id, a timeline build `timeline-build-<image id>`), and
//! `plugin:<job id>` for plugin jobs; [`cancel`] takes either.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use tokio::sync::Mutex;

use crate::app::error::AppError;
use crate::core::carving::carve_session::{CarveScanPhase, CarveScanSnapshot, CarveSessionStatus};
use crate::core::evidence::image::ReadFaultReport;
use crate::core::extensions::plugin::TaskTrigger;
use crate::core::extensions::task_scheduler::{SchedNotice, SchedTaskRecord, SchedTaskStatus};
use crate::core::search::global_search_store::{self, GlobalSearchMeta};
use crate::core::timeline::{timeline_project, timeline_store::TimelineMetaRow};
use crate::host::AppHandle;
use crate::state::{AppState, GlobalSearchRunState, TimelineBuildRunState};
use crate::task_manager::{
    TaskAmount, TaskCount, TaskFlag, TaskKind, TaskListing, TaskManager, TaskOutcome,
    TaskProgressUnit, TaskSnapshot, TaskStage, TaskStatus, TaskView,
};

/// Event carrying [`TaskEntryEvent`]s.
pub const TASK_ENTRY_EVENT: &str = "task-entry";

/// Searches listed per evidence.
const SEARCH_HISTORY_LIMIT: u32 = 100;

const PLUGIN_ENTRY_PREFIX: &str = "plugin:";

/// What the task center offers to do with a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskAction {
    Cancel,
    /// Carving scans only.
    Pause,
    Resume,
    /// Plugin jobs only: run an offline job (or one an older version
    /// paused) again from the start.
    Start,
    /// Plugin jobs only: a new run of an ended job.
    Restart,
    /// Plugin jobs only: delete the job and its results.
    Remove,
}

/// One task of the task center.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEntry {
    pub id: String,
    /// `None` for work of no evidence yet (opening evidence).
    pub image_id: Option<String>,
    /// The scheduler's status words for every task (`pause`: a paused
    /// carving scan, or a plugin job an older version paused; `offline`: a
    /// task that went away without an outcome).
    pub status: SchedTaskStatus,
    /// Cancellation was asked for and the task is stopping.
    pub cancelling: bool,
    /// Started by the application (evidence joining a case) rather than by
    /// the user.
    pub automatic: bool,
    /// Unix milliseconds.
    pub created_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub finished_at_ms: Option<i64>,
    /// `None` while the amount of work is unknown.
    pub progress: Option<TaskAmount>,
    pub error: Option<String>,
    pub actions: Vec<TaskAction>,
    #[serde(flatten)]
    pub source: TaskSource,
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "source",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TaskSource {
    /// A plugin analyzer job.
    Plugin { job: Box<SchedTaskRecord> },
    /// A native task.
    Builtin {
        kind: TaskKind,
        view: TaskView,
        /// The carving session of a recovery scan.
        #[serde(skip_serializing_if = "Option::is_none")]
        scan_id: Option<String>,
    },
}

/// A change of the task center.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TaskEntryEvent {
    Changed {
        entry: Box<TaskEntry>,
    },
    Removed {
        id: String,
    },
    /// Plugin scheduling notices (worker offline, progress timeout, all of an
    /// evidence's jobs finished, a follow-up action failed).
    Notice {
        notice: SchedNotice,
    },
}

impl TaskEntryEvent {
    pub fn changed(entry: TaskEntry) -> Self {
        Self::Changed {
            entry: Box::new(entry),
        }
    }
}

/// The registry id of an evidence's timeline build.
pub fn timeline_task_id(image_id: &str) -> String {
    format!("timeline-build-{image_id}")
}

pub fn plugin_entry_id(job_id: i64) -> String {
    format!("{PLUGIN_ENTRY_PREFIX}{job_id}")
}

/// Counts the unreadable source regions a task met.
pub fn count_read_faults(view: &mut TaskView, report: &ReadFaultReport) {
    if report.read_faults_detected {
        view.counts
            .insert(TaskCount::SyntheticBytes, report.synthetic_bytes);
        view.counts
            .insert(TaskCount::BadRegions, report.bad_regions.len() as u64);
    }
}

/// The entry of a plugin job.
pub fn job_entry(job: SchedTaskRecord) -> TaskEntry {
    let status = job.status;
    let actions = [
        (status.can_cancel(), TaskAction::Cancel),
        (status.can_start(), TaskAction::Start),
        (status.can_restart(), TaskAction::Restart),
        (status.is_terminal(), TaskAction::Remove),
    ]
    .into_iter()
    .filter_map(|(offered, action)| offered.then_some(action))
    .collect();
    TaskEntry {
        id: plugin_entry_id(job.id),
        image_id: Some(job.info.image_id.clone()),
        status,
        cancelling: false,
        automatic: job.info.trigger == TaskTrigger::Auto,
        created_at_ms: ledger_millis(&job.create_at).unwrap_or_default(),
        started_at_ms: job.start_at.as_deref().and_then(ledger_millis),
        finished_at_ms: status
            .is_terminal()
            .then(|| ledger_millis(&job.update_at))
            .flatten(),
        progress: Some(TaskAmount {
            unit: TaskProgressUnit::Percent,
            done: u64::try_from(job.progress.clamp(0, 100)).unwrap_or_default(),
            total: Some(100),
        }),
        error: job.info.error.clone(),
        actions,
        source: TaskSource::Plugin { job: Box::new(job) },
    }
}

/// The entry of a native task; `None` for kinds the task center does not
/// list and for carving scans, whose sessions are their entries.
pub fn native_entry(task: &TaskSnapshot) -> Option<TaskEntry> {
    match task.kind.listing() {
        TaskListing::Hidden => return None,
        TaskListing::Session | TaskListing::Domain => {}
    }
    if task.kind == TaskKind::CarveScan {
        return None;
    }
    let cancelling = task.outcome.is_none() && task.status == TaskStatus::CancellationRequested;
    let status = match &task.outcome {
        Some(TaskOutcome::Succeeded) => SchedTaskStatus::Success,
        Some(TaskOutcome::Failed { .. }) => SchedTaskStatus::Failed,
        Some(TaskOutcome::Cancelled) => SchedTaskStatus::Cancel,
        Some(TaskOutcome::Interrupted) => SchedTaskStatus::Offline,
        None => match task.status {
            TaskStatus::Queued => SchedTaskStatus::Queued,
            TaskStatus::Running | TaskStatus::CancellationRequested => SchedTaskStatus::Running,
        },
    };
    let error = match &task.outcome {
        Some(TaskOutcome::Failed { error }) if !error.is_empty() => Some(error.clone()),
        Some(_) | None => None,
    };
    // Merging a container's layers cannot stop halfway.
    let cancellable = task.outcome.is_none() && !cancelling && task.kind != TaskKind::DockerPrepare;
    Some(TaskEntry {
        id: task.id.clone(),
        image_id: task.image_id.clone(),
        status,
        cancelling,
        automatic: task.automatic,
        created_at_ms: task.created_at_ms,
        started_at_ms: Some(task.created_at_ms),
        finished_at_ms: task.finished_at_ms,
        progress: task.view.progress,
        error,
        actions: cancellable
            .then_some(TaskAction::Cancel)
            .into_iter()
            .collect(),
        source: TaskSource::Builtin {
            kind: task.kind,
            view: task.view.clone(),
            scan_id: None,
        },
    })
}

/// The entry of a carving session. `cancelling`: its task was cancelled and
/// is stopping.
pub fn carve_entry(scan: &CarveScanSnapshot, cancelling: bool) -> TaskEntry {
    let active = !scan.status.is_finished();
    let cancelling = active && cancelling;
    let status = match scan.status {
        CarveSessionStatus::Queued => SchedTaskStatus::Queued,
        CarveSessionStatus::Running => SchedTaskStatus::Running,
        CarveSessionStatus::Paused => SchedTaskStatus::Pause,
        CarveSessionStatus::Completed => SchedTaskStatus::Success,
        CarveSessionStatus::Cancelled => SchedTaskStatus::Cancel,
        CarveSessionStatus::Failed => SchedTaskStatus::Failed,
    };
    let filesystem = scan.filesystem.as_ref();
    let progress = match (scan.phase, filesystem) {
        (CarveScanPhase::Filesystem, Some(phase)) => Some(TaskAmount {
            unit: if phase.total.is_some() {
                TaskProgressUnit::Bytes
            } else {
                TaskProgressUnit::Entries
            },
            done: phase.done,
            total: phase.total,
        }),
        (CarveScanPhase::Filesystem, None) | (CarveScanPhase::Signature, _) => Some(TaskAmount {
            unit: TaskProgressUnit::Bytes,
            done: scan.bytes_scanned,
            total: Some(scan.bytes_total),
        }),
    };
    let mut view = TaskView {
        subject: vec![scan.scope_label.clone()],
        stage: Some(match scan.phase {
            CarveScanPhase::Filesystem => TaskStage::Filesystem,
            CarveScanPhase::Signature => TaskStage::Signature,
        }),
        progress,
        ..TaskView::default()
    };
    view.counts.insert(
        TaskCount::Found,
        filesystem.map_or(0, |phase| phase.files as u64) + scan.artifacts_found,
    );
    if scan.truncated || filesystem.is_some_and(|phase| phase.truncated) {
        view.flags.insert(TaskFlag::Truncated);
    }
    let mut actions = Vec::new();
    if active && !cancelling {
        actions.push(TaskAction::Cancel);
        match scan.status {
            CarveSessionStatus::Running => actions.push(TaskAction::Pause),
            CarveSessionStatus::Paused => actions.push(TaskAction::Resume),
            CarveSessionStatus::Queued
            | CarveSessionStatus::Completed
            | CarveSessionStatus::Cancelled
            | CarveSessionStatus::Failed => {}
        }
    }
    TaskEntry {
        id: scan.task_id.clone(),
        image_id: Some(scan.image_id.clone()),
        status,
        cancelling,
        automatic: false,
        created_at_ms: scan.created_unix_ms,
        started_at_ms: Some(scan.created_unix_ms),
        finished_at_ms: scan.finished_unix_ms,
        progress,
        error: scan.error.clone(),
        actions,
        source: TaskSource::Builtin {
            kind: TaskKind::CarveScan,
            view,
            scan_id: Some(scan.scan_id.clone()),
        },
    }
}

/// The entry of a search kept in the evidence's search history.
fn search_entry(meta: GlobalSearchMeta) -> TaskEntry {
    let status = stored_status(&meta.status);
    let mut subject = meta.keywords;
    subject.extend(meta.regexes);
    let progress = Some(TaskAmount {
        unit: TaskProgressUnit::Entries,
        done: meta.files_scanned,
        total: (meta.files_total > 0).then_some(meta.files_total),
    });
    let mut view = TaskView {
        subject,
        progress,
        ..TaskView::default()
    };
    view.counts.insert(TaskCount::Hits, meta.total_matches);
    view.counts
        .insert(TaskCount::MatchedFiles, meta.matched_file_count);
    if meta.truncated {
        view.flags.insert(TaskFlag::Truncated);
    }
    TaskEntry {
        id: meta.search_id,
        image_id: Some(meta.image_id),
        status,
        cancelling: false,
        automatic: false,
        created_at_ms: meta.started_ms,
        started_at_ms: Some(meta.started_ms),
        finished_at_ms: meta.finished_ms,
        progress,
        error: meta.error,
        actions: Vec::new(),
        source: TaskSource::Builtin {
            kind: TaskKind::GlobalSearch,
            view,
            scan_id: None,
        },
    }
}

/// The entry of an evidence's last timeline build.
fn timeline_entry(image_id: &str, meta: TimelineMetaRow) -> TaskEntry {
    let mut view = TaskView::default();
    view.counts.insert(TaskCount::FileRows, meta.file_rows);
    view.counts.insert(TaskCount::EventRows, meta.event_rows);
    TaskEntry {
        id: timeline_task_id(image_id),
        image_id: Some(image_id.to_owned()),
        status: stored_status(&meta.status),
        cancelling: false,
        automatic: false,
        created_at_ms: meta.started_ms,
        started_at_ms: Some(meta.started_ms),
        finished_at_ms: meta.finished_ms,
        progress: None,
        error: meta.error,
        actions: Vec::new(),
        source: TaskSource::Builtin {
            kind: TaskKind::TimelineBuild,
            view,
            scan_id: None,
        },
    }
}

/// Status words of the search and timeline stores. A stored `running` row
/// whose task is gone was repaired to `failed` on the way out; one still
/// running here is shadowed by its live task.
fn stored_status(status: &str) -> SchedTaskStatus {
    match status {
        "running" => SchedTaskStatus::Running,
        "success" => SchedTaskStatus::Success,
        "cancelled" => SchedTaskStatus::Cancel,
        _ => SchedTaskStatus::Failed,
    }
}

/// Ledger timestamps as Unix milliseconds: RFC 3339, or the naive UTC
/// `YYYY-MM-DD HH:MM:SS` of older task rows.
fn ledger_millis(value: &str) -> Option<i64> {
    match chrono::DateTime::parse_from_rfc3339(value) {
        Ok(parsed) => Some(parsed.timestamp_millis()),
        Err(_) => chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
            .ok()
            .map(|naive| naive.and_utc().timestamp_millis()),
    }
}

/// Every task of `image_id` (all evidence when `None`): live tasks, this
/// session's ended ones, carving sessions, searches and timeline builds of
/// the case's evidence, and plugin jobs. Tasks of no evidence are always
/// included.
pub async fn list(state: &AppState, image_id: Option<&str>) -> Result<Vec<TaskEntry>, AppError> {
    let wanted = |candidate: Option<&str>| {
        image_id.is_none() || candidate.is_none() || candidate == image_id
    };
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let mut push = |entry: TaskEntry| {
        if seen.insert(entry.id.clone()) {
            entries.push(entry);
        }
    };
    // Live tasks first: a running search shadows its stored row.
    for task in state.tasks.list().iter().chain(&state.tasks.finished()) {
        if wanted(task.image_id.as_deref()) {
            if let Some(entry) = native_entry(task) {
                push(entry);
            }
        }
    }
    let sessions = state
        .carve_scans
        .lock()
        .await
        .values()
        .cloned()
        .collect::<Vec<_>>();
    for session in sessions {
        let scan = session.read().snapshot();
        if wanted(Some(&scan.image_id)) {
            push(carve_entry(&scan, stopping(&state.tasks, &scan.task_id)));
        }
    }
    let case = {
        let active = state.active_case.lock().await;
        active.as_ref().map(|case| {
            let evidence = case
                .evidence_refs()
                .into_iter()
                .map(|evidence| evidence.image_id)
                .filter(|id| wanted(Some(id)))
                .collect::<Vec<_>>();
            (case.dir.clone(), evidence)
        })
    };
    if let Some((case_dir, evidence)) = case {
        let searches = Arc::clone(&state.global_searches);
        let builds = Arc::clone(&state.timeline_builds);
        let stored = tokio::task::spawn_blocking(move || {
            stored_entries(&case_dir, &evidence, &searches, &builds)
        })
        .await
        .map_err(|error| AppError::task_panicked("task_list", error))??;
        for entry in stored {
            push(entry);
        }
    }
    let scheduler = state.scheduler.clone();
    let filter = image_id.map(str::to_owned);
    let jobs =
        tokio::task::spawn_blocking(move || scheduler.list_for_image(filter.as_deref(), true))
            .await
            .map_err(|error| AppError::task_panicked("task_list", error))??;
    for job in jobs {
        push(job_entry(job));
    }
    Ok(entries)
}

/// Search history and timeline builds of the case's evidence. Live sets are
/// read after the rows, so a search that starts meanwhile is never taken
/// for an orphan.
fn stored_entries(
    case_dir: &Path,
    evidence: &[String],
    searches: &Mutex<HashMap<String, GlobalSearchRunState>>,
    builds: &Mutex<HashMap<String, TimelineBuildRunState>>,
) -> Result<Vec<TaskEntry>, AppError> {
    let mut entries = Vec::new();
    for image_id in evidence {
        let history =
            global_search_store::history_list(case_dir, image_id, SEARCH_HISTORY_LIMIT, || {
                searches.blocking_lock().keys().cloned().collect()
            })
            .map_err(|error| AppError::from_anyhow(error.context("读取搜索任务列表失败")))?;
        entries.extend(history.into_iter().map(search_entry));
        let building = builds.blocking_lock().contains_key(image_id);
        let timeline = timeline_project::compute_status(case_dir, image_id, building)
            .map_err(|error| AppError::from_anyhow(error.context("读取时间线状态失败")))?;
        if let Some(meta) = timeline.meta {
            entries.push(timeline_entry(image_id, meta));
        }
    }
    Ok(entries)
}

/// Whether the native task `task_id` was cancelled and is stopping.
fn stopping(tasks: &TaskManager, task_id: &str) -> bool {
    tasks
        .live(task_id)
        .is_some_and(|task| task.status == TaskStatus::CancellationRequested)
}

/// Cancels a task by its entry id.
pub async fn cancel(state: &AppState, id: &str) -> Result<(), AppError> {
    if let Some(job_id) = id.strip_prefix(PLUGIN_ENTRY_PREFIX) {
        let job_id = job_id
            .parse::<i64>()
            .map_err(|_| AppError::invalid_input(format!("无效的插件任务 ID：{id}")))?;
        state.scheduler.cancel(job_id).await?;
        return Ok(());
    }
    state.tasks.cancel(id);
    Ok(())
}

/// Tells the task center how a carving session stands.
pub fn publish_carve(app: &AppHandle, tasks: &TaskManager, scan: &CarveScanSnapshot) {
    let entry = carve_entry(scan, stopping(tasks, &scan.task_id));
    if let Err(error) = app.emit(TASK_ENTRY_EVENT, TaskEntryEvent::changed(entry)) {
        tracing::debug!("emit {TASK_ENTRY_EVENT} failed: {error}");
    }
}

#[cfg(test)]
mod tests;
