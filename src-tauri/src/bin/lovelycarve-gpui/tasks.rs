//! The task center's data: every native task and plugin job the backend
//! lists (`task_list`), kept current by `task-entry` events. Views read the
//! store; the workspace turns scheduler notices into notifications.
pub mod center;

use std::collections::BTreeMap;

use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, Global, SharedString, Subscription,
};
use lovelycarve_lib::{commands::system::system as system_commands, host::State};
use serde::Deserialize;

use crate::{
    backend::{Backend, BackendEvent},
    events::EventHub,
    format,
};

/// The amount of work a task reported.
#[derive(Clone, Debug, Deserialize)]
pub struct TaskAmount {
    pub unit: String,
    pub done: u64,
    #[serde(default)]
    pub total: Option<u64>,
}

/// A native task's own report: subject parts, stage, counts, labels, flags.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct TaskView {
    pub subject: Vec<String>,
    pub stage: Option<String>,
    pub counts: BTreeMap<String, u64>,
    pub labels: BTreeMap<String, String>,
    pub flags: Vec<String>,
}

/// One entry of the task center, read from the backend's JSON shape.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRow {
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub cancelling: bool,
    #[serde(default)]
    pub created_at_ms: i64,
    #[serde(default)]
    pub started_at_ms: Option<i64>,
    #[serde(default)]
    pub finished_at_ms: Option<i64>,
    #[serde(default)]
    pub progress: Option<TaskAmount>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub actions: Vec<String>,
    /// `builtin` or `plugin`.
    pub source: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub view: Option<TaskView>,
    #[serde(default)]
    pub job: Option<serde_json::Value>,
}

impl TaskRow {
    pub fn is_active(&self) -> bool {
        matches!(
            self.status.as_str(),
            "created" | "queued" | "running" | "pause"
        )
    }

    pub fn can_cancel(&self) -> bool {
        self.actions.iter().any(|action| action == "cancel") && !self.cancelling
    }

    /// The task's name: the kind of native work, or the plugin analyzer.
    pub fn title(&self) -> String {
        if self.source == "plugin" {
            return self
                .job
                .as_ref()
                .and_then(|job| job.get("displayName"))
                .and_then(serde_json::Value::as_str)
                .filter(|name| !name.is_empty())
                .unwrap_or("插件分析")
                .to_owned();
        }
        builtin_title(self.kind.as_deref().unwrap_or_default()).to_owned()
    }

    /// What the task works on.
    pub fn subject(&self) -> String {
        let Some(view) = &self.view else {
            return String::new();
        };
        match self.kind.as_deref() {
            Some("volumeSnapshot" | "sessionSnapshot") => view
                .subject
                .first()
                .map(|volume| volume_label(volume))
                .unwrap_or_default(),
            Some("globalSearch" | "imageHash") => view.subject.join("、"),
            Some("imageLoad") => view
                .subject
                .first()
                .map(|path| format::file_name(path).to_owned())
                .unwrap_or_default(),
            _ => view.subject.join(" · "),
        }
    }

    /// 0–100, or `None` while the amount of work is unknown.
    pub fn percent(&self) -> Option<f32> {
        if self.status == "success" {
            return Some(100.0);
        }
        let amount = self.progress.as_ref()?;
        if amount.unit == "percent" {
            return Some((amount.done as f32).clamp(0.0, 100.0));
        }
        amount
            .total
            .filter(|total| *total > 0)
            .map(|total| format::percent(amount.done, total))
    }

