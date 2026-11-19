//! Records when each examination task on case evidence starts and how it
//! settles, in the case's hash-chained audit journal.
//!
//! The task registry reports lifecycle events synchronously from whichever
//! thread changes a task, so [`TaskJournal::observe`] only queues the event; a
//! dedicated thread appends `task.started` (the task acquired its resources
//! and began) and `task.settled` (its last registration dropped, with its
//! outcome, counts and flags). Each entry names the evidence and carries the
//! task's view: what it worked on and what it found. A task whose case closes
//! first, or whose process ends, has no `task.settled` entry and stays
//! incomplete in the journal; nothing invents its ending afterwards.

use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tokio::sync::{oneshot, Mutex};

use crate::core::casework::case::CaseState;
use crate::task_manager::{
    TaskKind, TaskLifecycleEvent, TaskLifecyclePhase, TaskOutcome, TaskSnapshot, TaskView,
};

const TASK_STARTED: &str = "task.started";
const TASK_SETTLED: &str = "task.settled";

enum JournalMessage {
    Record(&'static str, Box<TaskSnapshot>),
    /// Answered once every message queued before it is handled.
    Flush(oneshot::Sender<()>),
}

/// Queues task lifecycle events for the journal writer thread. Clones share
/// the one writer.
#[derive(Clone)]
pub(crate) struct TaskJournal {
    messages: mpsc::Sender<JournalMessage>,
}

impl TaskJournal {
    /// Starts the writer thread, which records into whichever case is active
    /// when an event arrives.
    pub(crate) fn start(active_case: Arc<Mutex<Option<CaseState>>>) -> std::io::Result<Self> {
        let (messages, received) = mpsc::channel();
        std::thread::Builder::new()
            .name("task-journal".to_owned())
            .spawn(move || {
                for message in received {
                    match message {
                        JournalMessage::Record(action, task) => {
                            record(&active_case, action, &task);
                        }
                        JournalMessage::Flush(done) => {
                            if done.send(()).is_err() {
                                tracing::debug!("task journal flush abandoned by its caller");
                            }
                        }
                    }
                }
            })?;
        Ok(Self { messages })
    }

    /// Queues `event` when it starts or settles a journaled task of evidence.
    pub(crate) fn observe(&self, event: &TaskLifecycleEvent) {
        let Some(action) = journal_action(event) else {
            return;
        };
        if self
            .messages
            .send(JournalMessage::Record(action, Box::new(event.task.clone())))
            .is_err()
        {
            tracing::warn!(
                "task journal writer stopped; {action} of task {} not recorded",
                event.task.id
            );
        }
    }

    /// Waits, for at most `timeout`, until the events queued so far are
    /// written. Shutdown calls it before the case closes.
    pub(crate) async fn flush(&self, timeout: Duration) {
        let (done, written) = oneshot::channel();
        if self.messages.send(JournalMessage::Flush(done)).is_err() {
            tracing::warn!("task journal writer stopped before a flush");
            return;
        }
        match tokio::time::timeout(timeout, written).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => tracing::warn!("task journal writer stopped during a flush"),
            Err(_) => tracing::warn!("task journal flush timed out after {timeout:?}"),
        }
    }
}

/// The journal action for `event`, or `None` when the journal skips it.
fn journal_action(event: &TaskLifecycleEvent) -> Option<&'static str> {
    if event.task.image_id.is_none() || !event.task.kind.journaled() {
        return None;
    }
    match event.phase {
        TaskLifecyclePhase::Running => Some(TASK_STARTED),
        TaskLifecyclePhase::Finished => Some(TASK_SETTLED),
        TaskLifecyclePhase::Queued
        | TaskLifecyclePhase::Updated
        | TaskLifecyclePhase::CancellationRequested => None,
    }
}

/// The journal detail of one task event.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskEntryDetail<'a> {
    task_id: &'a str,
    kind: TaskKind,
    automatic: bool,
    created_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    finished_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<&'a TaskOutcome>,
    view: &'a TaskView,
}

