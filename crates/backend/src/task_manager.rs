//! Registry and cancellation ownership for long-running backend tasks.
//!
//! A [`TaskRegistration`] is a synchronous RAII guard: dropping it removes
//! the matching registration even when an async command returns early after a
//! worker panic or join error. The registry lock is never held while a task is
//! running or while cancellation callbacks execute.
//!
//! Every task also carries the task center's [`TaskView`] of it — what it
//! works on, how far it got, what it found — and reports how it ended
//! ([`TaskOutcome`]) just before its last registration goes. Tasks of kinds
//! the task center lists for the session ([`TaskListing::Session`]) stay in
//! a bounded history after they end, until their evidence is unloaded or,
//! for tasks of no evidence, until the case changes.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use tokio_util::sync::CancellationToken;

mod resources;

use resources::TaskResources;
pub use resources::{TaskResourceClass, TaskResourcePermit};

mod types;
use types::{unix_millis_now, TaskState};
pub use types::{
    TaskKind, TaskLifecycleEvent, TaskLifecyclePhase, TaskProgressEvent, TaskProgressUnit,
    TaskRecord, TaskResourceAcquireError, TaskSnapshot, TaskStatus, TASK_LIFECYCLE_EVENT,
    TASK_PROGRESS_EVENT,
};

mod view;
pub use view::{
    TaskAmount, TaskCount, TaskFailure, TaskFlag, TaskLabel, TaskListing, TaskOutcome, TaskStage,
    TaskView,
};

/// Finished tasks kept for the task center, oldest dropped first.
const FINISHED_HISTORY_LIMIT: usize = 200;

type TaskLifecycleSink = Arc<dyn Fn(TaskLifecycleEvent) + Send + Sync>;
type TaskProgressSink = Arc<dyn Fn(TaskProgressEvent) + Send + Sync>;

