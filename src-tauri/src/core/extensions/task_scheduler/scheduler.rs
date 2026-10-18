//! 任务调度器核心:案件绑定与崩溃恢复、调度循环与派发。
//!
//! 分工:本文件持有状态并推进 `created → queued`;`lifecycle.rs` 负责单条
//! 任务的 `queued → running → commit → OnTaskFinish` 与进度回流;`control.rs`
//! 是外部控制面(任务生命周期操作、证据上下线);
//! `watchdog.rs` 是周期巡检(离线/进度超时);`config.rs` 是参数与错误类型。
//!
//! 并发模型:每条运行中的任务都有自己的插件 worker,
//! 同一插件的多条任务也并行执行;派发循环按创建顺序
//! 派发,同时处于 queued/running 的任务不超过 `SchedulerConfig::max_running`。
//! 进度经每次执行自己的回报通道到达,按 `(task_id, seq)` 定位。
//!
//! 台账读写一律在阻塞线程池上进行([`TaskScheduler::on_blocking`]):台账
//! 忙或被其他连接锁住时,等待的只是那一步,不会卡住异步运行时上的 IPC 命令。

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};

use tokio_util::sync::CancellationToken;

use super::{
    config::{SchedError, SchedResult, SchedulerConfig},
    executor::TaskRunner,
    functions::FunctionRegistry,
    model::{SchedEvent, SchedNotice, SchedNoticeKind, SchedTaskRecord, SchedTaskStatus},
    post_action::{PostActionHandler, PostActionWatcher},
    store::TaskStore,
};

pub type RegistrySource = Arc<dyn Fn() -> FunctionRegistry + Send + Sync>;
pub type RunnerFactory = Arc<dyn Fn(&Path) -> Arc<dyn TaskRunner> + Send + Sync>;
type EventSink = Arc<dyn Fn(SchedEvent) + Send + Sync>;

pub(super) struct BoundCase {
    pub(super) store: Arc<TaskStore>,
    pub(super) runner: Arc<dyn TaskRunner>,
}

/// 进程内的运行句柄。`seq` 区分同一任务
/// 的不同次运行:任务被判离线重排队后,旧执行的 commit 不能删掉新登记。
pub(super) struct RunningTask {
    pub(super) seq: u64,
    pub(super) token: CancellationToken,
    /// Serializes publication + terminal state with withdrawal of this execution.
    pub(super) publication: Arc<parking_lot::Mutex<()>>,
    pub(super) started_at: Instant,
    /// 最近一次进度回报(看门狗的进度超时计时)。
    pub(super) last_update: Instant,
    /// 插件最近回报的百分比,尚未必落库。
    pub(super) progress: i64,
    /// 上次落库之后是否又有回报。
    pub(super) progress_reported: bool,
    pub(super) last_written_progress: i64,
    pub(super) last_write_at: Instant,
}

/// 本轮派发出去、等待启动执行的任务。
struct Dispatched {
    task: SchedTaskRecord,
    seq: u64,
    token: CancellationToken,
}

pub(super) struct SchedulerInner {
    pub(super) config: SchedulerConfig,
    registry_source: RegistrySource,
    runner_factory: RunnerFactory,
    bound: parking_lot::Mutex<Option<Arc<BoundCase>>>,
    pub(super) running: parking_lot::Mutex<HashMap<i64, RunningTask>>,
    run_seq: AtomicU64,
    pub(super) post_actions: PostActionWatcher,
    event_sink: parking_lot::Mutex<Option<EventSink>>,
    loops_started: AtomicBool,
    /// 派发/巡检/控制面互斥,避免同一任务被并发改写。
    pub(super) tick_lock: tokio::sync::Mutex<()>,
    /// 任务行写入与其广播的顺序锁,见 [`TaskScheduler::ordered`]。
    row_events: parking_lot::Mutex<()>,
}

#[derive(Clone)]
pub struct TaskScheduler {
    pub(super) inner: Arc<SchedulerInner>,
}

