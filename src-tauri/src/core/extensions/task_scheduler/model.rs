//! 任务调度数据模型:状态机、任务行与广播事件。
//!
//! 持久化约定(案件台账的 `task` 表,见 `core::casework::case_ledger`):状态词表
//! `created/queued/running/success/failed/cancel/pause/offline` 原样存进
//! `status` 列,台账以 CHECK 约束;插件、分析器、触发方式、父任务与失败原因
//! 各有独立列,只有 `params` / `result` 是 JSON。线上形态 [`SchedTaskRecord`]
//! 仍把它们收在 `info` 里。新任务不再暂停:`pause` 只出现在旧案件的任务行里,
//! 这样的任务可以启动或取消。

use serde::{Deserialize, Serialize};

use crate::core::extensions::plugin::TaskTrigger;

/// 任务状态。`Offline` 是看门狗/进程退出写入的过渡态,由
/// 离线处理器决定重排队(`auto` 任务)还是失败(手动任务)。
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SchedTaskStatus {
    Created,
    Queued,
    Running,
    Success,
    Failed,
    Cancel,
    Pause,
    Offline,
}

impl SchedTaskStatus {
    pub const ALL: [Self; 8] = [
        Self::Created,
        Self::Queued,
        Self::Running,
        Self::Success,
        Self::Failed,
        Self::Cancel,
        Self::Pause,
        Self::Offline,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Success => "success",
            Self::Failed => "failed",
            Self::Cancel => "cancel",
            Self::Pause => "pause",
            Self::Offline => "offline",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.as_str() == text)
    }

    /// 终态:不会再被调度器改写。
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Success | Self::Failed | Self::Cancel)
    }

    /// PostActionWatcher 的“已完成”口径:
    /// 终态即完成;`pause`/`offline` 仍算未完成。
    pub fn is_finished(self) -> bool {
        self.is_terminal()
    }

    /// 取消任务允许的起始状态。
    pub fn can_cancel(self) -> bool {
        matches!(
            self,
            Self::Created | Self::Queued | Self::Running | Self::Pause | Self::Offline
        )
    }

    /// 启动任务允许的起始状态(离线任务,以及旧案件里暂停的任务)。
    pub fn can_start(self) -> bool {
        matches!(self, Self::Pause | Self::Offline)
    }

    /// 重启任务允许的起始状态:终态任务重建一条新任务。
    pub fn can_restart(self) -> bool {
        self.is_terminal() || matches!(self, Self::Offline)
    }

    /// 参与调度器派发/运行计数的状态。
    pub fn is_active(self) -> bool {
        matches!(self, Self::Created | Self::Queued | Self::Running)
    }
}

/// 任务参数(`task.params` JSON):作业的目标与用户选定的选项。旧案件任务行
/// 里的其他字段(平台、子项)读取时忽略。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskParams {
    /// 文件作用域:分析的文件节点。
    #[serde(default)]
    pub node_id: Option<String>,
    #[serde(default)]
    pub file_name: Option<String>,
    /// 系统作用域:识别出的系统(`linux@part:2`)。
    #[serde(default)]
    pub system_id: Option<String>,
    /// 目标的展示名(系统名或文件名)。
    #[serde(default)]
    pub target_label: Option<String>,
    /// 分析器选项(按清单 `[[analyzer.option]]` 校验)。
    #[serde(default)]
    pub options: serde_json::Map<String, serde_json::Value>,
}

impl TaskParams {
    /// 作业在检材内的目标:系统 id、文件节点,整个检材为空串。同一
    /// (function, 检材, 目标) 是同一个作业,单实例约束与结果替换都按它算。
    pub fn target_key(&self) -> &str {
        self.system_id
            .as_deref()
            .or(self.node_id.as_deref())
            .unwrap_or_default()
    }
}

/// 任务的插件与调度元数据(线上形态;台账里各是一列,`image_id` 来自检材行)。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskInfo {
    pub plugin_id: String,
    pub analyzer: String,
    pub image_id: String,
    pub trigger: TaskTrigger,
    /// 依赖链/重启时的上一级任务 id。
    #[serde(default)]
    pub parent_tid: Option<i64>,
    /// 最近一次失败/离线的原因,供前端展示。
    #[serde(default)]
    pub error: Option<String>,
}

/// 提交结果(`task.result` JSON)。旧案件的 `artifactCount` 读作记录数。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskResult {
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default, alias = "artifactCount")]
    pub record_count: u64,
    /// 进入时间线的时间点数。
    #[serde(default)]
    pub timeline_count: u64,
    /// 运行留下的告警数(运行审计里可查看)。
    #[serde(default)]
    pub warning_count: u64,
    /// 合成(读不出、以零填充)的源字节数。
    #[serde(default)]
    pub synthetic_bytes: u64,
    /// 检材 `records.db`(旧案件为 `analysis.db`)里的运行 id。
    #[serde(default)]
    pub run_id: Option<String>,
}

/// 一条任务的完整视图,也是前端任务列表/广播事件的载荷。时间戳是台账的
/// RFC 3339 UTC 字符串。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedTaskRecord {
    pub id: i64,
    /// function 名(`<plugin id>/<analyzer>`),依赖匹配键。
    pub name: String,
    pub display_name: String,
    pub type_name: String,
    /// 台账检材行的整数主键。
    pub evidence_id: i64,
    pub progress: i64,
    pub params: TaskParams,
    pub result: Option<TaskResult>,
    pub dependence: String,
    pub status: SchedTaskStatus,
    pub info: TaskInfo,
    pub can_pause: bool,
    pub single_instance: bool,
    pub internal: bool,
    /// 已耗时(秒;终态时按开始与结束时间结算)。
    pub spend_time_secs: Option<i64>,
    /// 预计剩余(秒;按进度线性外推)。
    pub remain_time_secs: Option<i64>,
    pub start_at: Option<String>,
    pub create_at: String,
    pub update_at: String,
}

