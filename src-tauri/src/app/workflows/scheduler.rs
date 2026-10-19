//! Plugin task execution and result publication, wired into TaskRunner.
//!
//! An execution starts a worker for the task's plugin, prepares the run
//! (target view, declared inputs, facts, a staging run in the evidence's
//! `records.db`; see [`prepare_run`]) and waits for `run.start` while the
//! plugin streams its records. Publication — under the scheduler's
//! publication gate — swaps the staging run in and projects its timeline
//! points; an execution that is cancelled, fails or is superseded has its
//! staging run deleted.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use parking_lot::Mutex;

use crate::core::extensions::task_scheduler::{
    BoxFuture, ExecutionOutcome, SchedTaskRecord, TaskExecution, TaskProgress, TaskResult,
    TaskRunner,
};
use crate::core::{
    analysis::{
        identify::systems::SystemCache,
        results::{legacy_store, record_store},
    },
    evidence::image::ReadFaultReport,
    extensions::plugin::{
        prepare_run, AnalyzerScope, PluginManager, ProgressSink, RunPlan, RunTargetSpec, TaskWorker,
    },
    filesystem::evidence_session::EvidenceSession,
    timeline::{timeline_project, timeline_store},
};

/// 执行中的插件实例,按任务 id 登记(附执行 `seq`)。
type WorkerSlots = Arc<Mutex<HashMap<i64, WorkerSlot>>>;

struct WorkerSlot {
    seq: u64,
    worker: Arc<TaskWorker>,
}

/// The staging run of one execution, until it is published or discarded.
struct StagedRun {
    image_id: String,
    run_id: String,
    /// Set once the plugin finished the run.
    finished: Option<FinishedRun>,
}

struct FinishedRun {
    summary: Option<String>,
    finished_at: String,
    read_faults: ReadFaultReport,
}

type StagedRuns = Arc<Mutex<HashMap<(i64, u64), StagedRun>>>;

/// 生产执行器:把任务交给插件 worker 的 `run.start`。
pub struct PluginTaskRunner {
    images: Arc<tokio::sync::Mutex<HashMap<String, Arc<EvidenceSession>>>>,
    plugins: Arc<parking_lot::RwLock<Option<Arc<PluginManager>>>>,
    systems: Arc<SystemCache>,
    case_dir: PathBuf,
    workers: WorkerSlots,
    staged: StagedRuns,
}

impl PluginTaskRunner {
    pub fn new(
        images: Arc<tokio::sync::Mutex<HashMap<String, Arc<EvidenceSession>>>>,
        plugins: Arc<parking_lot::RwLock<Option<Arc<PluginManager>>>>,
        systems: Arc<SystemCache>,
        case_dir: PathBuf,
    ) -> Self {
        Self {
            images,
            plugins,
            systems,
            case_dir,
            workers: WorkerSlots::default(),
            staged: Arc::default(),
        }
    }

    fn manager(&self) -> Option<Arc<PluginManager>> {
        self.plugins.read().clone()
    }
}

/// 一次执行在异步任务里需要的全部状态。
struct Execution {
    images: Arc<tokio::sync::Mutex<HashMap<String, Arc<EvidenceSession>>>>,
    manager: Arc<PluginManager>,
    systems: Arc<SystemCache>,
    case_dir: PathBuf,
    workers: WorkerSlots,
    staged: StagedRuns,
}

impl Execution {
    async fn run(self, task: SchedTaskRecord, execution: TaskExecution) -> ExecutionOutcome {
        let TaskExecution {
            seq,
            token,
            progress,
        } = execution;
        let outcome = tokio::select! {
            outcome = self.analyze(&task, seq, progress, &token) => outcome,
            () = token.cancelled() => Err("任务已取消".to_owned()),
        };
        match outcome {
            // A cancellation that lands after the run finished publishes nothing.
            Ok(_) if token.is_cancelled() => ExecutionOutcome::Failed("任务已取消".to_owned()),
            Ok(result) => ExecutionOutcome::Success(result),
            Err(message) => ExecutionOutcome::Failed(message),
        }
    }

