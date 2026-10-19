//! 时间线存储:检材 `timeline.db` 里的投影表与查询。
//!
//! 时间线是一张**只存引用 + 冗余展示列**的投影表:源数据落库时各 datatype
//! 按声明投影一行(`oid`=源记录主键、`tid`=来源任务、`tl_time`=unix 秒、
//! `type`=datatype 编号、`from_app`=来源应用、`content`=FullDesc 摘要、
//! `tl_flag`=时间语义标记)。一条源记录 = 一条时间线记录;时间线永远可从源
//! 数据整体重建([`crate::core::timeline::timeline_project`])。
//!
//! 每个检材一个库,只有两类写者:重建任务(分批短事务)与插件收尾钩子(每个
//! run 一个 IMMEDIATE 事务)。不同检材的重建与投影互不排队;检材移除即删除
//! 整个库文件。读路径(状态轮询、聚合、明细)从不建库、不跑 DDL。查询面:
//!
//! - **count(聚合)**:`strftime('<fmt>', tl_time, 'unixepoch', '<tz> hours')`
//!   动态时间桶(年/月/日/时/分/秒粒度 + 时区参数化);
//! - **datas(明细)**:时间范围 + from_app/type 过滤 + `tl_time desc`
//!   默认排序 + skip/limit 分页,`total` 同条件 COUNT 另查;
//! - **types(维度)**:`group by from_app, type` 的两级(应用 → 数据类型)清单。

mod write;
pub use write::{
    clear, delete_runs, insert_rows, meta_begin, meta_finish, meta_get, project_run_rows,
};
mod query;
pub use query::{count_buckets, export_rows, page, time_range, types};

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use rusqlite::{types::Value, Connection, OptionalExtension, ToSql};
use serde::{Deserialize, Serialize};

use crate::core::casework::case_paths::EvidenceDb;
use crate::core::casework::evidence_db::{self, StoreSchema};

/// `timeline.db` 结构。
const SCHEMA: StoreSchema = StoreSchema {
    db: EvidenceDb::Timeline,
    // ASCII "LCTL".
    application_id: 0x4C43_544C,
    version: 1,
    ddl: r#"
        CREATE TABLE timeline (
            id INTEGER PRIMARY KEY,
            oid TEXT NOT NULL,
            tid TEXT NOT NULL DEFAULT '',
            "delete" INTEGER NOT NULL DEFAULT 0,
            tl_time INTEGER NOT NULL,
            from_app TEXT NOT NULL DEFAULT '',
            type INTEGER NOT NULL,
            tl_flag TEXT NOT NULL DEFAULT '',
            content TEXT NOT NULL DEFAULT '',
            extra TEXT NOT NULL DEFAULT ''
        );
        CREATE INDEX timeline_app_type ON timeline (from_app, type);
        CREATE INDEX timeline_deleted ON timeline ("delete");
        CREATE INDEX timeline_tid ON timeline (tid);
        CREATE INDEX timeline_time ON timeline (tl_time);

        CREATE TABLE timeline_build (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            status TEXT NOT NULL,
            error TEXT,
            started_ms INTEGER NOT NULL,
            finished_ms INTEGER,
            file_rows INTEGER NOT NULL DEFAULT 0,
            event_rows INTEGER NOT NULL DEFAULT 0,
            source_file_watermark TEXT NOT NULL DEFAULT '',
            source_event_watermark TEXT NOT NULL DEFAULT ''
        );
    "#,
    busy_timeout: Duration::from_secs(30),
};

// ── datatype 编号注册表 ──

/// `file`：文件系统元数据记录（filedb）,投影 Mtime。
/// Ctime/Atime 不进时间线,每种 datatype 只绑定一个时间字段。
pub const DATA_TYPE_FILE: i32 = 1;
/// `plugin_event`:插件记录的时间线时间点(`records.db` 里带时间线角色的时间戳字段),
/// 以及旧案件 `analysis.db` 的插件时间线事件。
pub const DATA_TYPE_PLUGIN_EVENT: i32 = 2;
/// Retained native NTFS change journal events (historical names and references).
pub const DATA_TYPE_NTFS_USN: i32 = 3;

/// datatype 编号 → 展示名。
pub fn data_type_name(data_type: i32) -> String {
    match data_type {
        DATA_TYPE_FILE => "文件".to_string(),
        DATA_TYPE_PLUGIN_EVENT => "插件时间线".to_string(),
        DATA_TYPE_NTFS_USN => "NTFS 变更日志".to_string(),
        other => format!("未知类型 {other}"),
    }
}

// ── Wire 类型（与前端 `src/lib/ipc/timeline.ts` 镜像，snake_case 透传）──

/// 一条时间线投影行(明细表行)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TimelineRow {
    pub id: i64,
    /// 源记录主键:file → filedb nid;plugin_event → event_id。
    pub oid: String,
    /// 来源任务:plugin_event → run_id;file → 空(索引构建无任务 tid)。
    pub tid: String,
    /// 所属检材(库所在的检材,查询时填写)。
    pub eid: String,
    /// 源记录已删除（软删镜像，`delete` 列）。
    pub deleted: bool,
    /// unix 秒。
    pub tl_time: i64,
    pub from_app: String,
    pub data_type: i32,
    /// 时间语义标记（创建/修改/访问…；file 固定「修改」）。
    pub tl_flag: String,
    /// FullDesc 摘要（desc 字段拼接）。
    pub content: String,
    pub extra: String,
}

