//! 调度器全生命周期集成测试:用脚本化执行器(`ScriptedRunner`)替身插件
//! 子进程,驱动 created → queued → running → success/failed 的完整链路,
//! 覆盖取消/重启、事实驱动的后续任务、系统目标、看门狗与崩溃恢复。

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use parking_lot::{Mutex, RwLock};

use super::{
    config::{SchedulerConfig, DEFAULT_MAX_RUNNING},
    dependence::SystemTarget,
    executor::{BoxFuture, ExecutionOutcome, TaskExecution, TaskProgress, TaskRunner},
    functions::{
        tests::{chain_definitions, definition},
        FunctionRegistry,
    },
    model::{
        SchedEvent, SchedNoticeKind, SchedTaskRecord, SchedTaskStatus, TaskParams, TaskResult,
    },
    scheduler::TaskScheduler,
    store::{tests::insert_evidence, InsertOutcome, TaskFilter, TaskStore},
};
use crate::core::casework::case_ledger;
use crate::core::extensions::plugin::TaskTrigger;

const NEVER: Duration = Duration::from_secs(3600);
const LONG: Duration = Duration::from_secs(30);

type DefinitionToml = &'static str;

/// A Linux system analyzer that starts on its own and publishes a fact.
const SYSTEM_BASE: DefinitionToml = r#"
id = "org.test.system"
name = "System"
version = "1"
protocol = 2
library = "test"
[[analyzer]]
id = "base"
name = "System base"
scope = "system"
platforms = ["linux"]
trigger = "auto"
provides = ["timezone"]
[[analyzer]]
id = "logs"
name = "System logs"
scope = "system"
platforms = ["linux"]
trigger = "auto"
requires = ["timezone"]
[[analyzer.option]]
id = "items"
type = "multi"
label = "子项"
choices = [{ id = "auth", label = "认证日志" }, { id = "dpkg", label = "dpkg" }]
"#;

#[derive(Clone)]
struct Script {
    delay: Duration,
    outcome: ExecutionOutcome,
}

impl Script {
    fn success() -> Self {
        Self {
            delay: Duration::from_millis(10),
            outcome: ExecutionOutcome::Success(TaskResult {
                summary: Some("done".to_owned()),
                record_count: 1,
                ..TaskResult::default()
            }),
        }
    }

    fn slow() -> Self {
        Self {
            delay: LONG,
            outcome: ExecutionOutcome::Success(TaskResult::default()),
        }
    }

    fn failing(message: &str) -> Self {
        Self {
            delay: Duration::from_millis(10),
            outcome: ExecutionOutcome::Failed(message.to_owned()),
        }
    }
}

#[derive(Default)]
struct PublicationHold {
    entered: tokio::sync::Notify,
    released: Mutex<bool>,
    changed: parking_lot::Condvar,
}

impl PublicationHold {
    fn release(&self) {
        *self.released.lock() = true;
        self.changed.notify_all();
    }
}

#[derive(Default)]
struct ScriptedRunner {
    scripts: RwLock<Vec<(String, Script)>>,
    started: Mutex<Vec<i64>>,
    terminated: Mutex<Vec<i64>>,
    removed_results: Mutex<Vec<i64>>,
    cleanup_error: Mutex<Option<String>>,
    published_results: Mutex<Vec<i64>>,
    publication_hold: Mutex<Option<Arc<PublicationHold>>>,
    alive: AtomicBool,
    /// Progress channel of each started execution, by task id.
    progress: Mutex<HashMap<i64, TaskProgress>>,
}

impl ScriptedRunner {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            alive: AtomicBool::new(true),
            ..Self::default()
        })
    }

    fn script(&self, function: &str, script: Script) {
        let mut scripts = self.scripts.write();
        scripts.retain(|(name, _)| name != function);
        scripts.push((function.to_owned(), script));
    }

    fn script_for(&self, function: &str) -> Script {
        self.scripts
            .read()
            .iter()
            .find(|(name, _)| name == function)
            .map(|(_, script)| script.clone())
            .unwrap_or_else(Script::success)
    }

    /// Reports progress the way the task's plugin worker would.
    fn report(&self, task_id: i64, current: u64, total: Option<u64>) {
        let progress = self
            .progress
            .lock()
            .get(&task_id)
            .cloned()
            .expect("task started");
        progress(current, total);
    }
}

