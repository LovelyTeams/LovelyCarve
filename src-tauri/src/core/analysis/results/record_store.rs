//! 插件记录库 `records.db`(LPP v2):运行、记录类型、按类型分表的记录、
//! 时间点与运行告警,存在检材目录下。
//!
//! 写入流程:
//!
//! 1. [`begin_run`] 以 `staging` 状态登记一次运行:作业键、目标、选项、
//!    输入文件、结构哈希。
//! 2. 运行期间插件分批提交记录,[`RecordWriter::insert_batch`] 每批一个
//!    `BEGIN IMMEDIATE` 事务,按记录类型写入各自的表;告警经
//!    [`RecordWriter::warn`] 随运行保存。持锁时间受批大小约束,同一检材的
//!    多条运行交替提交。
//! 3. 成功收尾时 [`publish`] 在一个事务里条件发布:本运行仍是 `staging`
//!    才改为 `published`;同一作业此前发布的运行改为 `superseded` 并删除其
//!    记录(运行行与告警留作审计)。失败、取消或被替换的执行由 [`discard`]
//!    删除。
//!
//! 查询只读 `published` 运行。进程异常退出留下的 `staging` 运行带着旧进程
//! 的属主令牌,下一次 [`begin_run`] 时删除——案件进程锁保证没有别的进程在
//! 写这个案件。
//!
//! 记录类型以 (类型名, 字段结构哈希) 登记在 `record_type`,每个类型一张
//! `rec_<type_id>` 表:字段按 schema 顺序成列 `c<i>`,时间戳字段另有排序列
//! `s<i>`(微秒;未带时区的本地时间按墙上时间计,只用于排序)。公共信封
//! (标题、严重度、来源)在 `record` 表;带时间线角色的时间戳另写入
//! `record_time`,供时间线投影。

use std::{collections::HashMap, path::Path, sync::LazyLock, time::Duration};

use anyhow::Context as _;
use lovelycarve_plugin::{protocol::WireRecord, record::SourceRef, schema::RecordSchema};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::core::{
    analysis::results::models::RecordSource,
    casework::{
        case_paths::EvidenceDb,
        evidence_db::{self, StoreSchema},
    },
    evidence::image::ReadFaultReport,
};

mod typed;

pub(crate) use typed::{
    column_json, registered_schema, sort_column, table_name, value_column, TypeTable,
};