/// 投影写入行(重建 / 增量钩子)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineInsertRow {
    pub oid: String,
    pub tid: String,
    pub deleted: bool,
    pub tl_time: i64,
    pub from_app: String,
    pub data_type: i32,
    pub tl_flag: String,
    pub content: String,
    pub extra: String,
}

/// 时间桶粒度：年/月/日/时/分/秒，默认 Year。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimelineUnit {
    #[default]
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
}

impl TimelineUnit {
    /// strftime 时间桶格式（聚合 SQL 动态拼入）。
    pub fn strftime_format(self) -> &'static str {
        match self {
            Self::Year => "%Y",
            Self::Month => "%Y-%m",
            Self::Day => "%Y-%m-%d",
            Self::Hour => "%Y-%m-%d %H",
            Self::Minute => "%Y-%m-%d %H:%M",
            Self::Second => "%Y-%m-%d %H:%M:%S",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Year => "year",
            Self::Month => "month",
            Self::Day => "day",
            Self::Hour => "hour",
            Self::Minute => "minute",
            Self::Second => "second",
        }
    }

    /// 钻取降档（Year → Month → … → Second；Second 已到底，返回自身）。
    pub fn drill_down(self) -> Self {
        match self {
            Self::Year => Self::Month,
            Self::Month => Self::Day,
            Self::Day => Self::Hour,
            Self::Hour => Self::Minute,
            Self::Minute | Self::Second => Self::Second,
        }
    }

    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "year" => Ok(Self::Year),
            "month" => Ok(Self::Month),
            "day" => Ok(Self::Day),
            "hour" => Ok(Self::Hour),
            "minute" => Ok(Self::Minute),
            "second" => Ok(Self::Second),
            other => anyhow::bail!("未知时间粒度：{other}"),
        }
    }
}

/// 两级维度树的一个勾选对（配置弹窗「应用 → 数据类型」精确过滤，
/// 避免两个独立列表的叉积误匹配）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineTypePair {
    pub from_app: String,
    pub data_type: i32,
}

/// 时间桶、明细与导出查询共享的参数族(作用于一个检材的时间线)。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TimelineQuery {
    /// unix 秒,闭区间。
    pub start: Option<i64>,
    pub end: Option<i64>,
    /// 聚合时区偏移小时数（strftime `'%+d hours'` 修饰符）。
    pub time_zone_hours: i32,
    pub time_unit: TimelineUnit,
    /// from_app 过滤（空 = 全部）。
    pub from_apps: Vec<String>,
    /// datatype 编号过滤（空 = 全部）。
    pub data_types: Vec<i32>,
    /// (from_app, type) 对过滤（配置弹窗两级勾选；非空时优先于上面两列）。
    pub pairs: Vec<TimelineTypePair>,
    /// content LIKE 过滤。
    pub text: Option<String>,
    /// 是否包含源已删除的行（`delete=1`）。
    pub show_delete: bool,
}

/// 一个聚合桶。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TimelineBucket {
    pub date_str: String,
    pub count: u64,
}

/// 明细分页查询：`skip/limit` + 排序方向,默认 `tl_time desc`。
#[derive(Debug, Clone)]
pub struct TimelinePageQuery {
    pub query: TimelineQuery,
    pub skip: u64,
    pub limit: u64,
    pub ascending: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TimelineDataPage {
    pub items: Vec<TimelineRow>,
    pub total: u64,
}

/// 两级维度（应用 → 数据类型）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TimelineTypeInfo {
    pub data_type: i32,
    pub data_type_name: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TimelineTypeGroup {
    pub from_app: String,
    pub types: Vec<TimelineTypeInfo>,
}

/// 检材时间线的构建元数据(`timeline_build` 唯一一行),记录重建任务状态。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TimelineMetaRow {
    /// `running` | `success` | `cancelled` | `failed`。
    pub status: String,
    pub error: Option<String>,
    pub started_ms: i64,
    pub finished_ms: Option<i64>,
    pub file_rows: u64,
    pub event_rows: u64,
    /// 文件源水位(已索引卷的快照构建时间),staleness 判定。
    pub source_file_watermark: String,
    /// 插件事件源水位:`records.db` 已发布运行的发布时间与旧版
    /// `analysis.db` 运行完成时间的最大值。
    pub source_event_watermark: String,
}

// ── 连接与建表 ─────────────────────────────────────────────────────────

/// 写者连接(重建任务、插件收尾钩子);库不存在时创建。
pub fn open_writer(case_dir: &Path, image_id: &str) -> anyhow::Result<Connection> {
    evidence_db::open_writer(case_dir, image_id, &SCHEMA)
}

/// 读者连接;检材还没有时间线时为 `None`。
pub fn open_reader(case_dir: &Path, image_id: &str) -> anyhow::Result<Option<Connection>> {
    evidence_db::open_reader(case_dir, image_id, &SCHEMA)
}

/// 只改已有数据的写者(孤儿状态修复、任务删除级联);不建库。
pub fn open_existing(
    case_dir: &Path,
    image_id: &str,
    busy_timeout: Duration,
) -> anyhow::Result<Option<Connection>> {
    evidence_db::open_existing(case_dir, image_id, &SCHEMA, busy_timeout)
}

const ROW_COLUMNS: &str =
    "id, oid, tid, \"delete\", tl_time, from_app, type, tl_flag, content, extra";

#[cfg(test)]
mod tests;