impl TaskRunner for ScriptedRunner {
    fn run(
        &self,
        task: SchedTaskRecord,
        execution: TaskExecution,
    ) -> BoxFuture<'static, ExecutionOutcome> {
        let script = self.script_for(&task.name);
        self.started.lock().push(task.id);
        self.progress.lock().insert(task.id, execution.progress);
        let token = execution.token;
        Box::pin(async move {
            tokio::select! {
                () = tokio::time::sleep(script.delay) => script.outcome,
                () = token.cancelled() => ExecutionOutcome::Failed("任务已取消".to_owned()),
            }
        })
    }

    fn publish_results(
        &self,
        task: &SchedTaskRecord,
        _seq: u64,
        result: TaskResult,
    ) -> anyhow::Result<TaskResult> {
        let hold = self.publication_hold.lock().clone();
        if let Some(hold) = hold {
            hold.entered.notify_one();
            let mut released = hold.released.lock();
            while !*released {
                if hold
                    .changed
                    .wait_for(&mut released, Duration::from_secs(5))
                    .timed_out()
                {
                    anyhow::bail!("test publication was never released");
                }
            }
        }
        self.published_results.lock().push(task.id);
        Ok(result)
    }

    fn remove_results(&self, task: &SchedTaskRecord) -> anyhow::Result<()> {
        self.removed_results.lock().push(task.id);
        match &*self.cleanup_error.lock() {
            Some(message) => Err(anyhow::anyhow!(message.clone())),
            None => Ok(()),
        }
    }

    fn terminate_worker(&self, task_id: i64, _seq: u64) -> BoxFuture<'static, ()> {
        self.terminated.lock().push(task_id);
        Box::pin(async {})
    }

    fn worker_alive(&self, _task_id: i64, _seq: u64) -> BoxFuture<'static, bool> {
        Box::pin(std::future::ready(self.alive.load(Ordering::SeqCst)))
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    ledger_path: std::path::PathBuf,
    scheduler: TaskScheduler,
    runner: Arc<ScriptedRunner>,
    events: Arc<Mutex<Vec<SchedEvent>>>,
}

impl Harness {
    fn status(&self, task_id: i64) -> SchedTaskStatus {
        self.scheduler.get(task_id).expect("task").status
    }

    /// The progress persisted in the case ledger.
    fn progress(&self, task_id: i64) -> i64 {
        self.scheduler.get(task_id).expect("task").progress
    }

    fn create(&self, function: &str, image_id: &str) -> SchedTaskRecord {
        self.scheduler
            .create_task(function, image_id, params())
            .expect("create task")
            .into_record()
    }