    /// The line under the task's name: how far it got, or what it produced.
    pub fn message(&self) -> String {
        if let Some(error) = self.error.as_ref().filter(|error| !error.is_empty()) {
            return error.clone();
        }
        if self.source == "plugin" {
            return self.plugin_message();
        }
        let view = self.view.clone().unwrap_or_default();
        let count = |key: &str| view.counts.get(key).copied().unwrap_or(0);
        let active = self.is_active();
        let (done, total) = self
            .progress
            .as_ref()
            .map(|amount| (amount.done, amount.total.unwrap_or(0)))
            .unwrap_or((0, 0));
        let bytes = match &self.progress {
            Some(amount) if amount.unit == "bytes" => {
                if total > 0 {
                    format!("{} / {}", format::bytes(done), format::bytes(total))
                } else {
                    format::bytes(done)
                }
            }
            _ => String::new(),
        };
        let faults = if count("syntheticBytes") > 0 {
            format!(
                " · 警告：含 {} 补零字节（{} 个坏区）",
                format::bytes(count("syntheticBytes")),
                count("badRegions")
            )
        } else {
            String::new()
        };
        let truncated = view.flags.iter().any(|flag| flag == "truncated");
        let status = self.status.as_str();
        match self.kind.as_deref().unwrap_or_default() {
            "imageLoad" => match status {
                "success" => "检材已就绪".to_owned(),
                "cancel" => "加载已取消".to_owned(),
                "queued" => "等待加载".to_owned(),
                _ if active => {
                    let stage = match view.stage.as_deref() {
                        Some("decompressing") => format!(
                            "解压 {}",
                            view.labels.get("item").cloned().unwrap_or_default()
                        ),
                        Some("detectingPartitions") => "检测分区".to_owned(),
                        Some("detectingFilesystems") => "检测文件系统".to_owned(),
                        _ => "打开镜像".to_owned(),
                    };
                    [stage, bytes]
                        .into_iter()
                        .filter(|part| !part.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · ")
                }
                _ => String::new(),
            },
            "imageExport" | "bitlockerExport" => match status {
                "success" => format!("已导出 {}{faults}", format::bytes(done)),
                "cancel" => "导出已取消（保留部分文件）".to_owned(),
                _ if active => bytes,
                _ => String::new(),
            },
            "imageHash" => match status {
                "success" => format!("哈希已完成{faults}"),
                _ if active => bytes,
                _ => String::new(),
            },
            "encryptedVolumeUnlock" => match status {
                "success" => format!(
                    "已解锁 · {}",
                    view.labels.get("cipher").cloned().unwrap_or_default()
                ),
                "cancel" => "已取消解锁".to_owned(),
                _ if active => "正在派生密钥并解锁".to_owned(),
                _ => String::new(),
            },
            "autoDetect" => match status {
                "success" => match view
                    .labels
                    .get("platformName")
                    .or(view.labels.get("platform"))
                {
                    Some(platform) => format!("识别为 {platform}"),
                    None => "未识别到平台".to_owned(),
                },
                "queued" => "等待识别".to_owned(),
                _ if active => "正在按文件系统类型与标志文件判定平台".to_owned(),
                _ => String::new(),
            },
            "volumeSnapshot" | "sessionSnapshot" => {
                let limit = if truncated {
                    " · 已达条目上限，结果不完整"
                } else {
                    ""
                };
                match status {
                    "success" if view.flags.iter().any(|flag| flag == "reused") => {
                        format!("已复用现有索引 · {} 项", format::count(count("entries")))
                    }
                    "success" => {
                        format!("索引已就绪 · {} 项{limit}", format::count(count("entries")))
                    }
                    "queued" => "等待加载".to_owned(),
                    _ if !active => String::new(),
                    _ if view.stage.as_deref() == Some("index") => format!(
                        "已写入 {} 项，正在建立索引",
                        format::count(count("entries"))
                    ),
                    _ if count("entries") > 0 => {
                        format!("已读取 {} 项{limit}", format::count(count("entries")))
                    }
                    _ => "正在读取文件系统".to_owned(),
                }
            }
            "globalSearch" => {
                if active {
                    format!(
                        "已扫描 {} / {} 个文件 · {} 处命中",
                        format::count(done),
                        format::count(total),
                        format::count(count("hits"))
                    )
                } else {
                    format!(
                        "{} 个命中文件 · {} 处命中",
                        format::count(count("matchedFiles")),
                        format::count(count("hits"))
                    )
                }
            }
            "timelineBuild" => {
                if active {
                    let of_total = if total > 0 {
                        format!(" / {}", format::count(total))
                    } else {
                        String::new()
                    };
                    format!(
                        "已处理 {}{of_total} 项 · {} 条时间线",
                        format::count(done),
                        format::count(count("rows"))
                    )
                } else {
                    format!(
                        "文件 {} 条 · 事件 {} 条",
                        format::count(count("fileRows")),
                        format::count(count("eventRows"))
                    )
                }
            }
            "carveScan" => {
                let limited = if truncated {
                    " · 已达结果上限"
                } else {
                    ""
                };
                if view.stage.as_deref() == Some("filesystem") {
                    if active {
                        format!("正在扫描文件系统 · 已检查 {} 项", format::count(done))
                    } else {
                        format!("发现 {} 个文件{limited}", format::count(count("found")))
                    }
                } else {
                    format!(
                        "已扫描 {} / {} · 发现 {} 个文件{limited}",
                        format::bytes(done),
                        format::bytes(total),
                        format::count(count("found"))
                    )
                }
            }
            "dockerPrepare" => match status {
                "success" => format!("容器已就绪 · {} 项", format::count(count("entries"))),
                _ if active => "正在合并容器文件系统…".to_owned(),
                _ => String::new(),
            },
            "directoryExport" => match status {
                "success" => format!("已导出 {} 个文件{faults}", format::count(count("files"))),
                "cancel" => "导出已取消（保留部分文件）".to_owned(),
                _ if active => format!(
                    "已导出 {} 个文件 · {}",
                    format::count(count("files")),
                    format::bytes(done)
                ),
                _ => String::new(),
            },
            "carveRecovery" => match status {
                "success" => format!("已恢复 {} 个文件{faults}", format::count(count("files"))),
                _ if active => bytes,
                _ => String::new(),
            },
            _ => String::new(),
        }
    }

    fn plugin_message(&self) -> String {
        let result = self.job.as_ref().and_then(|job| job.get("result"));
        let records = result
            .and_then(|result| result.get("recordCount"))
            .and_then(serde_json::Value::as_u64);
        match (self.status.as_str(), records) {
            ("success", Some(records)) => format!("{} 条结果", format::count(records)),
            ("running", _) => self
                .percent()
                .map(|percent| format!("{percent:.0}%"))
                .unwrap_or_else(|| "执行中".to_owned()),
            (status, _) => status_label(status).to_owned(),
        }
    }

    /// Seconds the task ran so far, by the wall clock.
    pub fn elapsed_secs(&self, now_ms: i64) -> f64 {
        let started = self.started_at_ms.unwrap_or(self.created_at_ms);
        let finished = self.finished_at_ms.unwrap_or(now_ms);
        (finished - started).max(0) as f64 / 1000.0
    }
}

/// The title of a native task kind the task center lists.
pub fn builtin_title(kind: &str) -> &str {
    match kind {
        "imageLoad" => "检材加载",
        "imageExport" | "bitlockerExport" => "镜像导出",
        "imageHash" => "镜像哈希",
        "encryptedVolumeUnlock" => "加密卷解锁",
        "autoDetect" => "自动识别平台",
        "volumeSnapshot" | "sessionSnapshot" => "文件系统加载",
        "globalSearch" => "全局搜索",
        "timelineBuild" => "时间线重建",
        "carveScan" => "文件恢复",
        "carveRecovery" => "导出恢复文件",
        "directoryExport" => "目录导出",
        "dockerPrepare" => "Docker 分析",
        other => other,
    }
}

pub fn status_label(status: &str) -> &'static str {
    match status {
        "created" => "待派发",
        "queued" => "已排队",
        "running" => "执行中",
        "success" => "已完成",
        "failed" => "失败",
        "cancel" => "已取消",
        "pause" => "已暂停",
        "offline" => "已离线",
        _ => "未知",
    }
}

