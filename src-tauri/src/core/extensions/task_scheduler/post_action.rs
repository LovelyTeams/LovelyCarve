//! PostActionWatcher:检材任务完成后的防抖与收尾动作。
//!
//! 按检材登记 `PostActionEntry`;某检材的全部任务完成后启动防抖,防抖到期
//! 仍全部完成则执行台账 `task_followup.actions` 中的动作,成功后写
//! `done_at` 保证幂等;失败只记日志并保留未执行标记,下一轮全部完成时重试
//! ("action %q failed for eid=%d")。启动时 `init_from_db` 从台账恢复登记
//! ("initFromDB restored eid=%d create=%d finish=%d"),崩溃重启不丢。

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use serde::Deserialize;

use super::{
    model::{SchedNotice, SchedNoticeKind},
    store::TaskStore,
};

/// 默认防抖时长。
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_secs(5);

/// `task_followup.actions` 的一项。
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct PostAction {
    pub action: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// 传给动作处理器的上下文。
#[derive(Clone, Debug, PartialEq)]
pub struct PostActionRequest {
    pub evidence_id: i64,
    pub action: String,
    pub params: serde_json::Value,
}

/// 收尾动作处理器。实现者按 `request.action` 名注册,返回错误即视为该动作
/// 失败:watcher 记日志、推送 `PostActionFailed` 通知并保留重试机会;
/// 实现必须幂等,因为重试会再次调用。
pub trait PostActionHandler: Send + Sync {
    fn run(&self, request: &PostActionRequest) -> anyhow::Result<()>;
}

impl<F> PostActionHandler for F
where
    F: Fn(&PostActionRequest) -> anyhow::Result<()> + Send + Sync,
{
    fn run(&self, request: &PostActionRequest) -> anyhow::Result<()> {
        self(request)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct PostActionEntry {
    created: u64,
    finished: u64,
    debounce_deadline: Option<Instant>,
}

/// 一次到期执行的结果。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PostActionReport {
    pub evidence_id: i64,
    pub executed: Vec<String>,
    pub failed: Vec<(String, String)>,
    pub already_executed: bool,
}

pub struct PostActionWatcher {
    entries: Mutex<HashMap<i64, PostActionEntry>>,
    handlers: Mutex<HashMap<String, Arc<dyn PostActionHandler>>>,
    debounce: Duration,
}

impl std::fmt::Debug for PostActionWatcher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PostActionWatcher")
            .field("entries", &self.entries())
            .field("debounce", &self.debounce)
            .finish_non_exhaustive()
    }
}

