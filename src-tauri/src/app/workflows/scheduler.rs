//! Plugin task execution and analysis publication, wired into TaskRunner.
use std::{collections::HashMap, path::PathBuf, sync::Arc};

use parking_lot::Mutex;

use crate::core::extensions::task_scheduler::{
    BoxFuture, ExecutionOutcome, SchedTaskRecord, TaskExecution, TaskProgress, TaskResult,
    TaskRunner,
};
use crate::core::{
    analysis::results::models::PluginAnalysisResult,
    analysis::results::plugin_result_store::{self, NewPluginAnalysis},
    extensions::plugin::{
        EvidenceAnalysisRequest, FileAnalysisRequest, PluginManager, PluginProgressEvent,
        TaskWorker,
    },
    filesystem::evidence_session::EvidenceSession,
    timeline::timeline_store,
};

/// 执行中的插件实例,按任务 id 登记(附执行 `seq`)。
type WorkerSlots = Arc<Mutex<HashMap<i64, WorkerSlot>>>;

struct WorkerSlot {
    seq: u64,
    worker: Arc<TaskWorker>,
}

/// 生产执行器:把任务交给插件的 `analyzer.analyze` / `analyze_evidence`。
pub struct PluginTaskRunner {
    images: Arc<tokio::sync::Mutex<HashMap<String, Arc<EvidenceSession>>>>,
    plugins: Arc<parking_lot::RwLock<Option<Arc<PluginManager>>>>,
    case_dir: PathBuf,
    workers: WorkerSlots,
    prepared: Arc<Mutex<HashMap<(i64, u64), Analysis>>>,
}

