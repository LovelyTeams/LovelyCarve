//! 看门狗,由调度循环周期调用。
//!
//! 每轮扫描 `status=running` 的任务:
//! - 进度更新超时("found update timeout task:")→ 置 `failed`,终止 worker,
//!   推送 `tasktimeout` 通知;
//! - worker 进程离线("found offline task:")→ 置 `offline`,推送 `taskoffline`
//!   通知,交离线处理器分流(`auto` 重排队 / 手动判失败);
//! - 台账里 `running` 却没有进程内登记的孤儿任务(进程异常路径)→ 同离线。
//!
//! 只终止判定超时/离线的那次执行的 worker;同时运行的其他任务不受影响。
//! 状态改写只从 `running` 迁出,与同时到达的 commit 竞争时以先写者为准。

use std::{sync::Arc, time::Instant};

use super::{
    model::{SchedNoticeKind, SchedTaskRecord, SchedTaskStatus},
    scheduler::{BoundCase, TaskScheduler},
};

#[derive(Clone, Debug)]
struct RunningSnapshot {
    task_id: i64,
    seq: u64,
    started_at: Instant,
    last_update: Instant,
}

#[derive(Clone, Copy, Debug)]
enum Verdict {
    UpdateTimeout,
    Offline,
}

impl TaskScheduler {
    pub async fn watchdog_tick(&self) {
        let _guard = self.inner.tick_lock.lock().await;
        let Some(bound) = self.bound() else {
            return;
        };
        let snapshot: Vec<RunningSnapshot> = self
            .inner
            .running
            .lock()
            .iter()
            .map(|(task_id, entry)| RunningSnapshot {
                task_id: *task_id,
                seq: entry.seq,
                started_at: entry.started_at,
                last_update: entry.last_update,
            })
            .collect();
        let rows = self
            .on_blocking({
                let bound = Arc::clone(&bound);
                move |scheduler| scheduler.running_rows(&bound, snapshot)
            })
            .await;
        let rows = match rows {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!("watchdog: {error}");
                return;
            }
        };
        let now = Instant::now();
        for (entry, task) in rows {
            let verdict =
                if now.duration_since(entry.last_update) >= self.inner.config.update_timeout {
                    tracing::warn!(
                        "found update timeout task: tid {} name {}",
                        task.id,
                        task.name
                    );
                    Verdict::UpdateTimeout
                } else if now.duration_since(entry.started_at) >= self.inner.config.offline_timeout
                    && !bound.runner.worker_alive(task.id, entry.seq).await
                {
                    tracing::warn!("found offline task: tid {} name {}", task.id, task.name);
                    Verdict::Offline
                } else {
                    continue;
                };
            self.cancel_execution(&bound, &task, entry.seq).await;
            let settled = self
                .on_blocking({
                    let bound = Arc::clone(&bound);
                    move |scheduler| scheduler.settle_verdict(&bound, &task, verdict)
                })
                .await;
            if let Err(error) = settled {
                tracing::warn!("watchdog: {error}");
            }
        }
        let swept = self
            .on_blocking({
                let bound = Arc::clone(&bound);
                move |scheduler| scheduler.sweep_orphans(&bound)
            })
            .await;
        if let Err(error) = swept {
            tracing::warn!("watchdog: {error}");
        }
    }

    /// 快照中仍为 `running` 的任务行;已删除或已结束的执行顺手移除登记。
    fn running_rows(
        &self,
        bound: &BoundCase,
        snapshot: Vec<RunningSnapshot>,
    ) -> Vec<(RunningSnapshot, SchedTaskRecord)> {
        let mut rows = Vec::with_capacity(snapshot.len());
        for entry in snapshot {
            let task = match bound.store.get(entry.task_id) {
                Ok(Some(task)) => task,
                Ok(None) => {
                    self.remove_running(entry.task_id, entry.seq);
                    continue;
                }
                Err(error) => {
                    tracing::warn!("watchdog: query task {}: {error:#}", entry.task_id);
                    continue;
                }
            };
            if task.status != SchedTaskStatus::Running {
                // queued 尚在拉起;终态/暂停由控制面负责清理。
                let publication = self.publication_gate(entry.task_id);
                let idle = publication.as_ref().and_then(|gate| gate.try_lock());
                if !task.status.is_active() && idle.is_some() {
                    self.remove_running(entry.task_id, entry.seq);
                }
                continue;
            }
            rows.push((entry, task));
        }
        rows
    }

    /// 撤销这次执行(不触碰任务状态):撤销 token、移除登记,终止它的插件
    /// 实例。
    async fn cancel_execution(&self, bound: &Arc<BoundCase>, task: &SchedTaskRecord, seq: u64) {
        let task_id = task.id;
        let withdrawn = self
            .on_blocking(move |scheduler| {
                let publication = scheduler.publication_gate(task_id);
                let _publication = publication.as_ref().map(|gate| gate.lock());
                let entry = {
                    let mut running = scheduler.inner.running.lock();
                    if running.get(&task_id).is_some_and(|entry| entry.seq == seq) {
                        running.remove(&task_id)
                    } else {
                        None
                    }
                };
                if let Some(entry) = entry {
                    entry.token.cancel();
                }
            })
            .await;
        if let Err(error) = withdrawn {
            tracing::warn!("withdraw timed out task {task_id}: {error}");
        }
        bound.runner.terminate_worker(task.id, seq).await;
    }

    fn settle_verdict(&self, bound: &BoundCase, task: &SchedTaskRecord, verdict: Verdict) {
        match verdict {
            Verdict::UpdateTimeout => match self.write_task(|| {
                bound.store.transition(
                    task.id,
                    &[SchedTaskStatus::Running],
                    SchedTaskStatus::Failed,
                    Some("进度更新超时,worker 无响应"),
                )
            }) {
                Ok(Some(failed)) => {
                    self.emit_notice(
                        SchedNoticeKind::TaskTimeout,
                        Some(failed.id),
                        Some(failed.evidence_id),
                        format!("任务 {} 进度更新超时,已判定失败", failed.display_name),
                    );
                    self.after_finish(bound, &failed);
                }
                Ok(None) => {}
                Err(error) => tracing::warn!("watchdog: fail task {}: {error:#}", task.id),
            },
            Verdict::Offline => self.mark_offline(bound, task.id, "worker 进程离线"),
        }
    }

    fn mark_offline(&self, bound: &BoundCase, task_id: i64, reason: &str) {
        let offline = self.write_task(|| {
            bound.store.transition(
                task_id,
                &[SchedTaskStatus::Running],
                SchedTaskStatus::Offline,
                Some(reason),
            )
        });
        match offline {
            Ok(Some(offline)) => {
                self.emit_notice(
                    SchedNoticeKind::TaskOffline,
                    Some(offline.id),
                    Some(offline.evidence_id),
                    format!("任务 {} 的 worker 已离线", offline.display_name),
                );
                self.handle_offline(bound, &offline);
            }
            Ok(None) => {}
            Err(error) => tracing::warn!("watchdog: set task {task_id} offline: {error:#}"),
        }
    }

    /// 台账 `running` 但进程内无登记且开始时间已超过离线阈值 → 离线。
    fn sweep_orphans(&self, bound: &BoundCase) {
        let running_rows = match bound.store.list_by_status(SchedTaskStatus::Running) {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!("watchdog: list running tasks: {error:#}");
                return;
            }
        };
        for task in running_rows {
            if self.inner.running.lock().contains_key(&task.id) {
                continue;
            }
            let started_long_ago = task
                .start_at
                .as_deref()
                .and_then(|started| chrono::DateTime::parse_from_rfc3339(started).ok())
                .map(|started| {
                    let elapsed = chrono::Utc::now() - started.with_timezone(&chrono::Utc);
                    elapsed.to_std().unwrap_or_default() >= self.inner.config.offline_timeout
                })
                .unwrap_or(true);
            if !started_long_ago {
                continue;
            }
            tracing::warn!(
                "found offline task: tid {} name {} (no worker registration)",
                task.id,
                task.name
            );
            self.mark_offline(bound, task.id, "worker 登记丢失");
        }
    }
}
