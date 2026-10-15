//! Registry and cancellation ownership for long-running backend tasks.
//!
//! A [`TaskRegistration`] is a synchronous RAII guard: dropping it removes
//! the matching registration even when an async command returns early after a
//! worker panic or join error. The registry lock is never held while a task is
//! running or while cancellation callbacks execute.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use tokio_util::sync::CancellationToken;

mod resources;

use resources::TaskResources;
pub use resources::{TaskResourceClass, TaskResourcePermit};

mod types;
pub use types::{
    TaskKind, TaskLifecycleEvent, TaskLifecyclePhase, TaskProgressEvent, TaskProgressUnit,
    TaskRecord, TaskResourceAcquireError, TaskSnapshot, TaskStatus, TASK_LIFECYCLE_EVENT,
    TASK_PROGRESS_EVENT,
};

struct ManagedTask {
    record: Arc<TaskRecord>,
    registrations: usize,
}

type TaskLifecycleSink = Arc<dyn Fn(TaskLifecycleEvent) + Send + Sync>;
type TaskProgressSink = Arc<dyn Fn(TaskProgressEvent) + Send + Sync>;

struct TaskManagerInner {
    records: Mutex<HashMap<String, ManagedTask>>,
    resources: TaskResources,
    event_sink: Mutex<Option<TaskLifecycleSink>>,
    progress_sink: Mutex<Option<TaskProgressSink>>,
}

#[derive(Clone)]
pub struct TaskManager {
    inner: Arc<TaskManagerInner>,
}

impl Default for TaskManager {
    fn default() -> Self {
        Self {
            inner: Arc::new(TaskManagerInner {
                records: Mutex::new(HashMap::new()),
                resources: TaskResources::default(),
                event_sink: Mutex::new(None),
                progress_sink: Mutex::new(None),
            }),
        }
    }
}

impl TaskManager {
    pub fn with_resource_limits(
        global_background_limit: usize,
        image_sequential_io_limit: usize,
    ) -> Self {
        Self::with_resources(TaskResources::with_primary_limits(
            global_background_limit,
            image_sequential_io_limit,
        ))
    }

    #[cfg(test)]
    fn with_all_resource_limits(
        global_background_limit: usize,
        image_sequential_io_limit: usize,
        image_random_io_limit: usize,
        cpu_task_limit: usize,
        gpu_task_limit: usize,
    ) -> Self {
        Self::with_resources(TaskResources::new(
            global_background_limit,
            image_sequential_io_limit,
            image_random_io_limit,
            cpu_task_limit,
            gpu_task_limit,
        ))
    }

    fn with_resources(resources: TaskResources) -> Self {
        Self {
            inner: Arc::new(TaskManagerInner {
                records: Mutex::new(HashMap::new()),
                resources,
                event_sink: Mutex::new(None),
                progress_sink: Mutex::new(None),
            }),
        }
    }