struct TaskManagerInner {
    /// Live tasks by id.
    records: Mutex<HashMap<String, Arc<TaskRecord>>>,
    /// Cancelled tasks whose workers have not let go yet. Their ids are
    /// free again: a new registration under the same id is a new task.
    stopping: Mutex<Vec<Arc<TaskRecord>>>,
    finished: Mutex<VecDeque<TaskSnapshot>>,
    /// Bumped when the case changes (see [`TaskManager::forget_unowned`]).
    generation: AtomicU64,
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
        Self::with_resources(TaskResources::default())
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
                stopping: Mutex::new(Vec::new()),
                finished: Mutex::new(VecDeque::new()),
                generation: AtomicU64::new(0),
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
        let (record, created) = {
            let mut records = self.records();
            if let Some(existing) = records.get(&task_id) {
                existing.registrations.fetch_add(1, Ordering::AcqRel);
                (Arc::clone(existing), false)
            } else {
                let record = self.new_record(&task_id, kind, image_id);
                records.insert(task_id, Arc::clone(&record));
                (record, true)
            }
        };
        if created {
            self.forget_finished(record.id());
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
            let record = self.new_record(&task_id, kind, image_id);
            records.insert(task_id, Arc::clone(&record));
            record
        };
        self.forget_finished(record.id());
        self.emit_lifecycle(TaskLifecyclePhase::Queued, &record);
        Some(TaskRegistration {
            manager: self.clone(),
            record,
        })
    }

    fn new_record(
        &self,
        task_id: &str,
        kind: TaskKind,
        image_id: Option<String>,
    ) -> Arc<TaskRecord> {
        Arc::new(TaskRecord {
            id: task_id.to_owned(),
            kind,
            image_id,
            token: CancellationToken::new(),
            started_at: Instant::now(),
            created_at_ms: unix_millis_now(),
            generation: self.inner.generation.load(Ordering::Acquire),
            registrations: AtomicUsize::new(1),
            status: Mutex::new(TaskStatus::Queued),
            state: Mutex::new(TaskState::default()),
        })
    }

    /// Cancel and unregister one task. Returns whether it was registered.
    pub fn cancel(&self, task_id: &str) -> bool {
        let removed = self.records().remove(task_id);
        match removed {
            Some(record) => {
                self.stop(&[record]);
                true
            }
            None => false,
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
        self.records().get(task_id).map(|record| record.snapshot())
    }

    /// The task with `task_id` while it runs, including while it stops
    /// after a cancellation.
    pub fn live(&self, task_id: &str) -> Option<TaskSnapshot> {
        self.find(task_id).map(|record| record.snapshot())
    }

    /// Live tasks, oldest first, including cancelled ones still stopping.
    pub fn list(&self) -> Vec<TaskSnapshot> {
        let mut records = self.records().values().cloned().collect::<Vec<_>>();
        records.extend(self.stopping().iter().cloned());
        records.sort_by_key(|record| record.started_at);
        records
            .into_iter()
            .map(|record| record.snapshot())
            .collect()
    }

    /// Ended tasks kept for the task center, oldest first.
    pub fn finished(&self) -> Vec<TaskSnapshot> {
        self.finished_tasks().iter().cloned().collect()
    }

    /// Drops the ended tasks of an unloaded image.
    pub fn forget_image(&self, image_id: &str) {
        self.finished_tasks()
            .retain(|task| task.image_id.as_deref() != Some(image_id));
    }

    /// Drops the ended tasks of no evidence (evidence loads) when the case
    /// changes; such tasks registered before now are not kept either.
    pub fn forget_unowned(&self) {
        self.inner.generation.fetch_add(1, Ordering::AcqRel);
        self.finished_tasks().retain(|task| task.image_id.is_some());
    }

    /// Changes the view of a live task. Returns whether the task exists.
    pub fn update(&self, task_id: &str, change: impl FnOnce(&mut TaskView)) -> bool {
        let Some(record) = self.find(task_id) else {
            return false;
        };
        if record.change_view(change) {
            self.emit_lifecycle(TaskLifecyclePhase::Updated, &record);
        }
        true
    }

    /// Reports how a live task ended; the first report stands. Returns
    /// whether the task exists.
    pub fn finish(&self, task_id: &str, outcome: TaskOutcome) -> bool {
        let Some(record) = self.find(task_id) else {
            return false;
        };
        if record.report(outcome) {
            self.emit_lifecycle(TaskLifecyclePhase::Updated, &record);
        }
        true
    }

    pub fn emit_progress(
        &self,
        task_id: &str,
        unit: TaskProgressUnit,
        done: u64,
        total: Option<u64>,
        message: Option<String>,
    ) -> bool {
        let Some(record) = self.find(task_id) else {
            return false;
        };
        record.state().view.progress = Some(TaskAmount { unit, done, total });
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
            task: record.snapshot(),
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
                .filter(|(_, record)| predicate(record))
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|id| records.remove(&id))
                .collect::<Vec<_>>()
        };
        self.stop(&removed);
        removed.len()
    }

    /// Cancels unregistered tasks; they stop on their own time.
    fn stop(&self, removed: &[Arc<TaskRecord>]) {
        self.stopping().extend(removed.iter().cloned());
        for record in removed {
            record.token.cancel();
            self.emit_lifecycle(TaskLifecyclePhase::CancellationRequested, record);
        }
    }

    fn release(&self, record: &Arc<TaskRecord>) {
        let last = {
            // Decrement under the registry lock so `register` cannot join a
            // record its last guard is finishing.
            let mut records = self.records();
            let last = record.registrations.fetch_sub(1, Ordering::AcqRel) == 1;
            if last
                && records
                    .get(record.id())
                    .is_some_and(|current| Arc::ptr_eq(current, record))
            {
                records.remove(record.id());
            }
            last
        };
        if !last {
            return;
        }
        self.stopping()
            .retain(|stopping| !Arc::ptr_eq(stopping, record));
        let snapshot = record.final_snapshot();
        if record.kind.listing() == TaskListing::Session
            && (record.image_id.is_some()
                || record.generation == self.inner.generation.load(Ordering::Acquire))
        {
            let mut finished = self.finished_tasks();
            finished.retain(|task| task.id != snapshot.id);
            finished.push_back(snapshot.clone());
            while finished.len() > FINISHED_HISTORY_LIMIT {
                finished.pop_front();
            }
        }
        self.send_lifecycle(TaskLifecyclePhase::Finished, snapshot);
    }

    /// The live or stopping task with `task_id` (the live one first).
    fn find(&self, task_id: &str) -> Option<Arc<TaskRecord>> {
        if let Some(record) = self.records().get(task_id) {
            return Some(Arc::clone(record));
        }
        self.stopping()
            .iter()
            .rev()
            .find(|record| record.id() == task_id)
            .cloned()
    }

    /// A new task under an ended task's id replaces the ended one.
    fn forget_finished(&self, task_id: &str) {
        self.finished_tasks().retain(|task| task.id != task_id);
    }

    fn records(&self) -> MutexGuard<'_, HashMap<String, Arc<TaskRecord>>> {
        self.inner
            .records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn stopping(&self) -> MutexGuard<'_, Vec<Arc<TaskRecord>>> {
        self.inner
            .stopping
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn finished_tasks(&self) -> MutexGuard<'_, VecDeque<TaskSnapshot>> {
        self.inner
            .finished
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn emit_lifecycle(&self, phase: TaskLifecyclePhase, record: &TaskRecord) {
        self.send_lifecycle(phase, record.snapshot());
    }

    fn send_lifecycle(&self, phase: TaskLifecyclePhase, task: TaskSnapshot) {
        let sink = self
            .inner
            .event_sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(sink) = sink else {
            return;
        };
        sink(TaskLifecycleEvent { phase, task });
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

    /// Changes the task center's view of this task.
    pub fn update(&self, change: impl FnOnce(&mut TaskView)) {
        if self.record.change_view(change) {
            self.manager
                .emit_lifecycle(TaskLifecyclePhase::Updated, &self.record);
        }
    }

    /// Marks the task as started by the application, not by the user.
    pub fn set_automatic(&self) {
        self.record.state().automatic = true;
        self.manager
            .emit_lifecycle(TaskLifecyclePhase::Updated, &self.record);
    }

    /// Reports how the task ended; the first report stands.
    pub fn finish(&self, outcome: TaskOutcome) {
        if self.record.report(outcome) {
            self.manager
                .emit_lifecycle(TaskLifecyclePhase::Updated, &self.record);
        }
    }

    /// Reports the outcome of the work that returned `result`.
    pub fn finish_with<T, E: TaskFailure>(&self, result: &Result<T, E>) {
        self.finish(TaskOutcome::of(result));
    }
}

impl Drop for TaskRegistration {
    fn drop(&mut self) {
        self.manager.release(&self.record);
    }
}

#[cfg(test)]
mod tests;