fn record(active_case: &Mutex<Option<CaseState>>, action: &str, task: &TaskSnapshot) {
    let Some(image_id) = task.image_id.as_deref() else {
        return;
    };
    let detail = match serde_json::to_string(&TaskEntryDetail {
        task_id: &task.id,
        kind: task.kind,
        automatic: task.automatic,
        created_at_ms: task.created_at_ms,
        finished_at_ms: task.finished_at_ms,
        outcome: task.outcome.as_ref(),
        view: &task.view,
    }) {
        Ok(detail) => detail,
        Err(error) => {
            tracing::warn!("task journal: encode {action} of task {}: {error}", task.id);
            return;
        }
    };
    let case = active_case.blocking_lock();
    let Some(case) = case.as_ref() else {
        return;
    };
    if let Err(error) = case.journal_task_event(action, image_id, &detail) {
        tracing::warn!(
            "task journal: record {action} of task {} failed: {error:#}",
            task.id
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::Mutex;

    use super::{journal_action, record, TaskJournal, TASK_SETTLED, TASK_STARTED};
    use crate::core::casework::case::test_support::case_with_evidence;
    use crate::core::casework::case_journal;
    use crate::task_manager::{
        TaskKind, TaskLifecycleEvent, TaskLifecyclePhase, TaskManager, TaskOutcome,
    };

    fn events_of(manager: &TaskManager) -> Arc<std::sync::Mutex<Vec<TaskLifecycleEvent>>> {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        manager.set_event_sink(move |event| sink.lock().expect("events").push(event));
        events
    }

    #[test]
    fn starts_and_settlements_of_evidence_tasks_are_journaled() {
        let manager = TaskManager::default();
        let events = events_of(&manager);
        let hash = manager.register("hash-1", TaskKind::ImageHash, Some("img".into()));
        hash.update(|view| view.subject.push("/disk.E01".to_string()));
        hash.finish(TaskOutcome::Succeeded);
        drop(hash);
        drop(manager.register("flat-1", TaskKind::FlatList, Some("img".into())));
        drop(manager.register("load-1", TaskKind::ImageLoad, None));

        let actions: Vec<_> = events
            .lock()
            .expect("events")
            .iter()
            .filter_map(|event| journal_action(event).map(|action| (action, event.task.id.clone())))
            .collect();
        assert_eq!(actions, [(TASK_SETTLED, "hash-1".to_string())]);

        let running = TaskLifecycleEvent {
            phase: TaskLifecyclePhase::Running,
            task: events.lock().expect("events")[0].task.clone(),
        };
        assert_eq!(journal_action(&running), Some(TASK_STARTED));
    }

    #[tokio::test]
    async fn a_flush_waits_for_queued_events_to_reach_the_journal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let active_case = Arc::new(Mutex::new(Some(case_with_evidence(dir.path(), "img"))));
        let journal = TaskJournal::start(Arc::clone(&active_case)).expect("journal");
        let manager = TaskManager::default();
        let observer = journal.clone();
        manager.set_event_sink(move |event| observer.observe(&event));
        let hash = manager.register("hash-1", TaskKind::ImageHash, Some("img".into()));
        hash.finish(TaskOutcome::Cancelled);
        drop(hash);

        journal.flush(std::time::Duration::from_secs(10)).await;

        let case = active_case.lock().await;
        let ledger =
            rusqlite::Connection::open(case.as_ref().expect("case").ledger_path()).expect("ledger");
        let settled = case_journal::recent(&ledger, 10)
            .expect("journal")
            .into_iter()
            .find(|entry| entry.action == TASK_SETTLED)
            .expect("settled entry written before the flush returned");
        let detail: serde_json::Value = serde_json::from_str(&settled.detail).expect("detail");
        assert_eq!(detail["outcome"]["state"].as_str(), Some("cancelled"));
    }

    #[test]
    fn task_events_are_recorded_only_for_evidence_of_the_active_case() {
        let dir = tempfile::tempdir().expect("tempdir");
        let case = case_with_evidence(dir.path(), "img");
        let case_uid = case.info().id;
        let active_case = Arc::new(Mutex::new(Some(case)));
        let manager = TaskManager::default();
        let events = events_of(&manager);
        let hash = manager.register("hash-1", TaskKind::ImageHash, Some("img".into()));
        hash.update(|view| view.subject.push("/disk.E01".to_string()));
        hash.finish(TaskOutcome::Succeeded);
        drop(hash);
        drop(manager.register("hash-2", TaskKind::ImageHash, Some("other".into())));

        for event in events.lock().expect("events").iter() {
            if let Some(action) = journal_action(event) {
                record(&active_case, action, &event.task);
            }
        }

        let case = active_case.blocking_lock();
        let case = case.as_ref().expect("case");
        let ledger = rusqlite::Connection::open(case.ledger_path()).expect("ledger");
        let entries = case_journal::recent(&ledger, 10).expect("journal");
        let settled = entries
            .iter()
            .find(|entry| entry.action == TASK_SETTLED)
            .expect("settled entry");
        let detail: serde_json::Value = serde_json::from_str(&settled.detail).expect("detail");
        assert_eq!(
            (
                settled.evidence.as_deref(),
                detail["taskId"].as_str(),
                detail["kind"].as_str(),
                detail["outcome"]["state"].as_str(),
                detail["view"]["subject"][0].as_str(),
            ),
            (
                Some("img"),
                Some("hash-1"),
                Some("imageHash"),
                Some("succeeded"),
                Some("/disk.E01")
            )
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.action == TASK_SETTLED)
                .count(),
            1,
            "the task of evidence outside the case is not recorded"
        );
        assert_eq!(
            case_journal::verify(&ledger, &case_uid)
                .expect("verify")
                .first_broken_seq,
            None
        );
    }
}
