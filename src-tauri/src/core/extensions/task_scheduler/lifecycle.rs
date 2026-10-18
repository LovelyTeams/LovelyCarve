//! 单条任务的状态推进:`queued → running` → 执行 → commit → 完成处理,以及
//! 进度回流与离线分流。
//!
//! 多条任务并行执行;每条任务的台账读写都在阻塞线程池上进行(见
//! `TaskScheduler::on_blocking`),插件的进度回报只改内存登记,由派发循环
//! 批量落库。插件实例只在进程内登记(执行器按 `(task_id, seq)` 定位),
//! 不落库:进程结束,实例也就不在了。

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use tokio_util::sync::CancellationToken;

use super::{
    dependence,
    executor::{ExecutionOutcome, TaskExecution, TaskProgress},
    model::{SchedEvent, SchedNoticeKind, SchedTaskRecord, SchedTaskStatus},
    scheduler::{BoundCase, TaskScheduler},
    store::InsertOutcome,
};
use crate::core::extensions::plugin::TaskTrigger;

/// 百分比未变时,进度最多每秒落库一次(刷新 `update_at` 与剩余时间)。
const PROGRESS_REFRESH: Duration = Duration::from_secs(1);

impl TaskScheduler {
    /// worker 侧的一次任务循环。`seq` 标识本次执行,commit 时只清理属于本次
    /// 执行的登记。
    pub(super) async fn run_task(
        &self,
        bound: Arc<BoundCase>,
        task: SchedTaskRecord,
        seq: u64,
        token: CancellationToken,
    ) {
        let task_id = task.id;
        let started = self
            .on_blocking({
                let bound = Arc::clone(&bound);
                move |scheduler| scheduler.start_execution(&bound, task, seq)
            })
            .await;
        let running = match started {
            Ok(Some(running)) => running,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!("run: start task {task_id}: {error}");
                self.remove_running(task_id, seq);
                return;
            }
        };
        let execution = TaskExecution {
            seq,
            token,
            progress: self.progress_reporter(task_id, seq),
        };
        let outcome = bound.runner.run(running, execution).await;
        let committed = self
            .on_blocking({
                let bound = Arc::clone(&bound);
                move |scheduler| scheduler.commit(&bound, task_id, seq, outcome)
            })
            .await;
        if let Err(error) = committed {
            tracing::warn!("commit task {task_id}: {error}");
            bound.runner.discard_execution(task_id, seq);
            self.remove_running(task_id, seq);
        }
    }

    /// `queued → running`。派发之后被取消/暂停/案件关闭的任务已离开
    /// `queued`,不再启动。状态迁移与"运行中"广播是一个有序步骤:取消要么
    /// 落在它之前(任务不再启动),要么落在广播之后。
    fn start_execution(
        &self,
        bound: &BoundCase,
        task: SchedTaskRecord,
        seq: u64,
    ) -> Option<SchedTaskRecord> {
        self.ordered(|| {
            match bound.store.transition(
                task.id,
                &[SchedTaskStatus::Queued],
                SchedTaskStatus::Running,
                None,
            ) {
                Ok(Some(running)) => {
                    self.emit_task(&running);
                    Some(running)
                }
                Ok(None) => {
                    tracing::debug!("run: task {} left queued before it started", task.id);
                    self.remove_running(task.id, seq);
                    None
                }
                Err(error) => {
                    tracing::warn!("run: mark task {} running: {error:#}", task.id);
                    self.remove_running(task.id, seq);
                    None
                }
            }
        })
    }

    /// 提交结果并处理任务完成。若期间已被取消/
    /// 暂停/判离线(登记被控制面/看门狗接管,或状态已不是 running),不改写
    /// 状态,只做进程内清理。登记保留到终态写入之后,看门狗的孤儿巡检不会
    /// 把正在提交的任务误判为离线。
    pub(super) fn commit(
        &self,
        bound: &BoundCase,
        task_id: i64,
        seq: u64,
        outcome: ExecutionOutcome,
    ) {
        let Some(publication) = self.publication_gate(task_id) else {
            bound.runner.discard_execution(task_id, seq);
            return;
        };
        let _publication = publication.lock();
        let owned = self
            .inner
            .running
            .lock()
            .get(&task_id)
            .is_some_and(|entry| entry.seq == seq);
        if !owned {
            tracing::debug!(
                "commit: execution {seq} of task {task_id} was superseded, result dropped"
            );
            bound.runner.discard_execution(task_id, seq);
            return;
        }
        let task = match bound.store.get(task_id) {
            Ok(Some(task)) if task.status == SchedTaskStatus::Running => task,
            other => {
                if let Err(error) = other {
                    tracing::warn!("read task before publication: {error:#}");
                }
                bound.runner.discard_execution(task_id, seq);
                self.remove_running(task_id, seq);
                return;
            }
        };
        let outcome = match outcome {
            ExecutionOutcome::Success(result) => {
                match bound.runner.publish_results(&task, seq, result) {
                    Ok(result) => ExecutionOutcome::Success(result),
                    Err(error) => ExecutionOutcome::Failed(format!("分析结果发布失败：{error:#}")),
                }
            }
            failed @ ExecutionOutcome::Failed(_) => failed,
        };
        let committed = self.write_task(|| match &outcome {
            ExecutionOutcome::Success(result) => {
                bound
                    .store
                    .complete(task_id, SchedTaskStatus::Success, Some(result), None)
            }
            ExecutionOutcome::Failed(error) => {
                bound
                    .store
                    .complete(task_id, SchedTaskStatus::Failed, None, Some(error))
            }
        });
        bound.runner.discard_execution(task_id, seq);
        if !matches!(&committed, Ok(Some(_))) {
            if let ExecutionOutcome::Success(result) = &outcome {
                let mut unpublished = task.clone();
                unpublished.result = Some(result.clone());
                if let Err(error) = bound.runner.remove_results(&unpublished) {
                    tracing::error!("rollback unpublished task {task_id} outputs: {error:#}");
                }
            }
        }
        match committed {
            Ok(Some(finished)) => self.after_finish(bound, &finished),
            Ok(None) => {
                tracing::debug!("commit: task {task_id} is no longer running, result dropped");
            }
            Err(error) => tracing::warn!("commit task error:{error:#}"),
        }
        // Keep the gate discoverable until follow-ups and bookkeeping finish,
        // so case close also drains the final publication work.
        self.remove_running(task_id, seq);
    }

    /// OnTaskFinish:PostActionWatcher 计数、依赖链建下一级任务。
    pub(super) fn after_finish(&self, bound: &BoundCase, finished: &SchedTaskRecord) {
        if finished.status.is_finished() {
            if let Err(error) = self
                .inner
                .post_actions
                .on_task_finished(&bound.store, finished.evidence_id)
            {
                tracing::warn!("PostActionWatcher on finish tid {}: {error:#}", finished.id);
            }
        }
        if finished.status != SchedTaskStatus::Success {
            return;
        }
        let registry = self.registry();
        let outcomes = self.ordered(|| {
            let outcomes = dependence::check_dependence(&bound.store, &registry, finished);
            for outcome in outcomes.iter().flatten() {
                if let InsertOutcome::Created(next) = outcome {
                    self.emit_task(next);
                }
            }
            outcomes
        });
        match outcomes {
            Ok(outcomes) => {
                for outcome in outcomes {
                    if let InsertOutcome::Created(next) = outcome {
                        self.inner.post_actions.on_task_created(next.evidence_id);
                    }
                }
            }
            Err(error) => tracing::warn!(
                "CheckDependenceTask task {} error: {error:#}",
                finished.name
            ),
        }
    }

    /// 离线处理器:自动任务重排队,手动任务判失败。
    pub(super) fn handle_offline(&self, bound: &BoundCase, task: &SchedTaskRecord) {
        let offline = [SchedTaskStatus::Offline];
        let outcome = self.write_task(|| match task.info.trigger {
            TaskTrigger::Auto => {
                bound
                    .store
                    .transition(task.id, &offline, SchedTaskStatus::Created, None)
            }
            TaskTrigger::Manually => bound.store.transition(
                task.id,
                &offline,
                SchedTaskStatus::Failed,
                Some(
                    task.info
                        .error
                        .as_deref()
                        .unwrap_or("receive request offline"),
                ),
            ),
        });
        match outcome {
            Ok(Some(updated)) if updated.status.is_finished() => {
                self.after_finish(bound, &updated);
            }
            Ok(Some(_) | None) => {}
            Err(error) => tracing::warn!("offline handler tid {}: {error:#}", task.id),
        }
    }

    /// 推进防抖到期的检材:再次确认全部完成后广播并执行收尾动作。
    pub(super) fn run_due_post_actions(&self, bound: &BoundCase) {
        for evidence_id in self.inner.post_actions.take_due(Instant::now()) {
            let aggregate = match bound.store.evidence_task_status(evidence_id) {
                Ok(aggregate) => aggregate,
                Err(error) => {
                    tracing::warn!("post action aggregate eid={evidence_id}: {error:#}");
                    continue;
                }
            };
            if !aggregate.all_finished() {
                continue;
            }
            self.emit_notice(
                SchedNoticeKind::AllTasksFinished,
                None,
                Some(evidence_id),
                format!(
                    "检材 {evidence_id} 的 {} 个任务已全部完成（成功 {}，失败 {}，取消 {}）",
                    aggregate.total, aggregate.success, aggregate.failed, aggregate.cancel
                ),
            );
            match self.inner.post_actions.execute(&bound.store, evidence_id) {
                Ok(report) => {
                    for notice in report.failure_notices() {
                        self.emit(SchedEvent::Notice { notice });
                    }
                }
                Err(error) => tracing::warn!("post actions eid={evidence_id}: {error:#}"),
            }
        }
    }

    /// 一次执行的进度回报通道。只持有调度器的弱
    /// 引用:插件实例比任务活得久时不延长调度器的生命周期。
    fn progress_reporter(&self, task_id: i64, seq: u64) -> TaskProgress {
        let inner = Arc::downgrade(&self.inner);
        Arc::new(move |current, total| {
            if let Some(inner) = inner.upgrade() {
                TaskScheduler { inner }.on_task_progress(task_id, seq, current, total);
            }
        })
    }

    /// 插件 `host.progress` 回流到所属执行:立即刷新看门狗的 `last_update`
    /// 并记下最新百分比;落库由 [`Self::flush_progress`] 完成,回报的插件线程
    /// 不会等待台账。
    pub(super) fn on_task_progress(
        &self,
        task_id: i64,
        seq: u64,
        current: u64,
        total: Option<u64>,
    ) {
        let mut running = self.inner.running.lock();
        let Some(entry) = running.get_mut(&task_id).filter(|entry| entry.seq == seq) else {
            return;
        };
        entry.last_update = Instant::now();
        if let Some(total) = total.filter(|total| *total > 0) {
            entry.progress = percent(current, total);
        }
        entry.progress_reported = true;
    }

    /// 把各执行最新的进度写入台账(派发循环在阻塞线程池上调用)。百分比
    /// 变化即写,未变化时按 1 s 节流;`update_progress` 只改仍为 running 的
    /// 行,迟到的落库不会改写已结束的任务。
    pub(super) fn flush_progress(&self, bound: &BoundCase) {
        let now = Instant::now();
        let due: Vec<(i64, i64, Option<i64>)> = self
            .inner
            .running
            .lock()
            .iter_mut()
            .filter_map(|(task_id, entry)| {
                if !entry.progress_reported {
                    return None;
                }
                let changed = entry.progress != entry.last_written_progress;
                if !changed && now.duration_since(entry.last_write_at) < PROGRESS_REFRESH {
                    return None;
                }
                entry.progress_reported = false;
                entry.last_written_progress = entry.progress;
                entry.last_write_at = now;
                let elapsed = i64::try_from(now.duration_since(entry.started_at).as_secs())
                    .unwrap_or(i64::MAX);
                Some((
                    *task_id,
                    entry.progress,
                    remaining_seconds(elapsed, entry.progress),
                ))
            })
            .collect();
        for (task_id, progress, remain) in due {
            let written =
                self.write_task(|| bound.store.update_progress(task_id, progress, remain));
            if let Err(error) = written {
                tracing::debug!("progress tid {task_id}: {error:#}");
            }
        }
    }
}

/// `current / total` 的整数百分比(u128 计算,超大字节数不会溢出)。
fn percent(current: u64, total: u64) -> i64 {
    let percent = u128::from(current.min(total)) * 100 / u128::from(total);
    i64::try_from(percent).unwrap_or(100)
}

/// 按进度线性外推的剩余秒数。
fn remaining_seconds(elapsed: i64, progress: i64) -> Option<i64> {
    if progress >= 100 {
        Some(0)
    } else if progress > 0 {
        Some(elapsed.saturating_mul(100 - progress) / progress)
    } else {
        None
    }
}
