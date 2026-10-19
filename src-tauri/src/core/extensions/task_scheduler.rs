//! 插件任务调度全生命周期。
//!
//! | 职责 | 模块 |
//! |---|---|
//! | 案件台账的 `task` / `task_followup` 表 | `store.rs` / `store_evidence.rs` |
//! | 状态机 created/queued/running/success/failed/cancel/offline | `model.rs` |
//! | 从插件包清单注册可调度功能 | `functions.rs` |
//! | 调度循环与任务派发 | `scheduler.rs` |
//! | 插件执行、提交结果与进度广播 | `lifecycle.rs` + `executor.rs` |
//! | 取消、启动、重启与删除任务 | `control.rs` |
//! | 离线/超时巡检与分流 | `watchdog.rs` + `lifecycle.rs::handle_offline` |
//! | 检材上线首任务与画像事实触发的后续任务 | `dependence.rs` |
//! | `PostActionWatcher` | `post_action.rs` |
//! | `LOVELYCARVE_TASK_*` 环境变量 | `config.rs` |
//!
//! 一个作业是 (function, 检材, 目标):目标是识别出的系统、选中的文件,
//! 或整个检材。worker 是为每次执行单独拉起的插件实例(lc-plugin-host 子
//! 进程);同一插件的多条任务并行,总数受 `LOVELYCARVE_TASK_MAX_RUNNING`
//! 限制。任务不能暂停:取消即结束执行,重启从头再跑。

mod config;
mod control;
mod dependence;
mod executor;
mod functions;
mod lifecycle;
mod model;
mod post_action;
mod scheduler;
mod store;
mod store_evidence;
mod watchdog;

#[cfg(test)]
mod scheduler_tests;

pub use config::{SchedError, SchedResult, SchedulerConfig};
pub use control::EvidenceWithdrawal;
pub use dependence::{SystemTarget, TaskTarget};
pub use executor::{BoxFuture, ExecutionOutcome, TaskExecution, TaskProgress, TaskRunner};
pub use functions::{
    function_name, FunctionChoice, FunctionOption, FunctionRegistry, SchedFunction,
};
pub use model::{
    EvidenceTaskStatus, NewTask, SchedEvent, SchedNotice, SchedNoticeKind, SchedTaskRecord,
    SchedTaskStatus, TaskInfo, TaskParams, TaskResult,
};
pub use post_action::{PostAction, PostActionHandler, PostActionRequest};
pub use scheduler::{RegistrySource, RunnerFactory, SystemsSource, TaskScheduler};
pub use store::{InsertOutcome, TaskFilter, TaskStore};