    /// Attach a best-effort task lifecycle event sink. Tests can leave this
    /// unset; task ownership and cancellation continue to work without it.
    pub fn set_event_sink(&self, sink: impl Fn(TaskLifecycleEvent) + Send + Sync + 'static) {
        *self
            .inner
            .event_sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(sink));
    }

    /// Attach a best-effort task progress event sink. Domain-specific progress
    /// events remain the compatibility contract; this unified stream lets UI or
    /// diagnostics subscribe by task id without knowing every command family.
    pub fn set_progress_sink(&self, sink: impl Fn(TaskProgressEvent) + Send + Sync + 'static) {
        *self
            .inner
            .progress_sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(sink));
    }

    /// Register a task or acquire another guard for an existing task id.
    /// Repeated registration is idempotent: callers share one cancellation
    /// token, and the record remains live until every guard is dropped.
    pub fn register(
        &self,
        task_id: impl Into<String>,
        kind: TaskKind,
        image_id: Option<String>,
    ) -> TaskRegistration {
        let task_id = task_id.into();
        let (record, should_emit) = {
            let mut records = self.records();
            if let Some(existing) = records.get_mut(&task_id) {
                existing.registrations = existing.registrations.saturating_add(1);
                (Arc::clone(&existing.record), false)
            } else {
                let record = Arc::new(TaskRecord {
                    id: task_id.clone(),
                    kind,
                    image_id,
                    token: CancellationToken::new(),
                    started_at: Instant::now(),
                    status: Mutex::new(TaskStatus::Queued),
                });
                records.insert(
                    task_id,
                    ManagedTask {
                        record: Arc::clone(&record),
                        registrations: 1,
                    },
                );
                (record, true)
            }
        };
        if should_emit {
            self.emit_lifecycle(TaskLifecyclePhase::Queued, &record);
        }
        TaskRegistration {
            manager: self.clone(),
            record,
        }
    }

    /// Register only when the id is not already active. Background jobs with
    /// deterministic ids use this to avoid doing the same scan twice.
    pub fn register_if_absent(
        &self,
        task_id: impl Into<String>,
        kind: TaskKind,
        image_id: Option<String>,
    ) -> Option<TaskRegistration> {
        let task_id = task_id.into();
        let record = {
            let mut records = self.records();
            if records.contains_key(&task_id) {
                return None;
            }
            let record = Arc::new(TaskRecord {
                id: task_id.clone(),
                kind,
                image_id,
                token: CancellationToken::new(),
                started_at: Instant::now(),
                status: Mutex::new(TaskStatus::Queued),
            });
            records.insert(
                task_id,
                ManagedTask {
                    record: Arc::clone(&record),
                    registrations: 1,
                },
            );
            record
        };
        self.emit_lifecycle(TaskLifecyclePhase::Queued, &record);
        Some(TaskRegistration {
            manager: self.clone(),
            record,
        })
    }

    /// Cancel and unregister one task. Returns whether it was registered.
    pub fn cancel(&self, task_id: &str) -> bool {
        let removed = self.records().remove(task_id);
        if let Some(task) = removed {
            task.record.token.cancel();
            self.emit_lifecycle(TaskLifecyclePhase::CancellationRequested, &task.record);
            true
        } else {
            false
        }
    }

    /// Cancel every task owned by an image, including tasks whose workers
    /// already captured an `Arc<EvidenceSession>` and can outlive image lookup state.
    pub fn cancel_image(&self, image_id: &str) -> usize {
        self.cancel_where(|record| record.image_id() == Some(image_id))
    }

    /// Cancel one task class for an image. Session snapshot cleanup uses this
    /// when a case takes ownership without interrupting unrelated user work.
    pub fn cancel_image_kind(&self, image_id: &str, kind: TaskKind) -> usize {
        self.cancel_where(|record| record.image_id() == Some(image_id) && record.kind() == kind)
    }

    /// Cancel opens even before they have published an image session.
    pub fn cancel_kind(&self, kind: TaskKind) -> usize {
        self.cancel_where(|record| record.kind() == kind)
    }

    pub fn contains(&self, task_id: &str) -> bool {
        self.records().contains_key(task_id)
    }

    pub fn status(&self, task_id: &str) -> Option<TaskSnapshot> {
        self.records()
            .get(task_id)
            .map(|task| task.record.snapshot())
    }

    pub fn list(&self) -> Vec<TaskSnapshot> {
        let mut records = self
            .records()
            .values()
            .map(|task| Arc::clone(&task.record))
            .collect::<Vec<_>>();
        records.sort_by_key(|record| record.started_at);
        records
            .into_iter()
            .map(|record| record.snapshot())
            .collect()
    }

    pub fn emit_progress(
        &self,
        task_id: &str,
        unit: TaskProgressUnit,
        done: u64,
        total: Option<u64>,
        message: Option<String>,
    ) -> bool {
        let Some(task) = self.status(task_id) else {
            return false;
        };
        let sink = self
            .inner
            .progress_sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(sink) = sink else {
            return true;
        };
        sink(TaskProgressEvent {
            task,
            unit,
            done,
            total,
            message,
        });
        true
    }

    pub async fn acquire_resources(
        &self,
        registration: &TaskRegistration,
    ) -> Result<TaskResourcePermit, TaskResourceAcquireError> {
        let permit = self.inner.resources.acquire(registration.record()).await?;
        registration.record().set_status(TaskStatus::Running);
        self.emit_lifecycle(TaskLifecyclePhase::Running, registration.record());
        Ok(permit)
    }

    fn cancel_where(&self, predicate: impl Fn(&TaskRecord) -> bool) -> usize {
        let removed = {
            let mut records = self.records();
            let ids = records
                .iter()
                .filter(|(_, task)| predicate(&task.record))
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|id| records.remove(&id))
                .collect::<Vec<_>>()
        };
        for task in &removed {
            task.record.token.cancel();
            self.emit_lifecycle(TaskLifecyclePhase::CancellationRequested, &task.record);
        }
        removed.len()
    }

    fn release(&self, record: &Arc<TaskRecord>) {
        let mut records = self.records();
        let should_remove = match records.get_mut(record.id()) {
            Some(current) if Arc::ptr_eq(&current.record, record) => {
                current.registrations = current.registrations.saturating_sub(1);
                current.registrations == 0
            }
            Some(_) | None => false,
        };
        if should_remove {
            records.remove(record.id());
            drop(records);
            self.emit_lifecycle(TaskLifecyclePhase::Finished, record);
        }
    }

    fn records(&self) -> MutexGuard<'_, HashMap<String, ManagedTask>> {
        self.inner
            .records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn emit_lifecycle(&self, phase: TaskLifecyclePhase, record: &TaskRecord) {
        let sink = self
            .inner
            .event_sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(sink) = sink else {
            return;
        };
        sink(TaskLifecycleEvent {
            phase,
            task: record.snapshot(),
        });
    }
}

pub struct TaskRegistration {
    manager: TaskManager,
    record: Arc<TaskRecord>,
}

impl TaskRegistration {
    pub fn token(&self) -> CancellationToken {
        self.record.token.clone()
    }

    pub fn record(&self) -> &TaskRecord {
        &self.record
    }
}

impl Drop for TaskRegistration {
    fn drop(&mut self) {
        self.manager.release(&self.record);
    }
}

#[cfg(test)]
mod tests;