/// `records.db` 结构。记录类型的表在首次登记时建立(见 [`TypeTable`])。
pub(crate) const SCHEMA: StoreSchema = StoreSchema {
    db: EvidenceDb::Records,
    // ASCII "LCRC".
    application_id: 0x4C43_5243,
    version: 1,
    ddl: r#"
        CREATE TABLE run (
            run_id TEXT PRIMARY KEY,
            job_key TEXT NOT NULL,
            owner TEXT NOT NULL,
            state TEXT NOT NULL
                CHECK (state IN ('staging', 'published', 'superseded')),
            plugin_id TEXT NOT NULL,
            plugin_version TEXT NOT NULL,
            analyzer TEXT NOT NULL,
            analyzer_name TEXT NOT NULL,
            category TEXT NOT NULL,
            target_kind TEXT NOT NULL,
            target_id TEXT NOT NULL,
            target_label TEXT NOT NULL,
            options TEXT NOT NULL,
            profile TEXT NOT NULL,
            inputs TEXT NOT NULL,
            input_count INTEGER NOT NULL,
            schema_hash TEXT NOT NULL,
            summary TEXT,
            record_count INTEGER NOT NULL DEFAULT 0,
            warning_count INTEGER NOT NULL DEFAULT 0,
            bad_regions TEXT NOT NULL DEFAULT '[]',
            synthetic_bytes INTEGER NOT NULL DEFAULT 0,
            started_at TEXT NOT NULL,
            finished_at TEXT,
            published_at TEXT
        );
        CREATE UNIQUE INDEX run_published ON run (job_key) WHERE state = 'published';
        CREATE INDEX run_by_state ON run (state, published_at);

        CREATE TABLE record_type (
            type_id INTEGER PRIMARY KEY,
            record_type TEXT NOT NULL,
            schema_hash TEXT NOT NULL,
            schema TEXT NOT NULL,
            UNIQUE (record_type, schema_hash)
        );

        CREATE TABLE run_type (
            run_id TEXT NOT NULL REFERENCES run (run_id) ON DELETE CASCADE,
            type_id INTEGER NOT NULL REFERENCES record_type (type_id),
            record_count INTEGER NOT NULL,
            PRIMARY KEY (run_id, type_id)
        ) WITHOUT ROWID;

        CREATE TABLE record (
            record_id INTEGER PRIMARY KEY,
            run_id TEXT NOT NULL REFERENCES run (run_id) ON DELETE CASCADE,
            type_id INTEGER NOT NULL REFERENCES record_type (type_id),
            title TEXT NOT NULL,
            severity TEXT,
            sources TEXT NOT NULL
        );
        CREATE INDEX record_by_run_type ON record (run_id, type_id, record_id);

        CREATE TABLE record_time (
            record_id INTEGER NOT NULL REFERENCES record (record_id) ON DELETE CASCADE,
            field TEXT NOT NULL,
            run_id TEXT NOT NULL,
            role TEXT NOT NULL,
            epoch_us INTEGER NOT NULL,
            zoned INTEGER NOT NULL CHECK (zoned IN (0, 1)),
            PRIMARY KEY (record_id, field)
        ) WITHOUT ROWID;
        CREATE INDEX record_time_by_run ON record_time (run_id);

        CREATE TABLE run_warning (
            warning_id INTEGER PRIMARY KEY,
            run_id TEXT NOT NULL REFERENCES run (run_id) ON DELETE CASCADE,
            code TEXT NOT NULL,
            message TEXT NOT NULL,
            path TEXT,
            node_id TEXT
        );
        CREATE INDEX run_warning_by_run ON run_warning (run_id, warning_id);
    "#,
    busy_timeout: Duration::from_secs(60),
};

/// Most warnings kept per run; later ones are only counted.
const MAX_STORED_WARNINGS: u64 = 10_000;
/// Most inputs listed in a run's audit; the count covers all of them.
const MAX_LISTED_INPUTS: usize = 10_000;
/// Most sources one record cites.
const MAX_SOURCES: usize = 16;
/// Longest text of one source field.
const MAX_SOURCE_TEXT: usize = 4096;
/// Longest warning message kept.
const MAX_WARNING_TEXT: usize = 16 * 1024;

/// This process's token on its staging runs.
static OWNER: LazyLock<String> = LazyLock::new(|| uuid::Uuid::new_v4().simple().to_string());

/// What a run looks at.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunTarget {
    /// `files`, `system` or `evidence`.
    pub kind: String,
    /// The system id, the input file's node id, or empty for the evidence.
    pub id: String,
    pub label: String,
}

/// One file a run was given, as its audit lists it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunInput {
    /// The path inside the run's view (the file name for file runs).
    pub path: String,
    #[serde(default)]
    pub root: Option<String>,
    #[serde(default)]
    pub root_path: Option<String>,
    #[serde(default)]
    pub node_id: Option<String>,
    pub size: u64,
}

/// A run being started.
pub struct NewRun<'a> {
    pub run_id: &'a str,
    /// One published run per job: `<plugin>/<analyzer>` plus the target.
    pub job_key: &'a str,
    pub plugin_id: &'a str,
    pub plugin_version: &'a str,
    pub analyzer: &'a str,
    pub analyzer_name: &'a str,
    pub category: &'a str,
    pub target: &'a RunTarget,
    pub options: &'a serde_json::Value,
    pub profile: &'a serde_json::Value,
    pub inputs: &'a [RunInput],
    pub schema_hash: &'a str,
    pub started_at: &'a str,
}

/// How a finished run ended, stored when it is published.
pub struct RunCompletion<'a> {
    pub summary: Option<&'a str>,
    pub finished_at: &'a str,
    pub read_faults: &'a ReadFaultReport,
}

