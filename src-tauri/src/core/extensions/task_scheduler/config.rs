//! 调度参数与控制面错误类型。
//!
//! 看门狗参数通过 `LOVELYCARVE_TASK_CHECK_INTERVAL` /
//! `LOVELYCARVE_TASK_OFFLINE_TIMEOUT` / `LOVELYCARVE_TASK_UPDATE_TIMEOUT` 配置(秒)。
//! `LOVELYCARVE_TASK_MAX_RUNNING` 是同时执行的任务数上限(全部插件合计)。

use std::time::Duration;

/// 默认同时执行的任务数:每条任务一个插件 worker(独立进程各有 512 MiB
/// 内存上限),多数插件以检材 I/O 为主,4 条并行不至于拖垮单块磁盘。
pub const DEFAULT_MAX_RUNNING: usize = 4;
/// `LOVELYCARVE_TASK_MAX_RUNNING` 的上限。
const MAX_RUNNING_CEILING: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchedulerConfig {
    pub dispatch_interval: Duration,
    pub check_interval: Duration,
    pub offline_timeout: Duration,
    pub update_timeout: Duration,
    pub post_action_debounce: Duration,
    /// 同时处于 queued/running 的任务数上限,不区分插件。
    pub max_running: usize,
}

impl SchedulerConfig {
    pub const ENV_CHECK_INTERVAL: &'static str = "LOVELYCARVE_TASK_CHECK_INTERVAL";
    pub const ENV_OFFLINE_TIMEOUT: &'static str = "LOVELYCARVE_TASK_OFFLINE_TIMEOUT";
    pub const ENV_UPDATE_TIMEOUT: &'static str = "LOVELYCARVE_TASK_UPDATE_TIMEOUT";
    pub const ENV_MAX_RUNNING: &'static str = "LOVELYCARVE_TASK_MAX_RUNNING";

    /// 从环境读取;`lookup` 注入便于测试,不改进程环境。
    pub fn from_env(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let positive = |key: &str| {
            lookup(key)
                .and_then(|value| value.trim().parse::<u64>().ok())
                .filter(|value| *value > 0)
        };
        let seconds =
            |key: &str, default: u64| Duration::from_secs(positive(key).unwrap_or(default));
        let max_running = positive(Self::ENV_MAX_RUNNING)
            .and_then(|value| usize::try_from(value).ok())
            .map_or(DEFAULT_MAX_RUNNING, |value| value.min(MAX_RUNNING_CEILING));
        Self {
            dispatch_interval: Duration::from_millis(500),
            check_interval: seconds(Self::ENV_CHECK_INTERVAL, 15),
            offline_timeout: seconds(Self::ENV_OFFLINE_TIMEOUT, 60),
            update_timeout: seconds(Self::ENV_UPDATE_TIMEOUT, 900),
            post_action_debounce: super::post_action::DEFAULT_DEBOUNCE,
            max_running,
        }
    }
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self::from_env(|_| None)
    }
}

/// 调度器控制面错误;命令层映射为 IPC 错误码。
#[derive(Debug, thiserror::Error)]
pub enum SchedError {
    #[error("没有打开的案件，任务调度未启用")]
    NoCase,
    #[error("任务 {0} 不存在")]
    TaskNotFound(i64),
    #[error("任务 {task_id} 当前状态为 {status}，不能{action}")]
    InvalidState {
        task_id: i64,
        status: &'static str,
        action: &'static str,
    },
    #[error("function {0} 未注册")]
    FunctionUnknown(String),
    #[error("检材 {0} 不在当前案件中")]
    EvidenceUnknown(String),
    #[error(transparent)]
    Store(#[from] anyhow::Error),
}

pub type SchedResult<T> = std::result::Result<T, SchedError>;