impl PostActionWatcher {
    pub fn new(debounce: Duration) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            handlers: Mutex::new(HashMap::new()),
            debounce,
        }
    }

    fn entries(&self) -> MutexGuard<'_, HashMap<i64, PostActionEntry>> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn register_handler(
        &self,
        action: impl Into<String>,
        handler: impl PostActionHandler + 'static,
    ) {
        self.handlers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(action.into(), Arc::new(handler));
    }

    /// 清空登记(案件切换)。
    pub fn reset(&self) {
        self.entries().clear();
    }

    /// 从台账恢复登记;已全部完成且动作未执行的检材立即进入防抖。
    pub fn init_from_db(&self, store: &TaskStore) -> anyhow::Result<()> {
        let now = Instant::now();
        let mut restored = HashMap::new();
        for evidence_id in store.evidence_ids_with_tasks()? {
            let aggregate = store.evidence_task_status(evidence_id)?;
            let pending_actions = matches!(
                store.evidence_post_actions(evidence_id)?,
                Some((Some(_), false))
            );
            let debounce_deadline =
                (aggregate.all_finished() && pending_actions).then_some(now + self.debounce);
            tracing::info!(
                "PostActionWatcher initFromDB restored eid={evidence_id} create={} finish={}",
                aggregate.total,
                aggregate.finished()
            );
            restored.insert(
                evidence_id,
                PostActionEntry {
                    created: aggregate.total,
                    finished: aggregate.finished(),
                    debounce_deadline,
                },
            );
        }
        *self.entries() = restored;
        Ok(())
    }

    /// 新任务登记:创建计数 +1,取消进行中的防抖(还有任务没跑完)。
    pub fn on_task_created(&self, evidence_id: i64) {
        let mut entries = self.entries();
        let entry = entries.entry(evidence_id).or_default();
        entry.created += 1;
        entry.debounce_deadline = None;
    }

    /// 任务终结:完成计数 +1;以台账里的任务状态为准判断"全部完成"后启动防抖。
    pub fn on_task_finished(&self, store: &TaskStore, evidence_id: i64) -> anyhow::Result<()> {
        let aggregate = store.evidence_task_status(evidence_id)?;
        let mut entries = self.entries();
        let entry = entries.entry(evidence_id).or_default();
        entry.finished += 1;
        entry.created = entry.created.max(aggregate.total);
        entry.finished = entry.finished.min(entry.created);
        if aggregate.all_finished() {
            tracing::info!(
                "PostActionWatcher eid={evidence_id}: all tasks finished, debounce started"
            );
            entry.debounce_deadline = Some(Instant::now() + self.debounce);
        }
        Ok(())
    }

    /// 任务删除:计数同步减少,防止“全部完成”判定被幽灵任务卡住。
    pub fn on_task_removed(&self, store: &TaskStore, evidence_id: i64) -> anyhow::Result<()> {
        let aggregate = store.evidence_task_status(evidence_id)?;
        let mut entries = self.entries();
        if aggregate.total == 0 {
            entries.remove(&evidence_id);
            return Ok(());
        }
        let entry = entries.entry(evidence_id).or_default();
        entry.created = aggregate.total;
        entry.finished = aggregate.finished();
        Ok(())
    }

    /// 检材已从案件移除:丢弃它的登记。
    pub fn forget(&self, evidence_id: i64) {
        self.entries().remove(&evidence_id);
    }

    /// 防抖到期的检材(消费掉 deadline)。
    pub fn take_due(&self, now: Instant) -> Vec<i64> {
        let mut entries = self.entries();
        let mut due = entries
            .iter_mut()
            .filter(|(_, entry)| {
                entry
                    .debounce_deadline
                    .is_some_and(|deadline| deadline <= now)
            })
            .map(|(evidence_id, entry)| {
                entry.debounce_deadline = None;
                *evidence_id
            })
            .collect::<Vec<_>>();
        due.sort_unstable();
        due
    }

    /// 执行检材的收尾动作。调用前调用方需再次确认仍全部完成。
    pub fn execute(&self, store: &TaskStore, evidence_id: i64) -> anyhow::Result<PostActionReport> {
        let mut report = PostActionReport {
            evidence_id,
            ..PostActionReport::default()
        };
        let Some((actions_json, executed)) = store.evidence_post_actions(evidence_id)? else {
            return Ok(report);
        };
        if executed {
            report.already_executed = true;
            return Ok(report);
        }
        let actions: Vec<PostAction> = match actions_json {
            Some(text) if !text.trim().is_empty() => serde_json::from_str(&text)?,
            Some(_) | None => Vec::new(),
        };
        if actions.is_empty() {
            return Ok(report);
        }
        tracing::info!(
            "PostActionWatcher executing {} actions for eid={evidence_id}",
            actions.len()
        );
        let handlers = self
            .handlers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        for action in actions {
            let request = PostActionRequest {
                evidence_id,
                action: action.action.clone(),
                params: action.params,
            };
            let outcome = match handlers.get(&action.action) {
                Some(handler) => handler.run(&request),
                None => Err(anyhow::anyhow!("no handler registered for action")),
            };
            match outcome {
                Ok(()) => report.executed.push(action.action),
                Err(error) => {
                    tracing::warn!(
                        "PostActionWatcher action {:?} failed for eid={evidence_id}: {error:#}",
                        action.action
                    );
                    report.failed.push((action.action, format!("{error:#}")));
                }
            }
        }
        if report.failed.is_empty() {
            store.mark_post_action_executed(evidence_id)?;
        }
        Ok(report)
    }
}