    async fn analyze(
        &self,
        task: &SchedTaskRecord,
        seq: u64,
        progress: TaskProgress,
        token: &tokio_util::sync::CancellationToken,
    ) -> Result<TaskResult, String> {
        let image = self
            .images
            .lock()
            .await
            .get(&task.info.image_id)
            .cloned()
            .ok_or_else(|| format!("检材 {} 未加载", task.info.image_id))?;
        let definition = self
            .manager
            .definition(&task.info.plugin_id)
            .map_err(|error| error.to_string())?;
        let analyzer = definition
            .manifest
            .analyzer(&task.info.analyzer)
            .cloned()
            .ok_or_else(|| format!("插件不再提供分析器 {}", task.info.analyzer))?;
        let target = match analyzer.scope {
            AnalyzerScope::System => RunTargetSpec::System {
                system_id: task
                    .params
                    .system_id
                    .clone()
                    .ok_or_else(|| "任务缺少目标系统".to_owned())?,
            },
            AnalyzerScope::Evidence => RunTargetSpec::Evidence,
            AnalyzerScope::Files => {
                let node_id = task
                    .params
                    .node_id
                    .clone()
                    .ok_or_else(|| "任务缺少目标文件".to_owned())?;
                RunTargetSpec::File {
                    name: task
                        .params
                        .file_name
                        .clone()
                        .unwrap_or_else(|| node_id.clone()),
                    node_id,
                }
            }
        };
        let worker = self
            .manager
            .start_worker(&task.info.plugin_id)
            .await
            .map_err(|error| format!("插件实例启动失败：{error}"))?;
        let worker = Arc::new(worker);
        let registration = WorkerRegistration::new(&self.workers, task.id, seq, &worker);

        let run_id = format!("run_{}", uuid::Uuid::new_v4().simple());
        self.staged.lock().insert(
            (task.id, seq),
            StagedRun {
                image_id: image.id.clone(),
                run_id: run_id.clone(),
                finished: None,
            },
        );
        let prepared = {
            let image = Arc::clone(&image);
            let systems = Arc::clone(&self.systems);
            let case_dir = self.case_dir.clone();
            let options = task.params.options.clone();
            let hello = worker.hello().clone();
            let run_id = run_id.clone();
            let token = token.clone();
            let progress: ProgressSink = Arc::new(move |current, total| progress(current, total));
            tokio::task::spawn_blocking(move || {
                let systems = if analyzer.scope == AnalyzerScope::System {
                    systems.get_or_discover(&image)
                } else {
                    Arc::default()
                };
                let evidence_label = image
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| image.id.clone());
                prepare_run(
                    &RunPlan {
                        case_dir: &case_dir,
                        image: &image,
                        definition: &definition,
                        analyzer: &analyzer,
                        target: &target,
                        options: &options,
                        systems: &systems,
                        hello: &hello,
                        evidence_label: &evidence_label,
                    },
                    &run_id,
                    progress,
                    &token,
                )
            })
            .await
            .map_err(|error| format!("运行准备失败：{error}"))?
            .map_err(|error| format!("运行准备失败：{error:#}"))?
        };
        // Discarded while it was being prepared: drop the staging run.
        if !self.staged.lock().contains_key(&(task.id, seq)) {
            discard_in_background(&self.case_dir, &image.id, &run_id);
            return Err("任务已取消".to_owned());
        }
        let finish = worker
            .run(Arc::clone(&prepared.context), &prepared.start)
            .await;
        drop(registration);
        // The results are in: let the worker exit in the background.
        tokio::spawn(async move {
            if let Err(error) = worker.shutdown().await {
                tracing::debug!(plugin_id = %worker.plugin_id(), "task worker shutdown failed: {error}");
            }
        });
        let finish = finish.map_err(|error| error.to_string())?;
        let (records, warnings, time_points) = prepared.context.counts();
        let read_faults = prepared.context.read_faults();
        let result = TaskResult {
            summary: finish.summary.clone(),
            record_count: records,
            timeline_count: time_points,
            warning_count: warnings,
            synthetic_bytes: read_faults.synthetic_bytes,
            run_id: None,
        };
        match self.staged.lock().get_mut(&(task.id, seq)) {
            Some(staged) => {
                staged.finished = Some(FinishedRun {
                    summary: finish.summary,
                    finished_at: chrono::Utc::now().to_rfc3339(),
                    read_faults,
                });
            }
            None => return Err("任务已取消".to_owned()),
        }
        Ok(result)
    }
}

/// 分析期间登记执行的插件实例,供 `terminate_worker` / `worker_alive`
/// 找到它;离开作用域(分析结束,或执行被丢弃)即注销。
struct WorkerRegistration {
    workers: WorkerSlots,
    task_id: i64,
    seq: u64,
}

impl WorkerRegistration {
    fn new(workers: &WorkerSlots, task_id: i64, seq: u64, worker: &Arc<TaskWorker>) -> Self {
        workers.lock().insert(
            task_id,
            WorkerSlot {
                seq,
                worker: Arc::clone(worker),
            },
        );
        Self {
            workers: Arc::clone(workers),
            task_id,
            seq,
        }
    }
}

impl Drop for WorkerRegistration {
    fn drop(&mut self) {
        let mut workers = self.workers.lock();
        if workers
            .get(&self.task_id)
            .is_some_and(|slot| slot.seq == self.seq)
        {
            workers.remove(&self.task_id);
        }
    }
}