impl TaskScheduler {
    pub fn new(
        config: SchedulerConfig,
        registry_source: RegistrySource,
        runner_factory: RunnerFactory,
    ) -> Self {
        let post_actions = PostActionWatcher::new(config.post_action_debounce);
        Self {
            inner: Arc::new(SchedulerInner {
                config,
                registry_source,
                runner_factory,
                bound: parking_lot::Mutex::new(None),
                running: parking_lot::Mutex::new(HashMap::new()),
                run_seq: AtomicU64::new(0),
                post_actions,
                event_sink: parking_lot::Mutex::new(None),
                loops_started: AtomicBool::new(false),
                tick_lock: tokio::sync::Mutex::new(()),
                row_events: parking_lot::Mutex::new(()),
            }),
        }
    }

    pub fn set_event_sink(&self, sink: impl Fn(SchedEvent) + Send + Sync + 'static) {
        *self.inner.event_sink.lock() = Some(Arc::new(sink));
    }

    pub fn register_post_action_handler(
        &self,
        action: impl Into<String>,
        handler: impl PostActionHandler + 'static,
    ) {
        self.inner.post_actions.register_handler(action, handler);
    }

    pub fn registry(&self) -> FunctionRegistry {
        (self.inner.registry_source)()
    }

    pub fn is_bound(&self) -> bool {
        self.inner.bound.lock().is_some()
    }

    pub(super) fn bound(&self) -> Option<Arc<BoundCase>> {
        self.inner.bound.lock().clone()
    }

    pub(super) fn require_bound(&self) -> SchedResult<Arc<BoundCase>> {
        self.bound().ok_or(SchedError::NoCase)
    }

    pub(super) fn emit(&self, event: SchedEvent) {
        let sink = self.inner.event_sink.lock().clone();
        if let Some(sink) = sink {
            sink(event);
        }
    }

    pub(super) fn emit_task(&self, task: &SchedTaskRecord) {
        self.emit(SchedEvent::TaskChanged {
            task: Box::new(task.clone()),
        });
    }

    /// 把一次任务行写入与它引发的广播作为一步执行。任务行由多方并发改写
    /// (各执行的启动与提交、派发循环的进度落库、控制面、看门狗);从写库到
    /// 广播都持有同一把锁,广播顺序就等于写入顺序,前端不会在收到较新的
    /// 状态之后又收到较旧的(例如"成功"之后的"运行中 100%")。不可嵌套调用。
    pub(super) fn ordered<T>(&self, step: impl FnOnce() -> T) -> T {
        let _ordered = self.inner.row_events.lock();
        step()
    }

    /// [`Self::ordered`] 写入一行,并广播写入后的行。
    pub(super) fn write_task(
        &self,
        write: impl FnOnce() -> anyhow::Result<Option<SchedTaskRecord>>,
    ) -> anyhow::Result<Option<SchedTaskRecord>> {
        self.ordered(|| {
            let written = write();
            if let Ok(Some(task)) = &written {
                self.emit_task(task);
            }
            written
        })
    }

    pub(super) fn emit_notice(
        &self,
        kind: SchedNoticeKind,
        task_id: Option<i64>,
        evidence_id: Option<i64>,
        message: impl Into<String>,
    ) {
        self.emit(SchedEvent::Notice {
            notice: SchedNotice {
                kind,
                task_id,
                evidence_id,
                message: message.into(),
            },
        });
    }