impl PostActionReport {
    pub fn failure_notices(&self) -> Vec<SchedNotice> {
        self.failed
            .iter()
            .map(|(action, error)| SchedNotice {
                kind: SchedNoticeKind::PostActionFailed,
                task_id: None,
                evidence_id: Some(self.evidence_id),
                message: format!("收尾动作 {action} 执行失败：{error}"),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::extensions::task_scheduler::{
        model::SchedTaskStatus,
        store::tests::{insert_evidence, sample_task, temp_store},
    };

    #[test]
    fn debounce_starts_only_when_every_task_finished() {
        let (_dir, store) = temp_store();
        let evidence_id = insert_evidence(&store, "image-1");
        let watcher = PostActionWatcher::new(Duration::from_millis(0));
        let first = store
            .insert_task(&sample_task("org.test.plugin/a", evidence_id))
            .expect("insert")
            .into_record();
        watcher.on_task_created(evidence_id);
        let second = store
            .insert_task(&sample_task("org.test.plugin/b", evidence_id))
            .expect("insert")
            .into_record();
        watcher.on_task_created(evidence_id);

        store
            .update_status(first.id, SchedTaskStatus::Success, None)
            .expect("finish");
        watcher
            .on_task_finished(&store, evidence_id)
            .expect("finished");
        assert!(watcher.take_due(Instant::now()).is_empty());

        store
            .update_status(second.id, SchedTaskStatus::Failed, Some("x"))
            .expect("finish");
        watcher
            .on_task_finished(&store, evidence_id)
            .expect("finished");
        assert_eq!(watcher.take_due(Instant::now()), vec![evidence_id]);
        assert!(watcher.take_due(Instant::now()).is_empty());
    }

    #[test]
    fn new_task_cancels_pending_debounce() {
        let (_dir, store) = temp_store();
        let evidence_id = insert_evidence(&store, "image-1");
        let watcher = PostActionWatcher::new(Duration::from_secs(60));
        let task = store
            .insert_task(&sample_task("org.test.plugin/a", evidence_id))
            .expect("insert")
            .into_record();
        watcher.on_task_created(evidence_id);
        store
            .update_status(task.id, SchedTaskStatus::Success, None)
            .expect("finish");
        watcher
            .on_task_finished(&store, evidence_id)
            .expect("finished");
        watcher.on_task_created(evidence_id);
        assert!(watcher
            .take_due(Instant::now() + Duration::from_secs(120))
            .is_empty());
    }

    #[test]
    fn execute_runs_handlers_once_and_reports_failures() {
        let (_dir, store) = temp_store();
        let evidence_id = insert_evidence(&store, "image-1");
        store
            .set_evidence_post_actions(
                evidence_id,
                Some(r#"[{"action":"ok","params":{"n":1}},{"action":"missing"}]"#),
            )
            .expect("set actions");
        let watcher = PostActionWatcher::new(Duration::from_millis(0));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        watcher.register_handler("ok", move |request: &PostActionRequest| {
            sink.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(request.clone());
            Ok(())
        });

        let report = watcher.execute(&store, evidence_id).expect("execute");
        assert_eq!(report.executed, vec!["ok".to_owned()]);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].0, "missing");
        assert!(!report.already_executed);
        assert_eq!(report.failure_notices().len(), 1);
        assert_eq!(
            seen.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            &[PostActionRequest {
                evidence_id,
                action: "ok".to_owned(),
                params: serde_json::json!({"n": 1}),
            }]
        );
        // 有失败:未打幂等标记,下次仍会执行。
        assert_eq!(
            store
                .evidence_post_actions(evidence_id)
                .expect("actions")
                .map(|(_, executed)| executed),
            Some(false)
        );

        store
            .set_evidence_post_actions(evidence_id, Some(r#"[{"action":"ok"}]"#))
            .expect("set actions");
        let report = watcher.execute(&store, evidence_id).expect("execute");
        assert_eq!(report.executed, vec!["ok".to_owned()]);
        assert!(report.failed.is_empty());
        let repeat = watcher.execute(&store, evidence_id).expect("execute again");
        assert!(repeat.already_executed);
        assert!(repeat.executed.is_empty());
    }

    #[test]
    fn init_from_db_restores_counts_and_schedules_pending_actions() {
        let (_dir, store) = temp_store();
        let done_evidence = insert_evidence(&store, "image-done");
        let busy_evidence = insert_evidence(&store, "image-busy");
        let done = store
            .insert_task(&sample_task("org.test.plugin/a", done_evidence))
            .expect("insert")
            .into_record();
        store
            .update_status(done.id, SchedTaskStatus::Success, None)
            .expect("finish");
        store
            .set_evidence_post_actions(done_evidence, Some(r#"[{"action":"ok"}]"#))
            .expect("actions");
        store
            .insert_task(&sample_task("org.test.plugin/a", busy_evidence))
            .expect("insert");
        store
            .set_evidence_post_actions(busy_evidence, Some(r#"[{"action":"ok"}]"#))
            .expect("actions");

        let watcher = PostActionWatcher::new(Duration::from_millis(0));
        watcher.init_from_db(&store).expect("init");
        assert_eq!(watcher.take_due(Instant::now()), vec![done_evidence]);
        let entries = watcher.entries();
        assert_eq!(
            entries.get(&busy_evidence),
            Some(&PostActionEntry {
                created: 1,
                finished: 0,
                debounce_deadline: None,
            })
        );
    }
}