/// A volume key as the file tree names it.
pub fn volume_label(volume_key: &str) -> String {
    match volume_key.strip_prefix("part:") {
        Some(partition) => format!("分区 {partition}"),
        None if volume_key == "root" => "根目录".to_owned(),
        None => volume_key.to_owned(),
    }
}

/// A scheduler notice the workspace shows as a notification.
#[derive(Clone, Debug)]
pub struct TaskNotice {
    pub title: SharedString,
    pub message: SharedString,
    pub success: bool,
}

/// Every task the backend lists, newest first.
pub struct TaskStore {
    rows: Vec<TaskRow>,
    _events: Subscription,
}

impl EventEmitter<TaskNotice> for TaskStore {}

struct GlobalTaskStore(Entity<TaskStore>);

impl Global for GlobalTaskStore {}

impl TaskStore {
    pub fn install(cx: &mut App) {
        let store = cx.new(Self::new);
        cx.set_global(GlobalTaskStore(store));
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalTaskStore>().0.clone()
    }

    fn new(cx: &mut Context<Self>) -> Self {
        let hub = EventHub::global(cx);
        let events = cx.subscribe(&hub, |this, _, event: &BackendEvent, cx| {
            if event.name == "task-entry" {
                this.apply_event(&event.payload, cx);
            }
        });
        let mut store = Self {
            rows: Vec::new(),
            _events: events,
        };
        store.reload(cx);
        store
    }