impl PluginTaskRunner {
    pub fn new(
        images: Arc<tokio::sync::Mutex<HashMap<String, Arc<EvidenceSession>>>>,
        plugins: Arc<parking_lot::RwLock<Option<Arc<PluginManager>>>>,
        case_dir: PathBuf,
    ) -> Self {
        Self {
            images,
            plugins,
            case_dir,
            workers: WorkerSlots::default(),
            prepared: Arc::default(),
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
    workers: WorkerSlots,
    prepared: Arc<Mutex<HashMap<(i64, u64), Analysis>>>,
}

/// 插件分析完成、尚未落库的结果。
struct Analysis {
    result: PluginAnalysisResult,
    plugin_version: String,
    node_id: String,
    file_name: String,
    started_at: String,
    completed_at: String,
}

impl Execution {
    async fn run(self, task: SchedTaskRecord, execution: TaskExecution) -> ExecutionOutcome {
        let TaskExecution {
            seq,
            token,
            progress,
        } = execution;
        let analysis = tokio::select! {
            analysis = self.analyze(&task, seq, progress) => analysis,
            () = token.cancelled() => return ExecutionOutcome::Failed("任务已取消".to_owned()),
        };
        let analysis = match analysis {
            Ok(analysis) => analysis,
            Err(message) => return ExecutionOutcome::Failed(message),
        };
        // 取消落在分析返回之后:不再落库,免得已取消的任务留下分析结果。
        if token.is_cancelled() {
            return ExecutionOutcome::Failed("任务已取消".to_owned());
        }
        let result = TaskResult {
            summary: analysis.result.summary.clone(),
            artifact_count: analysis.result.artifacts.len() as u64,
            timeline_count: analysis.result.timeline.len() as u64,
            finding_count: analysis.result.findings.len() as u64,
            run_id: None,
        };
        self.prepared.lock().insert((task.id, seq), analysis);
        ExecutionOutcome::Success(result)
    }

    async fn analyze(
        &self,
        task: &SchedTaskRecord,
        seq: u64,
        progress: TaskProgress,
    ) -> Result<Analysis, String> {
        let image = self
            .images
            .lock()
            .await
            .get(&task.info.image_id)
            .cloned()
            .ok_or_else(|| format!("检材 {} 未加载", task.info.image_id))?;
        let manifest = self
            .manager
            .definitions()
            .iter()
            .find(|definition| definition.manifest.id == task.info.plugin_id)
            .map(|definition| &definition.manifest);
        let plugin_version = manifest
            .map(|manifest| manifest.version.clone())
            .unwrap_or_default();
        // 检材级 function 没有目标文件,插件经
        // `evidence.*` 自行遍历检材;文件级 function 仍走 FileHandle。
        let evidence_level = manifest.is_some_and(|manifest| {
            manifest
                .evidence_analyzer_capabilities()
                .any(|capability| capability.analyzer == task.info.analyzer)
        });
        let (node_id, file_name) = if evidence_level {
            (String::new(), String::new())
        } else {
            let node_id = task
                .params
                .node_id
                .clone()
                .ok_or_else(|| "任务缺少目标文件 nodeId".to_owned())?;
            let file_name = task
                .params
                .file_name
                .clone()
                .unwrap_or_else(|| node_id.clone());
            (node_id, file_name)
        };
        let started_at = chrono::Utc::now().to_rfc3339();
        let worker = self
            .manager
            .start_task_worker(&task.info.plugin_id, move |event: PluginProgressEvent| {
                progress(event.current, event.total)
            })
            .await
            .map_err(|error| format!("插件实例启动失败：{error}"))?;
        let worker = Arc::new(worker);
        let registration = WorkerRegistration::new(&self.workers, task.id, seq, &worker);
        let result = if evidence_level {
            let params = serde_json::to_value(&task.params).unwrap_or(serde_json::Value::Null);
            self.manager
                .analyze_evidence_on(
                    &worker,
                    EvidenceAnalysisRequest {
                        plugin_id: task.info.plugin_id.clone(),
                        analyzer: task.info.analyzer.clone(),
                        image_id: task.info.image_id.clone(),
                        image,
                        params,
                    },
                )
                .await
        } else {
            self.manager
                .analyze_file_on(
                    &worker,
                    FileAnalysisRequest {
                        plugin_id: task.info.plugin_id.clone(),
                        analyzer: task.info.analyzer.clone(),
                        image_id: task.info.image_id.clone(),
                        image,
                        node_id: node_id.clone(),
                        name: file_name.clone(),
                    },
                )
                .await
        };
        drop(registration);
        // 结果已经拿到:实例在后台退出,不拖慢 commit。
        tokio::spawn(async move {
            if let Err(error) = worker.shutdown().await {
                tracing::debug!(plugin_id = %worker.plugin_id(), "task worker shutdown failed: {error}");
            }
        });
        let result = result.map_err(|error| error.to_string())?;
        Ok(Analysis {
            result,
            plugin_version,
            node_id,
            file_name,
            started_at,
            completed_at: chrono::Utc::now().to_rfc3339(),
        })
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
            workers: Arc::clone(&self.workers),
            prepared: Arc::clone(&self.prepared),
        };
        Box::pin(execution_state.run(task, execution))
    }

    fn publish_results(
        &self,
        task: &SchedTaskRecord,
        seq: u64,
        mut result: TaskResult,
    ) -> anyhow::Result<TaskResult> {
        let analysis = self
            .prepared
            .lock()
            .remove(&(task.id, seq))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "prepared analysis missing for task {} execution {seq}",
                    task.id
                )
            })?;
        let run_id = plugin_result_store::persist(
            &self.case_dir,
            NewPluginAnalysis {
                plugin_id: &task.info.plugin_id,
                plugin_version: &analysis.plugin_version,
                analyzer: &task.info.analyzer,
                display_name: &task.display_name,
                type_name: &task.type_name,
                image_id: &task.info.image_id,
                node_id: &analysis.node_id,
                file_name: &analysis.file_name,
                started_at: &analysis.started_at,
                completed_at: &analysis.completed_at,
                result: &analysis.result,
            },
        )?;
        // Project while the same execution publication gate is held; deleting
        // a terminal task cannot race a detached projection and resurrect it.
        if let Err(error) = crate::core::timeline::timeline_project::project_run(
            &self.case_dir,
            &task.info.image_id,
            &run_id,
        ) {
            tracing::warn!("timeline project_run {run_id} failed (will rebuild later): {error:#}");
        }
        result.run_id = Some(run_id);
        Ok(result)
    }

    fn discard_execution(&self, task_id: i64, seq: u64) {
        self.prepared.lock().remove(&(task_id, seq));
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

/// 删除任务已落库的分析结果与时间线投影。
/// 时间线只在已建立时按 tid 级联删。
fn remove_task_results(case_dir: &std::path::Path, task: &SchedTaskRecord) -> anyhow::Result<()> {
    let Some(run_id) = task
        .result
        .as_ref()
        .and_then(|result| result.run_id.as_deref())
    else {
        return Ok(());
    };
    plugin_result_store::remove_run(case_dir, &task.info.image_id, run_id)?;
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