    /// 在阻塞线程池上执行一段调度器簿记。凡是读写台账的步骤都经由这里
    /// (或本就运行在阻塞线程上),台账忙时只阻塞这一步,不占用异步工作线程。
    pub(super) async fn on_blocking<T, F>(&self, work: F) -> SchedResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&TaskScheduler) -> T + Send + 'static,
    {
        let scheduler = self.clone();
        tokio::task::spawn_blocking(move || work(&scheduler))
            .await
            .map_err(|error| {
                SchedError::Store(anyhow::anyhow!(
                    "task scheduler database work failed: {error}"
                ))
            })
    }

    // ----- case binding -------------------------------------------------------

    /// 绑定案件:打开台账上的任务表,做崩溃恢复(遗留 `queued`/`running` →
    /// `offline` → 离线处理器分流),恢复 PostActionWatcher 登记,并启动调度循环。
    pub async fn bind_case(&self, ledger_path: &Path, case_dir: PathBuf) -> anyhow::Result<()> {
        if self.is_bound() {
            self.unbind_case().await;
        }
        let ledger_path = ledger_path.to_owned();
        let store = tokio::task::spawn_blocking(move || TaskStore::open(&ledger_path))
            .await
            .map_err(|error| anyhow::anyhow!("open task store: {error}"))??;
        let runner = (self.inner.runner_factory)(&case_dir);
        let bound = Arc::new(BoundCase {
            store: Arc::new(store),
            runner,
        });
        let _guard = self.inner.tick_lock.lock().await;
        self.on_blocking(move |scheduler| scheduler.recover(&bound))
            .await??;
        drop(_guard);
        self.ensure_loops();
        Ok(())
    }

    /// 崩溃恢复并登记为当前案件(`bind_case` 在阻塞线程上调用)。
    fn recover(&self, bound: &Arc<BoundCase>) -> anyhow::Result<()> {
        let offline = bound
            .store
            .reset_active_to_offline("上次会话异常结束,worker 已丢失")?;
        if !offline.is_empty() {
            tracing::info!(
                "task scheduler recovery: {} tasks set offline",
                offline.len()
            );
        }
        *self.inner.bound.lock() = Some(Arc::clone(bound));
        self.inner.post_actions.reset();
        for task in offline {
            self.emit_task(&task);
            self.handle_offline(bound, &task);
        }
        self.inner.post_actions.init_from_db(&bound.store)?;
        Ok(())
    }

    /// 解绑案件:终止运行中的执行,任务置 `offline`(重开案件时由恢复流程
    /// 重排队或判失败),清空进程内登记。
    pub async fn unbind_case(&self) {
        let _guard = self.inner.tick_lock.lock().await;
        let Some(bound) = self.inner.bound.lock().take() else {
            return;
        };
        let executions: Vec<(i64, u64, Arc<parking_lot::Mutex<()>>)> = self
            .inner
            .running
            .lock()
            .iter()
            .map(|(id, entry)| (*id, entry.seq, Arc::clone(&entry.publication)))
            .collect();
        let settled = self
            .on_blocking({
                let bound = Arc::clone(&bound);
                let executions = executions.clone();
                move |scheduler| {
                    for (task_id, seq, publication) in executions {
                        let _publication = publication.lock();
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
                        let offline = scheduler.write_task(|| {
                            bound.store.transition(
                                task_id,
                                &[SchedTaskStatus::Queued, SchedTaskStatus::Running],
                                SchedTaskStatus::Offline,
                                Some("案件已关闭"),
                            )
                        });
                        if let Err(error) = offline {
                            tracing::warn!("unbind: set task {task_id} offline: {error:#}");
                        }
                    }
                }
            })
            .await;
        if let Err(error) = settled {
            tracing::warn!("unbind: {error}");
        }
        for (task_id, seq, _) in executions {
            bound.runner.terminate_worker(task_id, seq).await;
        }
        self.inner.post_actions.reset();
    }

    /// 启动派发与看门狗循环(进程生命周期内只启动一次;首次 tick 延后一个
    /// 周期,避免绑定尚未收尾就派发)。
    fn ensure_loops(&self) {
        if self.inner.loops_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let dispatcher = self.clone();
        tokio::spawn(async move {
            let period = dispatcher.inner.config.dispatch_interval;
            let mut interval =
                tokio::time::interval_at(tokio::time::Instant::now() + period, period);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                dispatcher.dispatch_tick().await;
            }
        });
        let watchdog = self.clone();
        tokio::spawn(async move {
            let period = watchdog.inner.config.check_interval;
            let mut interval =
                tokio::time::interval_at(tokio::time::Instant::now() + period, period);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                watchdog.watchdog_tick().await;
            }
        });
    }

    // ----- dispatch -----------------------------------------------------------

    /// 一轮派发:
    /// 落库最新进度,按创建顺序把 `created` 任务派发出去(`created → queued`,
    /// 不超过 `max_running`),并推进到期的收尾动作。
    pub async fn dispatch_tick(&self) {
        let _guard = self.inner.tick_lock.lock().await;
        let Some(bound) = self.bound() else {
            return;
        };
        let dispatched = self
            .on_blocking({
                let bound = Arc::clone(&bound);
                move |scheduler| scheduler.dispatch_created(&bound)
            })
            .await;
        let dispatched = match dispatched {
            Ok(dispatched) => dispatched,
            Err(error) => {
                tracing::warn!("dispatch: {error}");
                return;
            }
        };
        for Dispatched { task, seq, token } in dispatched {
            let scheduler = self.clone();
            let bound = Arc::clone(&bound);
            tokio::spawn(async move {
                scheduler.run_task(bound, task, seq, token).await;
            });
        }
    }

    fn dispatch_created(&self, bound: &BoundCase) -> Vec<Dispatched> {
        self.flush_progress(bound);
        let mut dispatched = Vec::new();
        match bound.store.list_by_status(SchedTaskStatus::Created) {
            Ok(created) if !created.is_empty() => {
                let registry = self.registry();
                let mut free_slots = self
                    .inner
                    .config
                    .max_running
                    .saturating_sub(self.inner.running.lock().len());
                for task in created {
                    if registry.get(&task.name).is_none() {
                        self.fail_before_dispatch(bound, &task, "function 已不存在(插件被移除)");
                        continue;
                    }
                    if free_slots == 0 {
                        continue;
                    }
                    let queued = self.write_task(|| {
                        bound.store.transition(
                            task.id,
                            &[SchedTaskStatus::Created],
                            SchedTaskStatus::Queued,
                            None,
                        )
                    });
                    let queued = match queued {
                        Ok(Some(queued)) => queued,
                        Ok(None) => continue,
                        Err(error) => {
                            tracing::warn!("dispatch: queue task {}: {error:#}", task.id);
                            continue;
                        }
                    };
                    tracing::info!(
                        "queued task eid:{} tid:{} name {}",
                        queued.evidence_id,
                        queued.id,
                        queued.name
                    );
                    free_slots -= 1;
                    let token = CancellationToken::new();
                    let now = Instant::now();
                    let seq = self.inner.run_seq.fetch_add(1, Ordering::SeqCst);
                    self.inner.running.lock().insert(
                        queued.id,
                        RunningTask {
                            seq,
                            token: token.clone(),
                            publication: Arc::default(),
                            started_at: now,
                            last_update: now,
                            progress: 0,
                            progress_reported: false,
                            last_written_progress: 0,
                            last_write_at: now,
                        },
                    );
                    dispatched.push(Dispatched {
                        task: queued,
                        seq,
                        token,
                    });
                }
            }
            Ok(_) => {}
            Err(error) => tracing::warn!("dispatch: list created tasks: {error:#}"),
        }
        self.run_due_post_actions(bound);
        dispatched
    }

    fn fail_before_dispatch(&self, bound: &BoundCase, task: &SchedTaskRecord, reason: &str) {
        let failed = self.write_task(|| {
            bound.store.transition(
                task.id,
                &[SchedTaskStatus::Created],
                SchedTaskStatus::Failed,
                Some(reason),
            )
        });
        match failed {
            Ok(Some(failed)) => self.after_finish(bound, &failed),
            Ok(None) => {}
            Err(error) => tracing::warn!("dispatch: fail task {}: {error:#}", task.id),
        }
    }

    /// 仅当登记仍属于本次运行(`seq` 一致)时移除,避免旧执行误删重排队
    /// 后的新登记。返回是否移除。
    pub(super) fn remove_running(&self, task_id: i64, seq: u64) -> bool {
        let mut running = self.inner.running.lock();
        if running.get(&task_id).is_some_and(|entry| entry.seq == seq) {
            running.remove(&task_id);
            true
        } else {
            false
        }
    }

    pub(super) fn publication_gate(&self, task_id: i64) -> Option<Arc<parking_lot::Mutex<()>>> {
        self.inner
            .running
            .lock()
            .get(&task_id)
            .map(|entry| Arc::clone(&entry.publication))
    }
}