    /// Replace the list with the backend's current one.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let backend = Backend::global(cx);
        cx.spawn(async move |this, cx| {
            let listed = backend
                .run(|_, state| async move {
                    system_commands::task_list(State::new(&state), None).await
                })
                .await;
            match listed {
                Ok(entries) => {
                    let rows = entries
                        .into_iter()
                        .filter_map(|entry| {
                            serde_json::to_value(entry)
                                .ok()
                                .and_then(|value| serde_json::from_value::<TaskRow>(value).ok())
                        })
                        .collect::<Vec<_>>();
                    this.update(cx, |this, cx| {
                        this.rows = rows;
                        this.sort();
                        cx.notify();
                    })
                    .ok();
                }
                Err(error) => tracing::warn!("task_list: {error}"),
            }
        })
        .detach();
    }

    fn apply_event(&mut self, payload: &serde_json::Value, cx: &mut Context<Self>) {
        match payload.get("kind").and_then(serde_json::Value::as_str) {
            Some("changed") => {
                let Some(row) = payload
                    .get("entry")
                    .and_then(|entry| serde_json::from_value::<TaskRow>(entry.clone()).ok())
                else {
                    return;
                };
                match self.rows.iter_mut().find(|existing| existing.id == row.id) {
                    Some(existing) => *existing = row,
                    None => {
                        self.rows.push(row);
                        self.sort();
                    }
                }
                cx.notify();
            }
            Some("removed") => {
                if let Some(id) = payload.get("id").and_then(serde_json::Value::as_str) {
                    self.rows.retain(|row| row.id != id);
                    cx.notify();
                }
            }
            Some("notice") => {
                let notice = payload.get("notice");
                let kind = notice
                    .and_then(|notice| notice.get("kind"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                let message = notice
                    .and_then(|notice| notice.get("message"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let (title, success) = match kind {
                    "taskOffline" => ("任务执行器离线", false),
                    "taskTimeout" => ("任务进度超时", false),
                    "allTasksFinished" => ("检材任务全部完成", true),
                    "postActionFailed" => ("收尾动作失败", false),
                    _ => return,
                };
                cx.emit(TaskNotice {
                    title: title.into(),
                    message: message.into(),
                    success,
                });
            }
            _ => {}
        }
    }

    fn sort(&mut self) {
        self.rows
            .sort_by_key(|row| std::cmp::Reverse(row.created_at_ms));
    }

    pub fn rows(&self) -> &[TaskRow] {
        &self.rows
    }

    pub fn row(&self, id: &str) -> Option<&TaskRow> {
        self.rows.iter().find(|row| row.id == id)
    }

    pub fn active_count(&self) -> usize {
        self.rows.iter().filter(|row| row.is_active()).count()
    }

    /// Ask the backend to cancel a task; the change arrives as an event.
    pub fn cancel(&mut self, id: String, cx: &mut Context<Self>) {
        let backend = Backend::global(cx);
        cx.spawn(async move |_, _| {
            let cancelled = backend
                .run(move |_, state| async move {
                    system_commands::task_cancel(State::new(&state), id).await
                })
                .await;
            if let Err(error) = cancelled {
                tracing::warn!("task_cancel: {error}");
            }
        })
        .detach();
    }
}