fn discard_in_background(case_dir: &std::path::Path, image_id: &str, run_id: &str) {
    let (case_dir, image_id, run_id) =
        (case_dir.to_owned(), image_id.to_owned(), run_id.to_owned());
    tokio::task::spawn_blocking(move || {
        if let Err(error) = record_store::discard(&case_dir, &image_id, &run_id) {
            tracing::warn!(%run_id, "staging run not discarded: {error:#}");
        }
    });
}

impl TaskRunner for PluginTaskRunner {
    fn run(
        &self,
        task: SchedTaskRecord,
        execution: TaskExecution,
    ) -> BoxFuture<'static, ExecutionOutcome> {
        let Some(manager) = self.manager() else {
            return Box::pin(async {
                ExecutionOutcome::Failed("插件运行时尚未初始化".to_owned())
            });
        };
        let execution_state = Execution {
            images: Arc::clone(&self.images),
            manager,
            systems: Arc::clone(&self.systems),
            case_dir: self.case_dir.clone(),
            workers: Arc::clone(&self.workers),
            staged: Arc::clone(&self.staged),
        };
        Box::pin(execution_state.run(task, execution))
    }

    fn publish_results(
        &self,
        task: &SchedTaskRecord,
        seq: u64,
        mut result: TaskResult,
    ) -> anyhow::Result<TaskResult> {
        let staged =
            self.staged.lock().remove(&(task.id, seq)).ok_or_else(|| {
                anyhow::anyhow!("no staged run for task {} execution {seq}", task.id)
            })?;
        let Some(finished) = &staged.finished else {
            record_store::discard(&self.case_dir, &staged.image_id, &staged.run_id)?;
            anyhow::bail!("run {} did not finish", staged.run_id);
        };
        let published = record_store::publish(
            &self.case_dir,
            &staged.image_id,
            &staged.run_id,
            &record_store::RunCompletion {
                summary: finished.summary.as_deref(),
                finished_at: &finished.finished_at,
                read_faults: &finished.read_faults,
            },
        )?
        .ok_or_else(|| anyhow::anyhow!("run {} was discarded before publication", staged.run_id))?;
        // Project under the same publication gate, so deleting the task cannot
        // race a detached projection and resurrect it.
        if let Err(error) = timeline_project::project_published_run(
            &self.case_dir,
            &staged.image_id,
            &staged.run_id,
            &published.superseded,
        ) {
            tracing::warn!(
                "timeline projection of run {} failed (rebuild the timeline to include it): {error:#}",
                staged.run_id
            );
        }
        result.run_id = Some(staged.run_id);
        Ok(result)
    }

    fn discard_execution(&self, task_id: i64, seq: u64) {
        let Some(staged) = self.staged.lock().remove(&(task_id, seq)) else {
            return;
        };
        if let Err(error) = record_store::discard(&self.case_dir, &staged.image_id, &staged.run_id)
        {
            tracing::warn!(run_id = %staged.run_id, "staging run not discarded: {error:#}");
        }
    }

    fn remove_results(&self, task: &SchedTaskRecord) -> anyhow::Result<()> {
        remove_task_results(&self.case_dir, task)
    }

    fn terminate_worker(&self, task_id: i64, seq: u64) -> BoxFuture<'static, ()> {
        let slot = {
            let mut workers = self.workers.lock();
            match workers.get(&task_id) {
                Some(slot) if slot.seq == seq => workers.remove(&task_id),
                Some(_) | None => None,
            }
        };
        // 尚在启动的实例没有登记:撤销的 token 让执行丢弃启动中的 future,
        // 子进程随之被结束(kill-on-drop)。
        Box::pin(async move {
            if let Some(slot) = slot {
                slot.worker.terminate().await;
            }
        })
    }

    fn worker_alive(&self, task_id: i64, seq: u64) -> BoxFuture<'static, bool> {
        // 尚在启动或已经结束的执行不算离线。
        let alive = self
            .workers
            .lock()
            .get(&task_id)
            .filter(|slot| slot.seq == seq)
            .is_none_or(|slot| slot.worker.is_alive());
        Box::pin(std::future::ready(alive))
    }
}

/// 删除任务已发布的运行(新版 `records.db`,旧案件的 `analysis.db`)与它的
/// 时间线投影。时间线只在已建立时按 tid 删。
fn remove_task_results(case_dir: &std::path::Path, task: &SchedTaskRecord) -> anyhow::Result<()> {
    let Some(run_id) = task
        .result
        .as_ref()
        .and_then(|result| result.run_id.as_deref())
    else {
        return Ok(());
    };
    if !record_store::remove_run(case_dir, &task.info.image_id, run_id)? {
        legacy_store::remove_run(case_dir, &task.info.image_id, run_id)?;
    }
    let Some(conn) = timeline_store::open_existing(
        case_dir,
        &task.info.image_id,
        std::time::Duration::from_secs(30),
    )?
    else {
        return Ok(());
    };
    timeline_store::delete_runs(&conn, &[run_id.to_owned()])?;
    Ok(())
}

#[cfg(test)]
mod tests;
