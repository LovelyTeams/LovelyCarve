//! Task execution, cancellation and progress contracts for scheduler runners.
use super::model::{SchedTaskRecord, TaskResult};
use std::{future::Future, pin::Pin, sync::Arc};
use tokio_util::sync::CancellationToken;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 进度回报 `(current, total)`。调度器实现只改内存登记,不触碰台账,
/// 插件线程等待回报确认时不会被数据库锁住。
pub type TaskProgress = Arc<dyn Fn(u64, Option<u64>) + Send + Sync>;

/// 一次执行的提交结果。
#[derive(Clone, Debug, PartialEq)]
pub enum ExecutionOutcome {
    Success(TaskResult),
    Failed(String),
}

/// 调度器交给执行器的一次执行。
pub struct TaskExecution {
    /// 区分同一任务的多次执行(判离线后重排队会以新 `seq` 再执行一次)。
    pub seq: u64,
    /// 取消、暂停、超时或案件关闭时撤销。
    pub token: CancellationToken,
    pub progress: TaskProgress,
}

/// 任务执行后端。实现者接收一条已置 `running` 的
/// 任务并返回提交结果,可同时执行多条任务(含同一插件的任务);必须响应
/// `execution.token` 的取消并尽快返回;进度经 `execution.progress` 回报。
/// `terminate_worker` 对应 runtask 终止该次执行的插件实例(取消/暂停/超时),
/// `worker_alive` 供看门狗做离线检测;两者都按 `(task_id, seq)` 定位执行,
/// 不会碰到同一任务更新的一次执行。
pub trait TaskRunner: Send + Sync {
    fn run(
        &self,
        task: SchedTaskRecord,
        execution: TaskExecution,
    ) -> BoxFuture<'static, ExecutionOutcome>;

    /// Publish prepared outputs only after the scheduler fences this execution
    /// against cancellation, pause, timeout and case close. Runs on a blocking
    /// worker; return the persisted output identity in the resulting TaskResult.
    /// An error must leave no published outputs behind.
    fn publish_results(
        &self,
        _task: &SchedTaskRecord,
        _seq: u64,
        result: TaskResult,
    ) -> anyhow::Result<TaskResult> {
        Ok(result)
    }

    /// Release unpublished output held for a completed or superseded execution.
    fn discard_execution(&self, _task_id: i64, _seq: u64) {}

    /// Remove only the outputs identified by task.result after task deletion
    /// or failed ledger commit. Errors are logged without undoing deletion.
    /// Runners that publish no external outputs can keep the default no-op.
    fn remove_results(&self, _task: &SchedTaskRecord) -> anyhow::Result<()> {
        Ok(())
    }

    fn terminate_worker(&self, task_id: i64, seq: u64) -> BoxFuture<'static, ()>;

    fn worker_alive(&self, task_id: i64, seq: u64) -> BoxFuture<'static, bool>;
}
