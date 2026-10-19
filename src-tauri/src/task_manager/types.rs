//! Immediate task status, progress and record models.
use super::{TaskOutcome, TaskResourceClass, TaskView};
use serde::Serialize;
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

pub const TASK_LIFECYCLE_EVENT: &str = "task-lifecycle";
pub const TASK_PROGRESS_EVENT: &str = "task-progress";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskResourceAcquireError {
    Cancelled,
    SemaphoreClosed { label: String },
}

impl TaskResourceAcquireError {
    pub fn message(&self) -> String {
        match self {
            Self::Cancelled => "任务已取消".to_string(),
            Self::SemaphoreClosed { label } => format!("{label} semaphore closed"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskKind {
    AutoDetect,
    BitlockerExport,
    BitlockerScan,
    BitlockerUnlock,
    EncryptedVolumeUnlock,
    FilevaultInspect,
    CarveRecovery,
    CarveScan,
    DirectoryExport,
    /// Merging a Docker container's layers into its browsable index.
    DockerPrepare,
    FlatList,
    ImageExport,
    ImageHash,
    ImageLoad,
    IosBackupAudit,
    IosBackupUnlock,
    GlobalSearch,
    MediaMaterialization,
    OcrScan,
    QrScan,
    SessionSnapshot,
    ShadowCopyOpen,
    TimelineBuild,
    VolumeSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    Queued,
    Running,
    CancellationRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskLifecyclePhase {
    Queued,
    Running,
    /// The task's view or outcome changed.
    Updated,
    CancellationRequested,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskProgressUnit {
    Attempts,
    Bytes,
    Entries,
    /// A share of 100 (plugin jobs report no unit of their own).
    Percent,
}

#[derive(Debug)]
pub struct TaskRecord {
    pub(super) id: String,
    pub(super) kind: TaskKind,
    pub(super) image_id: Option<String>,
    pub(super) token: CancellationToken,
    pub(super) started_at: Instant,
    /// Wall-clock registration time (Unix milliseconds).
    pub(super) created_at_ms: i64,
    /// Case transitions forget the finished tasks of no evidence; a task of
    /// no evidence registered before the last one is not kept either.
    pub(super) generation: u64,
    /// Live guards; the last one to go ends the task.
    pub(super) registrations: std::sync::atomic::AtomicUsize,
    pub(super) status: Mutex<TaskStatus>,
    pub(super) state: Mutex<TaskState>,
}

/// What a task reported about itself.
#[derive(Debug, Default)]
pub(super) struct TaskState {
    pub(super) view: TaskView,
    pub(super) outcome: Option<TaskOutcome>,
    /// Started by the application rather than by the user.
    pub(super) automatic: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSnapshot {
    pub id: String,
    pub kind: TaskKind,
    pub image_id: Option<String>,
    pub status: TaskStatus,
    pub resource_classes: Vec<TaskResourceClass>,
    pub elapsed_ms: u64,
    /// Started by the application (evidence joining a case) rather than by
    /// the user.
    pub automatic: bool,
    pub created_at_ms: i64,
    /// Set once the task ended.
    pub finished_at_ms: Option<i64>,
    pub view: TaskView,
    /// How the task ended; reported just before it does.
    pub outcome: Option<TaskOutcome>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskLifecycleEvent {
    pub phase: TaskLifecyclePhase,
    pub task: TaskSnapshot,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskProgressEvent {
    pub task: TaskSnapshot,
    pub unit: TaskProgressUnit,
    pub done: u64,
    pub total: Option<u64>,
    pub message: Option<String>,
}

impl TaskRecord {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn kind(&self) -> TaskKind {
        self.kind
    }

    pub fn image_id(&self) -> Option<&str> {
        self.image_id.as_deref()
    }

    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    pub fn status(&self) -> TaskStatus {
        if self.token.is_cancelled() {
            TaskStatus::CancellationRequested
        } else {
            *self
                .status
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }
    }

    pub(super) fn set_status(&self, status: TaskStatus) {
        *self
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = status;
    }

    pub(super) fn state(&self) -> std::sync::MutexGuard<'_, TaskState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Applies `change` to the view; returns whether it changed anything.
    pub(super) fn change_view(&self, change: impl FnOnce(&mut TaskView)) -> bool {
        let mut state = self.state();
        let before = state.view.clone();
        change(&mut state.view);
        state.view != before
    }

    /// Records the outcome unless one was reported already.
    pub(super) fn report(&self, outcome: TaskOutcome) -> bool {
        let mut state = self.state();
        if state.outcome.is_some() {
            return false;
        }
        state.outcome = Some(outcome);
        true
    }

    pub(super) fn snapshot(&self) -> TaskSnapshot {
        let state = self.state();
        TaskSnapshot {
            id: self.id.clone(),
            kind: self.kind,
            image_id: self.image_id.clone(),
            status: self.status(),
            resource_classes: self.kind.resource_classes(),
            elapsed_ms: duration_millis_u64(self.elapsed()),
            automatic: state.automatic,
            created_at_ms: self.created_at_ms,
            finished_at_ms: None,
            view: state.view.clone(),
            outcome: state.outcome.clone(),
        }
    }

    /// The snapshot kept once the last registration went away: its reported
    /// outcome, or cancelled / interrupted when it reported none.
    pub(super) fn final_snapshot(&self) -> TaskSnapshot {
        let mut snapshot = self.snapshot();
        if snapshot.outcome.is_none() {
            snapshot.outcome = Some(if self.token.is_cancelled() {
                TaskOutcome::Cancelled
            } else {
                TaskOutcome::Interrupted
            });
        }
        snapshot.finished_at_ms = Some(unix_millis_now());
        snapshot
    }
}

pub(super) fn unix_millis_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

pub(super) fn duration_millis_u64(duration: Duration) -> u64 {
    let millis = duration.as_millis();
    if millis > u64::MAX as u128 {
        u64::MAX
    } else {
        millis as u64
    }
}
