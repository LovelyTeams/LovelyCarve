//! Immediate task status, progress and record models.
use super::TaskResourceClass;
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
    CancellationRequested,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskProgressUnit {
    Attempts,
    Bytes,
    Entries,
}

#[derive(Debug)]
pub struct TaskRecord {
    pub(super) id: String,
    pub(super) kind: TaskKind,
    pub(super) image_id: Option<String>,
    pub(super) token: CancellationToken,
    pub(super) started_at: Instant,
    pub(super) status: Mutex<TaskStatus>,
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

    pub(super) fn snapshot(&self) -> TaskSnapshot {
        TaskSnapshot {
            id: self.id.clone(),
            kind: self.kind,
            image_id: self.image_id.clone(),
            status: self.status(),
            resource_classes: self.kind.resource_classes(),
            elapsed_ms: duration_millis_u64(self.elapsed()),
        }
    }
}

pub(super) fn duration_millis_u64(duration: Duration) -> u64 {
    let millis = duration.as_millis();
    if millis > u64::MAX as u128 {
        u64::MAX
    } else {
        millis as u64
    }
}
