//! Background work: tasks still running when the command that started them
//! returns, and tasks the application starts on its own (volume indexes,
//! platform detection and plugin jobs when evidence joins a case). Waits
//! report how they ended — `tasks wait`, and the wait before a one-shot
//! command, a shell or a JSONL session exits — and a failure fails the wait,
//! so the process never passes for a success. A foreground command reports
//! the tasks it waited for in its own result.
//!
//! Outcomes come from the task center's `task-entry` events, which carry
//! native tasks, carving scans and plugin jobs alike. Only tasks seen running
//! in this process are followed: a plugin job of an earlier process that the
//! scheduler marks offline when the case opens is not this process's work.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{native, CliError};
use crate::{
    app::workflows::task_center::TASK_ENTRY_EVENT,
    core::extensions::task_scheduler::SchedTaskStatus, state::AppState,
};

/// How long a wait lets the last events arrive once no task is registered.
const SETTLE: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(50);

/// Caseless session indexes are speculative caches of this process; a
/// command that needs one reports its failure itself.
const SPECULATIVE_KIND: &str = "sessionSnapshot";

/// One background task that ended, as waits report it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ended {
    pub id: String,
    /// The built-in task kind, or `plugin`.
    pub kind: String,
    /// What it worked on: paths, keywords and volumes, or the analyzer and
    /// its target.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subject: Vec<String>,
    pub status: SchedTaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Ended {
    /// Failed, or ended without reporting how.
    pub fn failed(&self) -> bool {
        matches!(
            self.status,
            SchedTaskStatus::Failed | SchedTaskStatus::Offline
        )
    }

    fn describe(&self) -> String {
        let mut name = self.kind.clone();
        for part in &self.subject {
            name.push(' ');
            name.push_str(part);
        }
        let reason = match (&self.error, self.status) {
            (Some(error), _) => error.as_str(),
            (None, SchedTaskStatus::Offline) => "结束时没有报告结果",
            (None, _) => "失败",
        };
        format!("{name}（{}）：{reason}", self.id)
    }
}

/// The background tasks that ended since the previous report.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    /// Oldest first.
    pub tasks: Vec<Ended>,
}

impl Report {
    /// The report, or `tasksFailed` naming every task that failed; `lead`
    /// says what happened besides the failures.
    pub fn verdict(self, lead: Option<&str>) -> Result<Self, CliError> {
        let failed: Vec<String> = self
            .tasks
            .iter()
            .filter(|task| task.failed())
            .map(|task| format!("- {}", task.describe()))
            .collect();
        if failed.is_empty() {
            return Ok(self);
        }
        let heading = format!("{} 个后台任务失败：", failed.len());
        let heading = match lead {
            Some(lead) => format!("{lead}；{heading}"),
            None => heading,
        };
        let mut error = CliError::new("tasksFailed", format!("{heading}\n{}", failed.join("\n")));
        error.details = Some(serde_json::to_value(&self)?);
        Err(error)
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum EntryEvent {
    Changed {
        entry: Entry,
    },
    Removed {
        id: String,
    },
    #[serde(other)]
    Other,
}

/// The fields of a task center entry that tell how its task stands.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    id: String,
    status: SchedTaskStatus,
    #[serde(default)]
    automatic: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(flatten)]
    source: Source,
}

#[derive(Deserialize)]
#[serde(tag = "source", rename_all = "camelCase")]
enum Source {
    Plugin {
        job: Job,
    },
    Builtin {
        kind: String,
        #[serde(default)]
        view: View,
    },
}

#[derive(Deserialize)]
struct Job {
    /// `<plugin id>/<analyzer>`.
    name: String,
    #[serde(default)]
    params: JobParams,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JobParams {
    #[serde(default)]
    target_label: Option<String>,
}

#[derive(Default, Deserialize)]
struct View {
    #[serde(default)]
    subject: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Running,
    /// Paused, or a plugin job the scheduler is about to queue again or fail.
    Suspended,
    Ended,
}

fn phase(status: SchedTaskStatus, plugin: bool) -> Phase {
    match status {
        SchedTaskStatus::Created | SchedTaskStatus::Queued | SchedTaskStatus::Running => {
            Phase::Running
        }
        SchedTaskStatus::Success | SchedTaskStatus::Failed | SchedTaskStatus::Cancel => {
            Phase::Ended
        }
        SchedTaskStatus::Offline if plugin => Phase::Suspended,
        SchedTaskStatus::Offline => Phase::Ended,
        SchedTaskStatus::Pause => Phase::Suspended,
    }
}

struct Tracked {
    task: Ended,
    phase: Phase,
    /// Outlives the command that started it, or the application started it.
    background: bool,
}

#[derive(Default)]
struct Tracker {
    /// Foreground commands running now.
    foreground: usize,
    /// A foreground command of the current busy period failed.
    period_failed: bool,
    tasks: HashMap<String, Tracked>,
    /// Ended background tasks not reported yet, oldest first.
    ended: Vec<Ended>,
}

impl Tracker {
    fn observe(&mut self, entry: Entry) {
        let (kind, subject, plugin) = match entry.source {
            Source::Builtin { kind, .. } if kind == SPECULATIVE_KIND => return,
            Source::Builtin { kind, view } => (kind, view.subject, false),
            Source::Plugin { job } => {
                let subject = std::iter::once(job.name)
                    .chain(job.params.target_label)
                    .collect();
                ("plugin".to_owned(), subject, true)
            }
        };
        let phase = phase(entry.status, plugin);
        let idle = self.foreground == 0;
        let task = Ended {
            id: entry.id.clone(),
            kind,
            subject,
            status: entry.status,
            error: entry.error,
        };
        match self.tasks.get_mut(&entry.id) {
            Some(tracked) => {
                if tracked.phase == Phase::Ended && phase != Phase::Ended {
                    // Another run under the same id.
                    tracked.background = idle;
                }
                tracked.background |= entry.automatic;
                tracked.phase = phase;
                tracked.task = Ended {
                    subject: if task.subject.is_empty() {
                        std::mem::take(&mut tracked.task.subject)
                    } else {
                        task.subject
                    },
                    ..task
                };
            }
            // Ended or suspended before this process saw it run.
            None if phase != Phase::Running => return,
            None => {
                self.tasks.insert(
                    entry.id.clone(),
                    Tracked {
                        task,
                        phase,
                        background: idle || entry.automatic,
                    },
                );
            }
        }
        if phase == Phase::Ended
            && self
                .tasks
                .get(&entry.id)
                .is_some_and(|tracked| tracked.background)
        {
            self.report(&entry.id);
        }
    }