    /// 反复派发直到条件成立(替代真实的 500ms 派发循环)。
    async fn pump_until(&self, mut probe: impl FnMut(&Self) -> bool) -> bool {
        for _ in 0..200 {
            self.scheduler.dispatch_tick().await;
            if probe(self) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        probe(self)
    }

    async fn wait_status(&self, task_id: i64, expected: SchedTaskStatus) -> bool {
        wait_for(|| self.status(task_id) == expected).await
    }

    /// Waits for a task of `function`. Follow-ups are created right after
    /// their parent's final status is committed, so a reader that saw the
    /// parent finish may still be ahead of them.
    async fn wait_task(&self, function: &str) -> Option<SchedTaskRecord> {
        let mut found = None;
        wait_for(|| {
            found = self
                .scheduler
                .list(&TaskFilter {
                    evidence_id: None,
                    include_internal: true,
                })
                .expect("list")
                .into_iter()
                .find(|task| task.name == function);
            found.is_some()
        })
        .await;
        found
    }

    /// Waits until the runner was handed every task in `task_ids` (a task is
    /// `running` in the database just before its execution starts).
    async fn wait_started(&self, task_ids: &[i64]) -> bool {
        wait_for(|| {
            let started = self.runner.started.lock();
            task_ids.iter().all(|task_id| started.contains(task_id))
        })
        .await
    }

    fn has_notice(&self, kind: SchedNoticeKind, task_id: Option<i64>) -> bool {
        self.events.lock().iter().any(|event| {
            matches!(
                event,
                SchedEvent::Notice { notice }
                    if notice.kind == kind && (task_id.is_none() || notice.task_id == task_id)
            )
        })
    }
}

fn paused_config() -> SchedulerConfig {
    SchedulerConfig {
        dispatch_interval: NEVER,
        check_interval: NEVER,
        offline_timeout: NEVER,
        update_timeout: NEVER,
        post_action_debounce: Duration::from_millis(1),
        max_running: DEFAULT_MAX_RUNNING,
    }
}

/// Adds evidence rows `image-2` .. `image-{count}` next to the harness's `image-1`.
fn add_images(harness: &Harness, count: usize) {
    let seed = TaskStore::open(&harness.ledger_path).expect("seed");
    for index in 2..=count {
        insert_evidence(&seed, &format!("image-{index}"));
    }
}

/// A case ledger in `dir` with the evidence `image_ids`; returns its path.
fn seeded_ledger(dir: &std::path::Path, image_ids: &[&str]) -> std::path::PathBuf {
    drop(case_ledger::test_support::ledger(dir));
    let ledger_path = dir.join("case.db");
    let seed = TaskStore::open(&ledger_path).expect("seed store");
    for image_id in image_ids {
        insert_evidence(&seed, image_id);
    }
    ledger_path
}

fn build_registry(extra: &[DefinitionToml]) -> FunctionRegistry {
    let mut definitions = chain_definitions();
    definitions.extend(extra.iter().map(|toml| definition(toml)));
    FunctionRegistry::from_definitions(&definitions)
}

/// A selected file (the manual analyzer is file-scoped).
fn params() -> TaskParams {
    TaskParams {
        node_id: Some("node-1".to_owned()),
        file_name: Some("data.db".to_owned()),
        ..TaskParams::default()
    }
}

fn linux_system() -> SystemTarget {
    SystemTarget {
        id: "linux@part:2".to_owned(),
        family: "linux".to_owned(),
        label: "Kali · part:2".to_owned(),
    }
}

async fn wait_for(mut probe: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if probe() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    probe()
}

fn new_scheduler(
    config: SchedulerConfig,
    extra: Vec<DefinitionToml>,
    runner: &Arc<ScriptedRunner>,
) -> TaskScheduler {
    let runner_for_factory = Arc::clone(runner);
    TaskScheduler::new(
        config,
        Arc::new(move || build_registry(&extra)),
        Arc::new(move |_case_dir| Arc::clone(&runner_for_factory) as Arc<dyn TaskRunner>),
    )
}

async fn harness_with(config: SchedulerConfig, extra: Vec<DefinitionToml>) -> Harness {
    let dir = tempfile::tempdir().expect("tempdir");
    let ledger_path = seeded_ledger(dir.path(), &["image-1"]);

    let runner = ScriptedRunner::new();
    let scheduler = new_scheduler(config, extra, &runner);
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink_events = Arc::clone(&events);
    scheduler.set_event_sink(move |event| sink_events.lock().push(event));
    scheduler
        .bind_case(&ledger_path, dir.path().to_owned())
        .await
        .expect("bind case");
    Harness {
        _dir: dir,
        ledger_path,
        scheduler,
        runner,
        events,
    }
}

async fn harness() -> Harness {
    harness_with(paused_config(), Vec::new()).await
}

#[tokio::test]
async fn manual_task_runs_to_success_and_broadcasts_each_transition() {
    let harness = harness().await;
    let outcome = harness
        .scheduler
        .create_task("org.test.manual/manual_scan", "image-1", params())
        .expect("create");
    let InsertOutcome::Created(task) = outcome else {
        panic!("manual task must be created");
    };
    assert_eq!(task.status, SchedTaskStatus::Created);
    assert_eq!(task.info.trigger, TaskTrigger::Manually);

    harness.scheduler.dispatch_tick().await;
    assert!(harness.wait_status(task.id, SchedTaskStatus::Success).await);
    let finished = harness.scheduler.get(task.id).expect("task");
    assert_eq!(finished.progress, 100);
    assert!(finished.start_at.is_some());
    assert!(finished.spend_time_secs.is_some());
    assert_eq!(
        finished.result,
        Some(TaskResult {
            summary: Some("done".to_owned()),
            record_count: 1,
            ..TaskResult::default()
        })
    );

    let statuses = harness
        .events
        .lock()
        .iter()
        .filter_map(|event| match event {
            SchedEvent::TaskChanged { task } => Some(task.status),
            SchedEvent::Notice { .. } | SchedEvent::TaskRemoved { .. } => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        statuses,
        vec![
            SchedTaskStatus::Created,
            SchedTaskStatus::Queued,
            SchedTaskStatus::Running,
            SchedTaskStatus::Success,
        ]
    );
}

#[tokio::test]
async fn published_facts_trigger_follow_ups_and_all_finished_notice() {
    let harness = harness().await;
    let mut outcomes = harness
        .scheduler
        .on_evidence_online("image-1")
        .expect("evidence online");
    assert_eq!(outcomes.len(), 1);
    let first = outcomes.remove(0).into_record();
    assert_eq!(first.name, "org.test.first/stage_one");
    assert_eq!(first.info.trigger, TaskTrigger::Auto);

    harness.scheduler.dispatch_tick().await;
    assert!(
        harness
            .wait_status(first.id, SchedTaskStatus::Success)
            .await
    );

    let second = harness
        .wait_task("org.test.second/stage_two")
        .await
        .expect("the task waiting for stage_one_done");
    assert_eq!(second.info.parent_tid, Some(first.id));
    assert_eq!(second.info.trigger, TaskTrigger::Auto);
    assert_eq!(second.params, TaskParams::default());

    let finished = harness
        .pump_until(|h| h.status(second.id) == SchedTaskStatus::Success)
        .await;
    assert!(finished);
    let noticed = harness
        .pump_until(|h| h.has_notice(SchedNoticeKind::AllTasksFinished, None))
        .await;
    assert!(
        noticed,
        "all-tasks-finished notice should arrive after debounce"
    );
}

#[tokio::test]
async fn tasks_of_one_plugin_run_side_by_side() {
    let harness = harness().await;
    add_images(&harness, 2);
    harness
        .runner
        .script("org.test.manual/manual_scan", Script::slow());
    let first = harness.create("org.test.manual/manual_scan", "image-1");
    let second = harness.create("org.test.manual/manual_scan", "image-2");

    harness.scheduler.dispatch_tick().await;
    assert!(
        harness.wait_started(&[first.id, second.id]).await,
        "a busy plugin must not hold back its other tasks"
    );
    assert_eq!(harness.status(first.id), SchedTaskStatus::Running);
    assert_eq!(harness.status(second.id), SchedTaskStatus::Running);
}

#[tokio::test]
async fn dispatch_keeps_running_tasks_within_the_limit() {
    let mut config = paused_config();
    config.max_running = 2;
    let harness = harness_with(config, Vec::new()).await;
    add_images(&harness, 3);
    harness
        .runner
        .script("org.test.manual/manual_scan", Script::slow());
    let tasks = (1..=3)
        .map(|index| harness.create("org.test.manual/manual_scan", &format!("image-{index}")))
        .collect::<Vec<_>>();

    harness.scheduler.dispatch_tick().await;
    assert!(
        harness
            .wait_status(tasks[0].id, SchedTaskStatus::Running)
            .await
    );
    assert!(
        harness
            .wait_status(tasks[1].id, SchedTaskStatus::Running)
            .await
    );
    harness.scheduler.dispatch_tick().await;
    assert_eq!(
        harness.status(tasks[2].id),
        SchedTaskStatus::Created,
        "the third task waits for a free slot"
    );

    harness.scheduler.cancel(tasks[0].id).await.expect("cancel");
    harness.scheduler.dispatch_tick().await;
    assert!(
        harness
            .wait_status(tasks[2].id, SchedTaskStatus::Running)
            .await,
        "the waiting task starts once a slot frees up"
    );
    assert_eq!(harness.status(tasks[1].id), SchedTaskStatus::Running);
}

#[tokio::test]
async fn cancelling_one_task_leaves_the_plugins_other_task_running() {
    let harness = harness().await;
    add_images(&harness, 2);
    harness
        .runner
        .script("org.test.manual/manual_scan", Script::slow());
    let cancelled = harness.create("org.test.manual/manual_scan", "image-1");
    let kept = harness.create("org.test.manual/manual_scan", "image-2");
    harness.scheduler.dispatch_tick().await;
    assert!(harness.wait_started(&[cancelled.id, kept.id]).await);

    harness
        .scheduler
        .cancel(cancelled.id)
        .await
        .expect("cancel");

    assert_eq!(harness.status(cancelled.id), SchedTaskStatus::Cancel);
    assert_eq!(harness.runner.terminated.lock().as_slice(), &[cancelled.id]);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(harness.status(kept.id), SchedTaskStatus::Running);
}

#[tokio::test]
async fn cancel_running_task_terminates_worker_and_stays_cancelled() {
    let harness = harness().await;
    harness
        .runner
        .script("org.test.manual/manual_scan", Script::slow());
    let task = harness.create("org.test.manual/manual_scan", "image-1");
    harness.scheduler.dispatch_tick().await;
    assert!(harness.wait_status(task.id, SchedTaskStatus::Running).await);

    let cancelled = harness.scheduler.cancel(task.id).await.expect("cancel");
    assert_eq!(cancelled.status, SchedTaskStatus::Cancel);
    assert_eq!(harness.runner.terminated.lock().as_slice(), &[task.id]);

    // 旧执行随 token 撤销而结束,其迟到的 commit 不得改写 cancel 终态。
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(harness.status(task.id), SchedTaskStatus::Cancel);
    assert!(harness.scheduler.cancel(task.id).await.is_err());

    let restart = harness.scheduler.restart(task.id).await.expect("restart");
    assert_ne!(restart.id, task.id);
    assert_eq!(restart.status, SchedTaskStatus::Created);
    assert_eq!(restart.info.parent_tid, Some(task.id));
    assert_eq!(harness.status(task.id), SchedTaskStatus::Cancel);
}

#[tokio::test]
async fn paused_tasks_of_old_cases_start_from_scratch() {
    let harness = harness().await;
    let task = harness.create("org.test.manual/manual_scan", "image-1");
    // Only an older version paused tasks; its rows still load.
    TaskStore::open(&harness.ledger_path)
        .expect("store")
        .update_status(task.id, SchedTaskStatus::Pause, None)
        .expect("pause row");
    assert_eq!(harness.status(task.id), SchedTaskStatus::Pause);

    let restarted = harness.scheduler.start(task.id).await.expect("start");
    assert_eq!(restarted.status, SchedTaskStatus::Created);
    harness.scheduler.dispatch_tick().await;
    assert!(harness.wait_status(task.id, SchedTaskStatus::Success).await);
}

#[tokio::test]
async fn system_jobs_run_per_detected_system_and_chain_by_facts() {
    let harness = harness_with(paused_config(), vec![SYSTEM_BASE]).await;
    harness.scheduler.set_systems_source(|image_id| {
        if image_id == "image-1" {
            vec![
                linux_system(),
                SystemTarget {
                    id: "windows@part:3".to_owned(),
                    family: "windows".to_owned(),
                    label: "Windows · part:3".to_owned(),
                },
            ]
        } else {
            Vec::new()
        }
    });
    let created = harness
        .scheduler
        .on_evidence_online("image-1")
        .expect("online")
        .into_iter()
        .map(InsertOutcome::into_record)
        .collect::<Vec<_>>();
    let base = created
        .iter()
        .find(|task| task.name == "org.test.system/base")
        .expect("base task on the Linux system")
        .clone();
    assert_eq!(base.params.system_id.as_deref(), Some("linux@part:2"));
    assert_eq!(base.params.target_label.as_deref(), Some("Kali · part:2"));
    assert!(
        created
            .iter()
            .all(|task| task.params.system_id.as_deref() != Some("windows@part:3")),
        "a Linux analyzer never runs on the Windows system"
    );

    harness.scheduler.dispatch_tick().await;
    assert!(harness.wait_status(base.id, SchedTaskStatus::Success).await);
    let logs = harness
        .wait_task("org.test.system/logs")
        .await
        .expect("logs waits for timezone");
    assert_eq!(logs.params.system_id.as_deref(), Some("linux@part:2"));
    assert_eq!(logs.info.parent_tid, Some(base.id));

    // A second manual run on the same system is the same job.
    let manual = |system: &str, options: serde_json::Value| {
        harness.scheduler.create_task(
            "org.test.system/logs",
            "image-1",
            TaskParams {
                system_id: Some(system.to_owned()),
                options: options.as_object().cloned().unwrap_or_default(),
                ..TaskParams::default()
            },
        )
    };
    let repeated =
        manual("linux@part:2", serde_json::json!({ "items": ["auth"] })).expect("same job");
    assert!(matches!(repeated, InsertOutcome::Existing(_)));
    for (system, options) in [
        ("windows@part:3", serde_json::json!({})),
        ("linux@part:9", serde_json::json!({})),
        ("linux@part:2", serde_json::json!({ "items": ["shadow"] })),
        ("linux@part:2", serde_json::json!({ "level": 3 })),
    ] {
        assert!(
            matches!(
                manual(system, options.clone()),
                Err(super::SchedError::InvalidParams(_))
            ),
            "{system} {options}"
        );
    }
    assert!(matches!(
        harness
            .scheduler
            .create_task("org.test.system/logs", "image-1", TaskParams::default()),
        Err(super::SchedError::InvalidParams(_))
    ));
}

#[tokio::test]
async fn failure_creates_no_follow_ups() {
    let harness = harness().await;
    let first = harness
        .scheduler
        .on_evidence_online("image-1")
        .expect("online")
        .remove(0)
        .into_record();
    harness
        .runner
        .script("org.test.first/stage_one", Script::failing("parse error"));
    harness.scheduler.dispatch_tick().await;
    assert!(harness.wait_status(first.id, SchedTaskStatus::Failed).await);
    let failed = harness.scheduler.get(first.id).expect("task");
    assert_eq!(failed.info.error.as_deref(), Some("parse error"));
    let tasks = harness
        .scheduler
        .list(&TaskFilter {
            evidence_id: None,
            include_internal: true,
        })
        .expect("list");
    assert_eq!(tasks.len(), 1, "no dependent task after failure");
}

#[tokio::test]
async fn bind_case_recovers_offline_tasks_by_trigger() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ledger_path = seeded_ledger(dir.path(), &["image-1"]);
    let seed = TaskStore::open(&ledger_path).expect("seed");
    let evidence_id = seed
        .evidence_id_for_image("image-1")
        .expect("lookup")
        .expect("evidence");
    let mut auto_task = super::store::tests::sample_task("org.test.first/stage_one", evidence_id);
    auto_task.trigger = TaskTrigger::Auto;
    let auto_task = seed.insert_task(&auto_task).expect("insert").into_record();
    seed.update_status(auto_task.id, SchedTaskStatus::Running, None)
        .expect("running");
    let manual_task = seed
        .insert_task(&super::store::tests::sample_task(
            "org.test.manual/manual_scan",
            evidence_id,
        ))
        .expect("insert")
        .into_record();
    seed.update_status(manual_task.id, SchedTaskStatus::Queued, None)
        .expect("queued");
    drop(seed);

    let runner = ScriptedRunner::new();
    let scheduler = new_scheduler(paused_config(), Vec::new(), &runner);
    scheduler
        .bind_case(&ledger_path, dir.path().to_owned())
        .await
        .expect("bind");

    assert_eq!(
        scheduler.get(auto_task.id).expect("task").status,
        SchedTaskStatus::Created
    );
    let failed = scheduler.get(manual_task.id).expect("task");
    assert_eq!(failed.status, SchedTaskStatus::Failed);
    assert!(failed.info.error.is_some());
}

#[tokio::test]
async fn watchdog_fails_update_timeout_and_notifies() {
    let mut config = paused_config();
    config.update_timeout = Duration::from_millis(40);
    let harness = harness_with(config, Vec::new()).await;
    harness
        .runner
        .script("org.test.manual/manual_scan", Script::slow());
    let task = harness.create("org.test.manual/manual_scan", "image-1");
    harness.scheduler.dispatch_tick().await;
    assert!(harness.wait_status(task.id, SchedTaskStatus::Running).await);

    tokio::time::sleep(Duration::from_millis(60)).await;
    harness.scheduler.watchdog_tick().await;
    assert_eq!(harness.status(task.id), SchedTaskStatus::Failed);
    assert!(harness.has_notice(SchedNoticeKind::TaskTimeout, Some(task.id)));
    assert_eq!(harness.runner.terminated.lock().as_slice(), &[task.id]);
}

#[tokio::test]
async fn progress_keeps_watchdog_update_timer_alive() {
    // The watchdog reads the wall clock. The task outlives the timeout in
    // total (300 + 150 ms > 400 ms) but not since its progress report, and a
    // loaded test run may delay the tick by up to 250 ms without a false fail.
    // That an unreported task does fail is `watchdog_fails_update_timeout_and_notifies`.
    let mut config = paused_config();
    config.update_timeout = Duration::from_millis(400);
    let harness = harness_with(config, Vec::new()).await;
    harness
        .runner
        .script("org.test.manual/manual_scan", Script::slow());
    let task = harness.create("org.test.manual/manual_scan", "image-1");
    harness.scheduler.dispatch_tick().await;
    assert!(harness.wait_started(&[task.id]).await);

    tokio::time::sleep(Duration::from_millis(300)).await;
    harness.runner.report(task.id, 10, Some(100));
    tokio::time::sleep(Duration::from_millis(150)).await;
    harness.scheduler.watchdog_tick().await;
    assert_eq!(harness.status(task.id), SchedTaskStatus::Running);
}

#[tokio::test]
async fn watchdog_marks_dead_worker_offline_and_splits_by_trigger() {
    let mut config = paused_config();
    config.offline_timeout = Duration::from_millis(1);
    let harness = harness_with(config, Vec::new()).await;
    harness
        .runner
        .script("org.test.first/stage_one", Script::slow());
    harness
        .runner
        .script("org.test.manual/manual_scan", Script::slow());
    let auto_task = harness
        .scheduler
        .on_evidence_online("image-1")
        .expect("online")
        .remove(0)
        .into_record();
    let manual = harness.create("org.test.manual/manual_scan", "image-1");
    harness.scheduler.dispatch_tick().await;
    assert!(
        harness
            .wait_status(auto_task.id, SchedTaskStatus::Running)
            .await
    );
    assert!(
        harness
            .wait_status(manual.id, SchedTaskStatus::Running)
            .await
    );

    tokio::time::sleep(Duration::from_millis(5)).await;
    harness.runner.alive.store(false, Ordering::SeqCst);
    harness.scheduler.watchdog_tick().await;

    assert!(harness.has_notice(SchedNoticeKind::TaskOffline, Some(auto_task.id)));
    assert!(harness.has_notice(SchedNoticeKind::TaskOffline, Some(manual.id)));
    assert_eq!(harness.status(auto_task.id), SchedTaskStatus::Created);
    assert_eq!(harness.status(manual.id), SchedTaskStatus::Failed);
    let mut terminated = harness.runner.terminated.lock().clone();
    terminated.sort_unstable();
    let mut expected = vec![auto_task.id, manual.id];
    expected.sort_unstable();
    assert_eq!(terminated, expected);
}

#[tokio::test]
async fn progress_reaches_only_its_own_task_and_is_persisted_by_dispatch() {
    let harness = harness().await;
    add_images(&harness, 2);
    harness
        .runner
        .script("org.test.manual/manual_scan", Script::slow());
    let first = harness.create("org.test.manual/manual_scan", "image-1");
    let second = harness.create("org.test.manual/manual_scan", "image-2");
    harness.scheduler.dispatch_tick().await;
    assert!(harness.wait_started(&[first.id, second.id]).await);

    harness.runner.report(first.id, 25, Some(100));
    harness.runner.report(second.id, 3, Some(5));
    // Reports only touch memory; the plugin never waits for the database.
    assert_eq!(harness.progress(first.id), 0);
    harness.scheduler.dispatch_tick().await;
    assert_eq!(harness.progress(first.id), 25);
    assert_eq!(harness.progress(second.id), 60);
    assert_eq!(
        harness
            .scheduler
            .get(first.id)
            .expect("task")
            .remain_time_secs,
        Some(0)
    );

    // A report without a total keeps the last percentage.
    harness.runner.report(first.id, 7, None);
    harness.scheduler.dispatch_tick().await;
    assert_eq!(harness.progress(first.id), 25);

    // Reports of a cancelled execution are ignored.
    harness.scheduler.cancel(first.id).await.expect("cancel");
    harness.runner.report(first.id, 90, Some(100));
    harness.scheduler.dispatch_tick().await;
    assert_eq!(harness.progress(first.id), 25);
    assert_eq!(harness.progress(second.id), 60);
}

#[tokio::test]
async fn withdrawing_evidence_cancels_active_tasks_and_broadcasts_their_removal() {
    let harness = harness().await;
    harness
        .runner
        .script("org.test.manual/manual_scan", Script::slow());
    let task = harness.create("org.test.manual/manual_scan", "image-1");
    harness.scheduler.dispatch_tick().await;
    assert!(harness.wait_status(task.id, SchedTaskStatus::Running).await);
    let withdrawal = harness
        .scheduler
        .withdraw_evidence("image-1")
        .await
        .expect("withdraw");
    assert_eq!(
        (withdrawal.cancelled, withdrawal.task_ids.as_slice()),
        (1, [task.id].as_slice())
    );
    assert_eq!(harness.status(task.id), SchedTaskStatus::Cancel);
    assert_eq!(
        harness
            .scheduler
            .withdraw_evidence("image-1")
            .await
            .expect("withdraw again")
            .cancelled,
        0
    );

    // The ledger drops the evidence row (and its tasks by cascade); the
    // scheduler then tells the task center.
    let ledger = case_ledger::open(&harness.ledger_path).expect("ledger");
    ledger
        .execute("DELETE FROM evidence WHERE uid = 'image-1'", [])
        .expect("remove evidence");
    harness.scheduler.forget_evidence(&withdrawal);
    assert!(harness.scheduler.get(task.id).is_err());
    assert!(harness.events.lock().iter().any(|event| {
        matches!(event, SchedEvent::TaskRemoved { task_id, .. } if *task_id == task.id)
    }));
    assert_eq!(
        harness
            .scheduler
            .withdraw_evidence("image-1")
            .await
            .expect("unknown evidence"),
        super::control::EvidenceWithdrawal::default()
    );
}

#[tokio::test]
async fn remove_only_terminal_tasks_and_broadcasts() {
    let harness = harness().await;
    let task = harness.create("org.test.manual/manual_scan", "image-1");
    assert!(harness.scheduler.remove(task.id).await.is_err());
    harness.scheduler.dispatch_tick().await;
    assert!(harness.wait_status(task.id, SchedTaskStatus::Success).await);

    harness.scheduler.remove(task.id).await.expect("remove");
    assert!(harness.events.lock().iter().any(|event| {
        matches!(event, SchedEvent::TaskRemoved { task_id, .. } if *task_id == task.id)
    }));
    assert!(harness.scheduler.get(task.id).is_err());
}

#[tokio::test]
async fn unbind_marks_running_tasks_offline_for_next_session() {
    let harness = harness().await;
    harness
        .runner
        .script("org.test.manual/manual_scan", Script::slow());
    let task = harness.create("org.test.manual/manual_scan", "image-1");
    harness.scheduler.dispatch_tick().await;
    assert!(harness.wait_status(task.id, SchedTaskStatus::Running).await);

    harness.scheduler.unbind_case().await;
    assert!(!harness.scheduler.is_bound());
    assert!(harness.scheduler.get(task.id).is_err());
    let store = TaskStore::open(&harness.ledger_path).expect("reopen");
    assert_eq!(
        store.get(task.id).expect("get").map(|task| task.status),
        Some(SchedTaskStatus::Offline)
    );
    assert_eq!(harness.runner.terminated.lock().as_slice(), &[task.id]);
}

#[tokio::test]
async fn create_task_validates_function_and_evidence() {
    let harness = harness().await;
    assert!(harness
        .scheduler
        .create_task("org.nope/none", "image-1", params())
        .is_err());
    assert!(harness
        .scheduler
        .create_task("org.test.manual/manual_scan", "image-missing", params())
        .is_err());
    let first = harness.create("org.test.manual/manual_scan", "image-1");
    let repeated = harness
        .scheduler
        .create_task("org.test.manual/manual_scan", "image-1", params())
        .expect("create");
    assert_eq!(repeated, InsertOutcome::Existing(first));
}

#[test]
fn config_reads_env_overrides() {
    let config = SchedulerConfig::from_env(|key| match key {
        "LOVELYCARVE_TASK_CHECK_INTERVAL" => Some("7".to_owned()),
        "LOVELYCARVE_TASK_OFFLINE_TIMEOUT" => Some(" 90 ".to_owned()),
        "LOVELYCARVE_TASK_UPDATE_TIMEOUT" => Some("0".to_owned()),
        _ => None,
    });
    assert_eq!(config.check_interval, Duration::from_secs(7));
    assert_eq!(config.offline_timeout, Duration::from_secs(90));
    assert_eq!(config.update_timeout, Duration::from_secs(900));
    assert_eq!(config.dispatch_interval, Duration::from_millis(500));
    assert_eq!(config.max_running, DEFAULT_MAX_RUNNING);

    let max_running = |value: &str| {
        let value = value.to_owned();
        SchedulerConfig::from_env(move |key| {
            (key == SchedulerConfig::ENV_MAX_RUNNING).then(|| value.clone())
        })
        .max_running
    };
    assert_eq!(max_running("2"), 2);
    assert_eq!(max_running("0"), DEFAULT_MAX_RUNNING);
    assert_eq!(max_running("many"), DEFAULT_MAX_RUNNING);
    assert_eq!(max_running("1000"), 32);
}

#[tokio::test]
async fn function_metadata_reaches_task_rows() {
    let harness = harness().await;
    let registry = harness.scheduler.registry();
    let function = registry.get("org.test.second/stage_two").expect("function");
    assert!(function.internal);
    assert_eq!(function.requires, vec!["stage_one_done".to_owned()]);
    let task = harness.create("org.test.second/stage_two", "image-1");
    assert!(task.internal);
    // An evidence-scope task keeps no file target.
    assert_eq!(task.params, TaskParams::default());
    let visible = harness
        .scheduler
        .list(&TaskFilter {
            evidence_id: None,
            include_internal: false,
        })
        .expect("list");
    assert!(visible.is_empty());
}

#[tokio::test]
async fn result_cleanup_failure_keeps_terminal_task_deletion_and_event() {
    let harness = harness().await;
    let task = harness.create("org.test.manual/manual_scan", "image-1");
    // A queued task cannot be removed and must not invoke result cleanup.
    assert!(harness.scheduler.remove(task.id).await.is_err());
    assert!(harness.runner.removed_results.lock().is_empty());
    harness.scheduler.cancel(task.id).await.expect("cancel");
    *harness.runner.cleanup_error.lock() = Some("result store unavailable".to_owned());
    harness
        .scheduler
        .remove(task.id)
        .await
        .expect("best-effort removal");
    assert_eq!(*harness.runner.removed_results.lock(), vec![task.id]);
    assert!(harness.scheduler.get(task.id).is_err());
    assert!(harness.events.lock().iter().any(|event| matches!(
        event,
        SchedEvent::TaskRemoved { task_id, .. } if *task_id == task.id
    )));
    harness.scheduler.unbind_case().await;
}

#[tokio::test]
async fn cancelled_execution_never_publishes_a_late_result() {
    let harness = harness().await;
    let function = "org.test.manual/manual_scan";
    harness.runner.script(function, Script::slow());
    let task = harness.create(function, "image-1");
    harness.scheduler.dispatch_tick().await;
    assert!(harness.wait_started(&[task.id]).await);
    let seq = harness
        .scheduler
        .inner
        .running
        .lock()
        .get(&task.id)
        .expect("running")
        .seq;
    let bound = harness.scheduler.require_bound().expect("bound");
    harness.scheduler.cancel(task.id).await.expect("cancel");
    harness.scheduler.commit(
        &bound,
        task.id,
        seq,
        ExecutionOutcome::Success(TaskResult::default()),
    );
    assert!(harness.runner.published_results.lock().is_empty());
    assert_eq!(harness.status(task.id), SchedTaskStatus::Cancel);
    harness.scheduler.unbind_case().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publication_finishes_before_cancellation_or_case_close_can_return() {
    for closing in [false, true] {
        let harness = harness().await;
        let hold = Arc::new(PublicationHold::default());
        *harness.runner.publication_hold.lock() = Some(Arc::clone(&hold));
        let task = harness.create("org.test.manual/manual_scan", "image-1");
        harness.scheduler.dispatch_tick().await;
        tokio::time::timeout(Duration::from_secs(3), hold.entered.notified())
            .await
            .expect("publishing");
        let scheduler = harness.scheduler.clone();
        let mut control = tokio::spawn(async move {
            if closing {
                scheduler.unbind_case().await;
                Ok(())
            } else {
                scheduler.cancel(task.id).await.map(drop)
            }
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut control)
                .await
                .is_err()
        );
        hold.release();
        let outcome = control.await.expect("control task");
        if closing {
            outcome.expect("closed");
        } else {
            assert!(matches!(
                outcome,
                Err(super::SchedError::InvalidState { .. })
            ));
        }
        let stored = TaskStore::open(&harness.ledger_path)
            .expect("store")
            .get(task.id)
            .expect("get")
            .expect("task");
        assert_eq!(stored.status, SchedTaskStatus::Success);
        assert_eq!(*harness.runner.published_results.lock(), vec![task.id]);
        harness.scheduler.unbind_case().await;
    }
}