/// 建任务输入。检材的 image id 由 `evidence_id` 决定,不重复存储。
#[derive(Clone, Debug, PartialEq)]
pub struct NewTask {
    /// function 名(`<plugin id>/<analyzer>`)。
    pub name: String,
    pub display_name: String,
    pub type_name: String,
    pub evidence_id: i64,
    pub plugin_id: String,
    pub analyzer: String,
    pub trigger: TaskTrigger,
    /// 触发它的上一个任务(事实发布者或重启前的任务)。
    pub parent_tid: Option<i64>,
    pub params: TaskParams,
    pub single_instance: bool,
    pub internal: bool,
}

/// 检材级任务状态聚合,由任务行即时统计(PostActionWatcher 的"全部完成"
/// 判定),不落库。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceTaskStatus {
    pub total: u64,
    pub created: u64,
    pub queued: u64,
    pub running: u64,
    pub success: u64,
    pub failed: u64,
    pub cancel: u64,
    pub pause: u64,
    pub offline: u64,
}

impl EvidenceTaskStatus {
    pub fn from_statuses(statuses: impl IntoIterator<Item = SchedTaskStatus>) -> Self {
        let mut aggregate = Self::default();
        for status in statuses {
            aggregate.total += 1;
            match status {
                SchedTaskStatus::Created => aggregate.created += 1,
                SchedTaskStatus::Queued => aggregate.queued += 1,
                SchedTaskStatus::Running => aggregate.running += 1,
                SchedTaskStatus::Success => aggregate.success += 1,
                SchedTaskStatus::Failed => aggregate.failed += 1,
                SchedTaskStatus::Cancel => aggregate.cancel += 1,
                SchedTaskStatus::Pause => aggregate.pause += 1,
                SchedTaskStatus::Offline => aggregate.offline += 1,
            }
        }
        aggregate
    }

    pub fn finished(&self) -> u64 {
        self.success + self.failed + self.cancel
    }

    pub fn all_finished(&self) -> bool {
        self.total > 0 && self.finished() == self.total
    }
}

/// 看门狗与 PostActionWatcher 的离线、超时和完成通知,推送到前端。
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SchedNoticeKind {
    TaskOffline,
    TaskTimeout,
    AllTasksFinished,
    PostActionFailed,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedNotice {
    pub kind: SchedNoticeKind,
    pub task_id: Option<i64>,
    pub evidence_id: Option<i64>,
    pub message: String,
}

/// 调度器对外广播。任务行较大,装箱以免撑大每个事件。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SchedEvent {
    TaskChanged { task: Box<SchedTaskRecord> },
    TaskRemoved { task_id: i64, evidence_id: i64 },
    Notice { notice: SchedNotice },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trips_through_text_and_serde() {
        for status in SchedTaskStatus::ALL {
            assert_eq!(SchedTaskStatus::parse(status.as_str()), Some(status));
            let json = serde_json::to_string(&status).expect("serialize");
            assert_eq!(json, format!("\"{}\"", status.as_str()));
            let parsed: SchedTaskStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(parsed, status);
        }
        assert_eq!(SchedTaskStatus::parse("restarting"), None);
    }

    #[test]
    fn transition_guards_follow_reference_state_machine() {
        assert!(SchedTaskStatus::Running.can_cancel());
        assert!(SchedTaskStatus::Pause.can_cancel());
        assert!(!SchedTaskStatus::Success.can_cancel());
        assert!(SchedTaskStatus::Pause.can_start());
        assert!(SchedTaskStatus::Offline.can_start());
        assert!(!SchedTaskStatus::Running.can_start());
        assert!(SchedTaskStatus::Failed.can_restart());
        assert!(SchedTaskStatus::Offline.can_restart());
        assert!(!SchedTaskStatus::Queued.can_restart());
        assert!(SchedTaskStatus::Cancel.is_finished());
        assert!(!SchedTaskStatus::Offline.is_finished());
    }

    #[test]
    fn evidence_aggregate_counts_every_status() {
        let aggregate = EvidenceTaskStatus::from_statuses([
            SchedTaskStatus::Success,
            SchedTaskStatus::Failed,
            SchedTaskStatus::Running,
            SchedTaskStatus::Cancel,
        ]);
        assert_eq!(
            aggregate,
            EvidenceTaskStatus {
                total: 4,
                created: 0,
                queued: 0,
                running: 1,
                success: 1,
                failed: 1,
                cancel: 1,
                pause: 0,
                offline: 0,
            }
        );
        assert_eq!(aggregate.finished(), 3);
        assert!(!aggregate.all_finished());
        assert!(EvidenceTaskStatus::from_statuses([SchedTaskStatus::Success]).all_finished());
        assert!(!EvidenceTaskStatus::default().all_finished());
    }

    #[test]
    fn old_task_rows_still_parse() {
        let params: TaskParams = serde_json::from_str(
            r#"{"nodeId":null,"fileName":null,"platform":"linux","subItems":["用户列表"]}"#,
        )
        .expect("old params");
        assert_eq!(params, TaskParams::default());
        let result: TaskResult = serde_json::from_str(
            r#"{"summary":"done","artifactCount":7,"timelineCount":2,"findingCount":0,"runId":"run_1"}"#,
        )
        .expect("old result");
        assert_eq!(result.record_count, 7);
        assert_eq!(result.run_id.as_deref(), Some("run_1"));
        let system = TaskParams {
            system_id: Some("linux@part:2".to_owned()),
            node_id: Some("ignored".to_owned()),
            ..TaskParams::default()
        };
        assert_eq!(system.target_key(), "linux@part:2");
    }
}