    fn report(&mut self, id: &str) {
        if let Some(tracked) = self.tasks.remove(id) {
            self.ended.push(tracked.task);
        }
    }

    /// The busy period ended: tasks still running outlive their command,
    /// and a task that failed under a command that succeeded is reported.
    fn settle(&mut self) {
        let failed = std::mem::take(&mut self.period_failed);
        let foreground: Vec<String> = self
            .tasks
            .iter()
            .filter(|(_, tracked)| !tracked.background)
            .map(|(id, _)| id.clone())
            .collect();
        for id in foreground {
            let Some(tracked) = self.tasks.get_mut(&id) else {
                continue;
            };
            match tracked.phase {
                Phase::Running | Phase::Suspended => tracked.background = true,
                Phase::Ended if !failed && tracked.task.failed() => self.report(&id),
                Phase::Ended => {
                    self.tasks.remove(&id);
                }
            }
        }
    }

    fn running(&self) -> bool {
        self.tasks
            .values()
            .any(|tracked| tracked.phase == Phase::Running)
    }

    /// Tasks still shown running although nothing is registered any more
    /// ended without the event that says how.
    fn abandon_running(&mut self) {
        let running: Vec<String> = self
            .tasks
            .iter()
            .filter(|(_, tracked)| tracked.phase == Phase::Running)
            .map(|(id, _)| id.clone())
            .collect();
        for id in running {
            let Some(tracked) = self.tasks.get_mut(&id) else {
                continue;
            };
            tracked.phase = Phase::Ended;
            tracked.task.status = SchedTaskStatus::Offline;
            tracked.task.error = Some("任务已结束，但没有报告结果".to_owned());
            if tracked.background {
                self.report(&id);
            }
        }
    }
}

/// The background work of one CLI process, shared by its host and executor.
#[derive(Clone, Default)]
pub struct Background(Arc<Mutex<Tracker>>);

impl Background {
    fn lock(&self) -> MutexGuard<'_, Tracker> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Follows a host event; only task center changes matter.
    pub fn observe(&self, event: &str, payload: &Value) {
        if event != TASK_ENTRY_EVENT {
            return;
        }
        match EntryEvent::deserialize(payload) {
            Ok(EntryEvent::Changed { entry }) => self.lock().observe(entry),
            Ok(EntryEvent::Removed { id }) => {
                let mut tracker = self.lock();
                tracker.tasks.remove(&id);
                tracker.ended.retain(|task| task.id != id);
            }
            Ok(EntryEvent::Other) => {}
            Err(error) => tracing::debug!("task entry event not followed: {error}"),
        }
    }

    /// Marks a foreground command as running until [`Foreground::finish`]:
    /// tasks it starts belong to its result unless they outlive it, or fail
    /// although it succeeds.
    pub fn foreground(&self) -> Foreground<'_> {
        let mut tracker = self.lock();
        if tracker.foreground == 0 {
            tracker.period_failed = false;
        }
        tracker.foreground += 1;
        Foreground {
            background: self,
            succeeded: false,
        }
    }

    /// The ended background tasks not reported yet, without waiting.
    pub fn take(&self) -> Report {
        Report {
            tasks: std::mem::take(&mut self.lock().ended),
        }
    }

    /// Waits until no task is active, then reports the background tasks
    /// that ended.
    pub async fn wait(&self, state: &AppState) -> Result<Report, CliError> {
        let mut quiet_since = None;
        loop {
            if native::busy(state).await? {
                quiet_since = None;
            } else if !self.lock().running() {
                break;
            } else if quiet_since.get_or_insert_with(Instant::now).elapsed() >= SETTLE {
                self.lock().abandon_running();
                break;
            }
            tokio::time::sleep(POLL).await;
        }
        Ok(self.take())
    }
}

/// A foreground command in progress. A command dropped unfinished counts as
/// failed.
pub struct Foreground<'a> {
    background: &'a Background,
    succeeded: bool,
}

impl Foreground<'_> {
    /// The command ended with `result`. When it succeeded, a task it started
    /// that failed is reported as background work, since no result carried it.
    pub fn finish<T>(mut self, result: &Result<T, CliError>) {
        self.succeeded = result.is_ok();
    }
}

impl Drop for Foreground<'_> {
    fn drop(&mut self) {
        let mut tracker = self.background.lock();
        if !self.succeeded {
            tracker.period_failed = true;
        }
        tracker.foreground = tracker.foreground.saturating_sub(1);
        if tracker.foreground == 0 {
            tracker.settle();
        }
    }
}

#[cfg(test)]
mod tests;