/// A publication that took place.
#[derive(Clone, Debug, PartialEq)]
pub struct Published {
    pub published_at: String,
    /// Earlier runs of the same job, now superseded.
    pub superseded: Vec<String>,
}

/// Why a batch of records was refused.
#[derive(Debug, thiserror::Error)]
pub enum RecordWriteError {
    /// The plugin sent something its schema does not allow.
    #[error("{code}: {message}")]
    Rejected { code: &'static str, message: String },
    #[error(transparent)]
    Store(#[from] anyhow::Error),
}

impl From<rusqlite::Error> for RecordWriteError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

fn rejected(code: &'static str, message: impl Into<String>) -> RecordWriteError {
    RecordWriteError::Rejected {
        code,
        message: message.into(),
    }
}

/// Registers a run in `staging`. Staging runs of an exited process are
/// removed first: nothing can publish them any more.
pub fn begin_run(case_dir: &Path, image_id: &str, run: &NewRun<'_>) -> anyhow::Result<()> {
    let connection = evidence_db::open_writer(case_dir, image_id, &SCHEMA)?;
    let transaction = evidence_db::write_transaction(&connection).context("begin plugin run")?;
    transaction.execute(
        "DELETE FROM run WHERE state = 'staging' AND owner <> ?1",
        [OWNER.as_str()],
    )?;
    let listed = &run.inputs[..run.inputs.len().min(MAX_LISTED_INPUTS)];
    transaction.execute(
        "INSERT INTO run (
            run_id, job_key, owner, state, plugin_id, plugin_version, analyzer, analyzer_name,
            category, target_kind, target_id, target_label, options, profile, inputs,
            input_count, schema_hash, started_at
        ) VALUES (?1, ?2, ?3, 'staging', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                  ?16, ?17)",
        params![
            run.run_id,
            run.job_key,
            OWNER.as_str(),
            run.plugin_id,
            run.plugin_version,
            run.analyzer,
            run.analyzer_name,
            run.category,
            run.target.kind,
            run.target.id,
            run.target.label,
            serde_json::to_string(run.options)?,
            serde_json::to_string(run.profile)?,
            serde_json::to_string(listed)?,
            run.inputs.len() as i64,
            run.schema_hash,
            run.started_at,
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

/// Publishes a staging run (see the module docs). `None` when the run is no
/// longer staging: it was discarded, or another execution published it.
pub fn publish(
    case_dir: &Path,
    image_id: &str,
    run_id: &str,
    completion: &RunCompletion<'_>,
) -> anyhow::Result<Option<Published>> {
    let Some(connection) =
        evidence_db::open_existing(case_dir, image_id, &SCHEMA, SCHEMA.busy_timeout)?
    else {
        return Ok(None);
    };
    let transaction = evidence_db::write_transaction(&connection)?;
    let job_key: Option<String> = transaction
        .query_row(
            "SELECT job_key FROM run WHERE run_id = ?1 AND state = 'staging'",
            [run_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(job_key) = job_key else {
        return Ok(None);
    };
    let superseded: Vec<String> = {
        let mut statement = transaction
            .prepare("SELECT run_id FROM run WHERE job_key = ?1 AND state = 'published'")?;
        let rows = statement.query_map([&job_key], |row| row.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    for old in &superseded {
        transaction.execute("DELETE FROM record WHERE run_id = ?1", [old])?;
        transaction.execute(
            "UPDATE run SET state = 'superseded' WHERE run_id = ?1",
            [old],
        )?;
    }
    let published_at = chrono::Utc::now().to_rfc3339();
    transaction.execute(
        "UPDATE run SET state = 'published', summary = ?2, finished_at = ?3, published_at = ?4,
                        bad_regions = ?5, synthetic_bytes = ?6
         WHERE run_id = ?1 AND state = 'staging'",
        params![
            run_id,
            completion.summary,
            completion.finished_at,
            published_at,
            serde_json::to_string(&completion.read_faults.bad_regions)?,
            i64::try_from(completion.read_faults.synthetic_bytes).unwrap_or(i64::MAX),
        ],
    )?;
    transaction.commit()?;
    Ok(Some(Published {
        published_at,
        superseded,
    }))
}

/// Deletes a staging run and its records. Published runs are kept.
pub fn discard(case_dir: &Path, image_id: &str, run_id: &str) -> anyhow::Result<()> {
    let Some(connection) =
        evidence_db::open_existing(case_dir, image_id, &SCHEMA, SCHEMA.busy_timeout)?
    else {
        return Ok(());
    };
    connection.execute(
        "DELETE FROM run WHERE run_id = ?1 AND state = 'staging'",
        [run_id],
    )?;
    Ok(())
}

/// Deletes a run in any state with its records and warnings (task
/// deletion). Returns whether the run existed.
pub fn remove_run(case_dir: &Path, image_id: &str, run_id: &str) -> anyhow::Result<bool> {
    let Some(connection) =
        evidence_db::open_existing(case_dir, image_id, &SCHEMA, SCHEMA.busy_timeout)?
    else {
        return Ok(false);
    };
    Ok(connection.execute("DELETE FROM run WHERE run_id = ?1", [run_id])? > 0)
}

/// Streams one run's records into `records.db`.
pub struct RecordWriter {
    connection: Connection,
    run_id: String,
    types: HashMap<String, TypeTable>,
    records: u64,
    warnings: u64,
    time_points: u64,
}

impl RecordWriter {
    /// Opens the writer of `run_id` and registers the plugin's record types.
    pub fn open(
        case_dir: &Path,
        image_id: &str,
        run_id: &str,
        schemas: &[RecordSchema],
    ) -> anyhow::Result<Self> {
        let connection = evidence_db::open_writer(case_dir, image_id, &SCHEMA)?;
        let transaction = evidence_db::write_transaction(&connection)?;
        let mut types = HashMap::new();
        for schema in schemas {
            types.insert(
                schema.record_type.clone(),
                TypeTable::register(&transaction, schema)?,
            );
        }
        transaction.commit()?;
        Ok(Self {
            connection,
            run_id: run_id.to_owned(),
            types,
            records: 0,
            warnings: 0,
            time_points: 0,
        })
    }

    pub fn records(&self) -> u64 {
        self.records
    }

    pub fn warnings(&self) -> u64 {
        self.warnings
    }

    pub fn time_points(&self) -> u64 {
        self.time_points
    }

    /// Validates and stores one batch in one transaction. Nothing is stored
    /// when any record is refused.
    pub fn insert_batch(&mut self, records: &[WireRecord]) -> Result<u64, RecordWriteError> {
        let mut prepared = Vec::with_capacity(records.len());
        for record in records {
            prepared.push(self.prepare(record)?);
        }
        let transaction = evidence_db::write_transaction(&self.connection)?;
        let mut per_type: HashMap<i64, i64> = HashMap::new();
        let mut time_points = 0_u64;
        {
            let mut envelope = transaction.prepare_cached(
                "INSERT INTO record (run_id, type_id, title, severity, sources)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            let mut times = transaction.prepare_cached(
                "INSERT INTO record_time (record_id, field, run_id, role, epoch_us, zoned)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for (record, prepared) in records.iter().zip(&prepared) {
                let table = &self.types[&record.record_type];
                envelope.execute(params![
                    self.run_id,
                    table.type_id,
                    prepared.title,
                    prepared.severity,
                    prepared.sources,
                ])?;
                let record_id = transaction.last_insert_rowid();
                table.insert(&transaction, record_id, &record.values)?;
                for point in table.time_points(&record.values) {
                    times.execute(params![
                        record_id,
                        point.field,
                        self.run_id,
                        point.role,
                        point.epoch_us,
                        point.zoned,
                    ])?;
                    time_points += 1;
                }
                *per_type.entry(table.type_id).or_default() += 1;
            }
        }
        for (type_id, count) in &per_type {
            transaction.execute(
                "INSERT INTO run_type (run_id, type_id, record_count) VALUES (?1, ?2, ?3)
                 ON CONFLICT (run_id, type_id)
                 DO UPDATE SET record_count = record_count + excluded.record_count",
                params![self.run_id, type_id, count],
            )?;
        }
        let accepted = records.len() as u64;
        transaction.execute(
            "UPDATE run SET record_count = record_count + ?2 WHERE run_id = ?1",
            params![self.run_id, accepted as i64],
        )?;
        transaction.commit()?;
        self.records += accepted;
        self.time_points += time_points;
        Ok(accepted)
    }

    /// Stores a warning of the run.
    pub fn warn(
        &mut self,
        code: &str,
        message: &str,
        path: Option<&str>,
        node_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let transaction = evidence_db::write_transaction(&self.connection)?;
        if self.warnings < MAX_STORED_WARNINGS {
            transaction.execute(
                "INSERT INTO run_warning (run_id, code, message, path, node_id)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    self.run_id,
                    truncate(code, 64),
                    truncate(message, MAX_WARNING_TEXT),
                    path,
                    node_id
                ],
            )?;
        }
        transaction.execute(
            "UPDATE run SET warning_count = warning_count + 1 WHERE run_id = ?1",
            [&self.run_id],
        )?;
        transaction.commit()?;
        self.warnings += 1;
        Ok(())
    }

    fn prepare(&self, record: &WireRecord) -> Result<PreparedRecord, RecordWriteError> {
        let table = self.types.get(&record.record_type).ok_or_else(|| {
            rejected(
                "UNKNOWN_RECORD_TYPE",
                format!(
                    "record type {} is not in the plugin's schema",
                    record.record_type
                ),
            )
        })?;
        table
            .schema
            .validate_values(&record.values)
            .map_err(|message| {
                rejected(
                    "INVALID_RECORD",
                    format!("{}: {message}", record.record_type),
                )
            })?;
        let title = match &record.title {
            Some(title) if !title.trim().is_empty() => truncate(title, 1024).to_owned(),
            Some(_) | None => table.schema.render_title(&record.values),
        };
        if record.sources.len() > MAX_SOURCES {
            return Err(rejected(
                "INVALID_RECORD",
                format!("a record cites at most {MAX_SOURCES} sources"),
            ));
        }
        for source in &record.sources {
            validate_source(source)?;
        }
        let sources: Vec<RecordSource> = record.sources.iter().map(RecordSource::from).collect();
        Ok(PreparedRecord {
            title,
            severity: record.severity.map(severity_name),
            sources: serde_json::to_string(&sources).map_err(anyhow::Error::from)?,
        })
    }
}

struct PreparedRecord {
    title: String,
    severity: Option<&'static str>,
    sources: String,
}

impl From<&SourceRef> for RecordSource {
    fn from(source: &SourceRef) -> Self {
        Self {
            root: source.root.clone(),
            path: source.path.clone(),
            node_id: source.node_id.clone(),
            offset: source.offset,
            length: source.length,
            note: source.note.clone(),
        }
    }
}

fn validate_source(source: &SourceRef) -> Result<(), RecordWriteError> {
    for text in [&source.root, &source.path, &source.node_id, &source.note]
        .into_iter()
        .flatten()
    {
        if text.len() > MAX_SOURCE_TEXT {
            return Err(rejected(
                "INVALID_RECORD",
                format!("source text exceeds {MAX_SOURCE_TEXT} bytes"),
            ));
        }
    }
    Ok(())
}

fn severity_name(severity: lovelycarve_plugin::Severity) -> &'static str {
    match severity {
        lovelycarve_plugin::Severity::Info => "info",
        lovelycarve_plugin::Severity::Low => "low",
        lovelycarve_plugin::Severity::Medium => "medium",
        lovelycarve_plugin::Severity::High => "high",
        lovelycarve_plugin::Severity::Critical => "critical",
    }
}

/// `text` cut to at most `limit` bytes on a character boundary.
fn truncate(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests;
