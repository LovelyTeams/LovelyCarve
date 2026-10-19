//! 记录库查询:已发布运行、运行的记录类型、类型化分页与单条详情、运行审计、
//! 画像事实与时间线投影源。读路径不建库——检材还没有 `records.db` 就是
//! 还没有结果。

use std::{collections::BTreeMap, path::Path};

use lovelycarve_plugin::schema::{FieldKind, RecordSchema};
use rusqlite::{Connection, OptionalExtension, ToSql};
use serde::Serialize;

use crate::core::{
    analysis::results::{
        models::{ColumnChoice, RecordColumn, RecordSource},
        record_store::{
            self, column_json, registered_schema, sort_column, table_name, value_column, RunInput,
            RunTarget,
        },
    },
    casework::evidence_db,
    evidence::image::BadRegion,
};

/// Most records one page returns.
pub const MAX_PAGE_LIMIT: i64 = 500;
/// The record type of profile facts (SDK `Run::provide`).
pub const PROFILE_FACT: &str = "profile.fact";

/// The evidence's `records.db`, or `None` before its first run.
pub fn open(case_dir: &Path, image_id: &str) -> anyhow::Result<Option<Connection>> {
    evidence_db::open_reader(case_dir, image_id, &record_store::SCHEMA)
}

/// A published run, as the result tree lists it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishedRun {
    pub run_id: String,
    pub job_key: String,
    pub plugin_id: String,
    pub plugin_version: String,
    pub analyzer: String,
    pub analyzer_name: String,
    pub category: String,
    pub target: RunTarget,
    pub summary: Option<String>,
    pub record_count: u64,
    pub warning_count: u64,
    pub synthetic_bytes: u64,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub published_at: String,
}

const RUN_COLUMNS: &str = "run_id, job_key, plugin_id, plugin_version, analyzer, analyzer_name, \
category, target_kind, target_id, target_label, summary, record_count, warning_count, \
synthetic_bytes, started_at, finished_at, published_at";

fn run_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PublishedRun> {
    Ok(PublishedRun {
        run_id: row.get(0)?,
        job_key: row.get(1)?,
        plugin_id: row.get(2)?,
        plugin_version: row.get(3)?,
        analyzer: row.get(4)?,
        analyzer_name: row.get(5)?,
        category: row.get(6)?,
        target: RunTarget {
            kind: row.get(7)?,
            id: row.get(8)?,
            label: row.get(9)?,
        },
        summary: row.get(10)?,
        record_count: count(row.get(11)?),
        warning_count: count(row.get(12)?),
        synthetic_bytes: count(row.get(13)?),
        started_at: row.get(14)?,
        finished_at: row.get(15)?,
        published_at: row.get::<_, Option<String>>(16)?.unwrap_or_default(),
    })
}

fn count(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

/// Every published run of the evidence: one per job.
pub fn published_runs(connection: &Connection) -> anyhow::Result<Vec<PublishedRun>> {
    let mut statement = connection.prepare(&format!(
        "SELECT {RUN_COLUMNS} FROM run WHERE state = 'published'
         ORDER BY category, analyzer_name, target_label, run_id"
    ))?;
    let rows = statement.query_map([], run_from_row)?;
    rows.collect::<Result<_, _>>().map_err(Into::into)
}

/// The published run `run_id`, or `None`.
pub fn published_run(
    connection: &Connection,
    run_id: &str,
) -> anyhow::Result<Option<PublishedRun>> {
    connection
        .query_row(
            &format!("SELECT {RUN_COLUMNS} FROM run WHERE run_id = ?1 AND state = 'published'"),
            [run_id],
            run_from_row,
        )
        .optional()
        .map_err(Into::into)
}

/// A record type a run emitted.
#[derive(Clone, Debug, PartialEq)]
pub struct RunRecordType {
    pub type_id: i64,
    pub schema: RecordSchema,
    pub record_count: u64,
}

/// The record types a run emitted, profile facts excluded, in type order.
pub fn run_record_types(
    connection: &Connection,
    run_id: &str,
) -> anyhow::Result<Vec<RunRecordType>> {
    let mut statement = connection.prepare(
        "SELECT t.type_id, t.schema, u.record_count
         FROM run_type u JOIN record_type t ON t.type_id = u.type_id
         WHERE u.run_id = ?1 AND u.record_count > 0 AND t.record_type <> ?2
         ORDER BY t.type_id",
    )?;
    let rows = statement.query_map(rusqlite::params![run_id, PROFILE_FACT], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    let mut types = Vec::new();
    for row in rows {
        let (type_id, schema, record_count) = row?;
        types.push(RunRecordType {
            type_id,
            schema: serde_json::from_str(&schema)?,
            record_count: count(record_count),
        });
    }
    Ok(types)
}

/// The columns of `schema`, in field order.
pub fn columns(schema: &RecordSchema) -> Vec<RecordColumn> {
    schema
        .fields
        .iter()
        .map(|field| RecordColumn {
            id: field.id.clone(),
            label: field.label.clone(),
            kind: field.kind.as_str(),
            key: field.key,
            hidden: field.hidden,
            timeline: field.timeline.clone(),
            choices: field
                .choices
                .iter()
                .map(|choice| ColumnChoice {
                    id: choice.id.clone(),
                    label: choice.label.clone(),
                })
                .collect(),
        })
        .collect()
}

/// One stored record.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredRecord {
    pub record_id: String,
    pub run_id: String,
    pub record_type: String,
    pub title: String,
    pub severity: Option<String>,
    /// Values by field id.
    pub values: serde_json::Map<String, serde_json::Value>,
    pub sources: Vec<RecordSource>,
}

/// A page of one record type of one run.
pub struct RecordsQuery<'a> {
    pub run_id: &'a str,
    pub record_type: &'a str,
    /// Matches the title and every text-like column.
    pub keyword: Option<&'a str>,
    /// A field id, or `title`; emission order when absent.
    pub sort: Option<&'a str>,
    pub descending: bool,
    pub skip: i64,
    pub limit: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordsPage {
    pub total: u64,
    pub columns: Vec<RecordColumn>,
    pub properties: Option<lovelycarve_plugin::schema::PropertyLayout>,
    pub items: Vec<StoredRecord>,
}

/// One page of records (see [`RecordsQuery`]).
pub fn records_page(
    connection: &Connection,
    query: &RecordsQuery<'_>,
) -> anyhow::Result<RecordsPage> {
    let type_id: Option<i64> = connection
        .query_row(
            "SELECT t.type_id FROM run_type u JOIN record_type t ON t.type_id = u.type_id
             WHERE u.run_id = ?1 AND t.record_type = ?2",
            rusqlite::params![query.run_id, query.record_type],
            |row| row.get(0),
        )
        .optional()?;
    let Some(type_id) = type_id else {
        return Ok(RecordsPage {
            total: 0,
            columns: Vec::new(),
            properties: None,
            items: Vec::new(),
        });
    };
    let schema = registered_schema(connection, type_id)?
        .ok_or_else(|| anyhow::anyhow!("record type {type_id} is not registered"))?;
    let table = table_name(type_id);
    let mut filter = String::new();
    let mut parameters: Vec<Box<dyn ToSql>> =
        vec![Box::new(query.run_id.to_owned()), Box::new(type_id)];
    if let Some(keyword) = query
        .keyword
        .map(str::trim)
        .filter(|keyword| !keyword.is_empty())
    {
        let mut alternatives = vec!["r.title LIKE ?3 ESCAPE '\\'".to_owned()];
        for (index, field) in schema.fields.iter().enumerate() {
            if text_column(field.kind) {
                alternatives.push(format!("t.{} LIKE ?3 ESCAPE '\\'", value_column(index)));
            }
        }
        filter = format!(" AND ({})", alternatives.join(" OR "));
        parameters.push(Box::new(format!("%{}%", escape_like(keyword))));
    }
    let order = match query.sort {
        Some("title") => "r.title".to_owned(),
        Some(field) => schema
            .field_index(field)
            .and_then(|index| sort_column(&schema, index))
            .map_or_else(|| "r.record_id".to_owned(), |column| format!("t.{column}")),
        None => "r.record_id".to_owned(),
    };
    let direction = if query.descending { "DESC" } else { "ASC" };
    let source = format!(
        "FROM record r JOIN {table} t ON t.record_id = r.record_id
         WHERE r.run_id = ?1 AND r.type_id = ?2{filter}"
    );
    let parameter_refs: Vec<&dyn ToSql> = parameters.iter().map(AsRef::as_ref).collect();
    let total: i64 = connection.query_row(
        &format!("SELECT COUNT(*) {source}"),
        parameter_refs.as_slice(),
        |row| row.get(0),
    )?;
    let limit = query.limit.clamp(1, MAX_PAGE_LIMIT);
    let skip = query.skip.max(0);
    let value_columns = (0..schema.fields.len())
        .map(|index| format!("t.{}", value_column(index)))
        .collect::<Vec<_>>();
    let select = if value_columns.is_empty() {
        String::new()
    } else {
        format!(", {}", value_columns.join(", "))
    };
    let mut statement = connection.prepare(&format!(
        "SELECT r.record_id, r.title, r.severity, r.sources{select} {source}
         ORDER BY {order} {direction}, r.record_id {direction} LIMIT {limit} OFFSET {skip}"
    ))?;
    let items = statement
        .query_map(parameter_refs.as_slice(), |row| {
            record_from_row(row, query.run_id, &schema)
        })?
        .collect::<Result<_, _>>()?;
    Ok(RecordsPage {
        total: count(total),
        columns: columns(&schema),
        properties: schema.properties.clone(),
        items,
    })
}

/// One record with its type's columns, by id.
pub fn record_detail(
    connection: &Connection,
    record_id: i64,
) -> anyhow::Result<Option<(StoredRecord, Vec<RecordColumn>)>> {
    let envelope: Option<(String, i64)> = connection
        .query_row(
            "SELECT run_id, type_id FROM record WHERE record_id = ?1",
            [record_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((run_id, type_id)) = envelope else {
        return Ok(None);
    };
    let Some(schema) = registered_schema(connection, type_id)? else {
        return Ok(None);
    };
    let value_columns = (0..schema.fields.len())
        .map(|index| format!(", t.{}", value_column(index)))
        .collect::<String>();
    let record = connection
        .query_row(
            &format!(
                "SELECT r.record_id, r.title, r.severity, r.sources{value_columns}
                 FROM record r JOIN {} t ON t.record_id = r.record_id
                 WHERE r.record_id = ?1",
                table_name(type_id)
            ),
            [record_id],
            |row| record_from_row(row, &run_id, &schema),
        )
        .optional()?;
    Ok(record.map(|record| (record, columns(&schema))))
}

fn record_from_row(
    row: &rusqlite::Row<'_>,
    run_id: &str,
    schema: &RecordSchema,
) -> rusqlite::Result<StoredRecord> {
    let sources: String = row.get(3)?;
    let mut values = serde_json::Map::new();
    for (index, field) in schema.fields.iter().enumerate() {
        values.insert(field.id.clone(), column_json(field.kind, row, 4 + index)?);
    }
    Ok(StoredRecord {
        record_id: row.get::<_, i64>(0)?.to_string(),
        run_id: run_id.to_owned(),
        record_type: schema.record_type.clone(),
        title: row.get(1)?,
        severity: row.get(2)?,
        values,
        sources: serde_json::from_str(&sources).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
    })
}

fn text_column(kind: FieldKind) -> bool {
    match kind {
        FieldKind::Text
        | FieldKind::Timestamp
        | FieldKind::Path
        | FieldKind::Hash
        | FieldKind::Ip
        | FieldKind::Enum
        | FieldKind::List
        | FieldKind::Json => true,
        FieldKind::Int
        | FieldKind::Float
        | FieldKind::Bool
        | FieldKind::Size
        | FieldKind::Duration => false,
    }
}

/// `%` / `_` / `\` match themselves.
fn escape_like(keyword: &str) -> String {
    keyword
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// A warning a run kept.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunWarning {
    pub code: String,
    pub message: String,
    pub path: Option<String>,
    pub node_id: Option<String>,
}

/// A record type and its count, for the audit.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditRecordType {
    pub record_type: String,
    pub label: String,
    pub count: u64,
}

/// Everything recorded about one run: what it was given, what it produced,
/// what it could not do, and which source bytes were synthesized.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunAudit {
    pub run_id: String,
    pub state: String,
    pub plugin_id: String,
    pub plugin_version: String,
    pub analyzer: String,
    pub analyzer_name: String,
    pub target: RunTarget,
    pub options: serde_json::Value,
    pub profile: serde_json::Value,
    pub inputs: Vec<RunInput>,
    pub input_count: u64,
    pub schema_hash: String,
    pub summary: Option<String>,
    pub record_count: u64,
    pub record_types: Vec<AuditRecordType>,
    pub warning_count: u64,
    pub warnings: Vec<RunWarning>,
    pub bad_regions: Vec<BadRegion>,
    pub synthetic_bytes: u64,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub published_at: Option<String>,
}

/// Most warnings one audit lists.
const AUDIT_WARNINGS: i64 = 1000;

/// The audit of `run_id` in any state but staging.
pub fn run_audit(connection: &Connection, run_id: &str) -> anyhow::Result<Option<RunAudit>> {
    let row = connection
        .query_row(
            "SELECT state, plugin_id, plugin_version, analyzer, analyzer_name, target_kind,
                    target_id, target_label, options, profile, inputs, input_count, schema_hash,
                    summary, record_count, warning_count, bad_regions, synthetic_bytes,
                    started_at, finished_at, published_at
             FROM run WHERE run_id = ?1 AND state <> 'staging'",
            [run_id],
            |row| {
                Ok((
                    (
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ),
                    RunTarget {
                        kind: row.get(5)?,
                        id: row.get(6)?,
                        label: row.get(7)?,
                    },
                    (
                        row.get::<_, String>(8)?,
                        row.get::<_, String>(9)?,
                        row.get::<_, String>(10)?,
                        row.get::<_, i64>(11)?,
                        row.get::<_, String>(12)?,
                    ),
                    (
                        row.get::<_, Option<String>>(13)?,
                        row.get::<_, i64>(14)?,
                        row.get::<_, i64>(15)?,
                        row.get::<_, String>(16)?,
                        row.get::<_, i64>(17)?,
                    ),
                    (
                        row.get::<_, String>(18)?,
                        row.get::<_, Option<String>>(19)?,
                        row.get::<_, Option<String>>(20)?,
                    ),
                ))
            },
        )
        .optional()?;
    let Some((
        (state, plugin_id, plugin_version, analyzer, analyzer_name),
        target,
        (options, profile, inputs, input_count, schema_hash),
        (summary, record_count, warning_count, bad_regions, synthetic_bytes),
        (started_at, finished_at, published_at),
    )) = row
    else {
        return Ok(None);
    };
    let warnings = {
        let mut statement = connection.prepare(&format!(
            "SELECT code, message, path, node_id FROM run_warning WHERE run_id = ?1
             ORDER BY warning_id LIMIT {AUDIT_WARNINGS}"
        ))?;
        let rows = statement.query_map([run_id], |row| {
            Ok(RunWarning {
                code: row.get(0)?,
                message: row.get(1)?,
                path: row.get(2)?,
                node_id: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let record_types = {
        let mut statement = connection.prepare(
            "SELECT t.record_type, t.schema, u.record_count
             FROM run_type u JOIN record_type t ON t.type_id = u.type_id
             WHERE u.run_id = ?1 ORDER BY t.type_id",
        )?;
        let rows = statement.query_map([run_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let mut types = Vec::new();
        for row in rows {
            let (record_type, schema, record_count) = row?;
            let schema: RecordSchema = serde_json::from_str(&schema)?;
            types.push(AuditRecordType {
                record_type,
                label: schema.label,
                count: count(record_count),
            });
        }
        types
    };
    Ok(Some(RunAudit {
        run_id: run_id.to_owned(),
        state,
        plugin_id,
        plugin_version,
        analyzer,
        analyzer_name,
        target,
        options: serde_json::from_str(&options)?,
        profile: serde_json::from_str(&profile)?,
        inputs: serde_json::from_str(&inputs)?,
        input_count: count(input_count),
        schema_hash,
        summary,
        record_count: count(record_count),
        record_types,
        warning_count: count(warning_count),
        warnings,
        bad_regions: serde_json::from_str(&bad_regions)?,
        synthetic_bytes: count(synthetic_bytes),
        started_at,
        finished_at,
        published_at,
    }))
}

/// Facts known for `target_id`: those published for the whole evidence,
/// overridden by those published for the target itself. Within each, a
/// later publication of a key wins. The scheduler accepts the same two
/// sources when an automatic run waits for a fact.
pub fn profile_facts(
    connection: &Connection,
    target_id: &str,
) -> anyhow::Result<BTreeMap<String, serde_json::Value>> {
    let mut facts = BTreeMap::new();
    let mut statement =
        connection.prepare("SELECT type_id, schema FROM record_type WHERE record_type = ?1")?;
    let types = statement
        .query_map([PROFILE_FACT], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    // (published for the target itself, published_at, record id, key, value)
    let mut rows: Vec<(bool, String, i64, String, serde_json::Value)> = Vec::new();
    for (type_id, schema) in types {
        let schema: RecordSchema = serde_json::from_str(&schema)?;
        let (Some(key), Some(value)) = (schema.field_index("key"), schema.field_index("value"))
        else {
            continue;
        };
        let value_kind = schema.fields[value].kind;
        let mut query = connection.prepare(&format!(
            "SELECT t.{}, u.published_at, r.record_id, t.{}, u.target_id = ?2
             FROM record r
             JOIN run u ON u.run_id = r.run_id
             JOIN {} t ON t.record_id = r.record_id
             WHERE r.type_id = ?1 AND u.state = 'published' AND u.target_id IN (?2, '')",
            value_column(key),
            value_column(value),
            table_name(type_id)
        ))?;
        let found = query.query_map(rusqlite::params![type_id, target_id], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                row.get::<_, i64>(2)?,
                column_json(value_kind, row, 3)?,
                row.get::<_, bool>(4)?,
            ))
        })?;
        for row in found {
            let (key, published_at, record_id, value, own) = row?;
            if let Some(key) = key {
                rows.push((own, published_at, record_id, key, value));
            }
        }
    }
    rows.sort_by(|left, right| (left.0, &left.1, left.2).cmp(&(right.0, &right.1, right.2)));
    for (_, _, _, key, value) in rows {
        facts.insert(key, value);
    }
    Ok(facts)
}

/// One timeline point of a published record.
#[derive(Clone, Debug, PartialEq)]
pub struct TimePointSource {
    pub run_id: String,
    pub plugin_id: String,
    pub record_id: i64,
    pub record_type: String,
    pub field: String,
    pub role: String,
    pub epoch_us: i64,
    pub title: String,
}

const POINT_QUERY: &str = "SELECT p.run_id, u.plugin_id, p.record_id, t.record_type, p.field,
        p.role, p.epoch_us, r.title
     FROM record_time p
     JOIN record r ON r.record_id = p.record_id
     JOIN record_type t ON t.type_id = r.type_id
     JOIN run u ON u.run_id = p.run_id
     WHERE p.zoned = 1 AND u.state = 'published'";

fn point_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TimePointSource> {
    Ok(TimePointSource {
        run_id: row.get(0)?,
        plugin_id: row.get(1)?,
        record_id: row.get(2)?,
        record_type: row.get(3)?,
        field: row.get(4)?,
        role: row.get(5)?,
        epoch_us: row.get(6)?,
        title: row.get(7)?,
    })
}

/// Streams the projectable timeline points of the published runs. Local
/// times without a time zone are not projected: their UTC instant is
/// unknown.
pub fn visit_time_points(
    case_dir: &Path,
    image_id: &str,
    mut visit: impl FnMut(&TimePointSource) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let Some(connection) = open(case_dir, image_id)? else {
        return Ok(());
    };
    let mut statement = connection.prepare(&format!("{POINT_QUERY} ORDER BY p.record_id"))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        visit(&point_from_row(row)?)?;
    }
    Ok(())
}

/// The number of projectable timeline points.
pub fn count_time_points(case_dir: &Path, image_id: &str) -> anyhow::Result<u64> {
    let Some(connection) = open(case_dir, image_id)? else {
        return Ok(0);
    };
    let total: i64 = connection.query_row(
        "SELECT COUNT(*) FROM record_time p JOIN run u ON u.run_id = p.run_id
         WHERE p.zoned = 1 AND u.state = 'published'",
        [],
        |row| row.get(0),
    )?;
    Ok(count(total))
}

/// The projectable points of one published run with its publication time.
pub fn run_time_points(
    case_dir: &Path,
    image_id: &str,
    run_id: &str,
) -> anyhow::Result<Option<(String, Vec<TimePointSource>)>> {
    let Some(connection) = open(case_dir, image_id)? else {
        return Ok(None);
    };
    let Some(run) = published_run(&connection, run_id)? else {
        return Ok(None);
    };
    let mut statement = connection.prepare(&format!(
        "{POINT_QUERY} AND p.run_id = ?1 ORDER BY p.record_id"
    ))?;
    let points = statement
        .query_map([run_id], point_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some((run.published_at, points)))
}

/// The latest publication time (empty without runs): the timeline's source
/// watermark for plugin records.
pub fn max_published_at(case_dir: &Path, image_id: &str) -> anyhow::Result<String> {
    let Some(connection) = open(case_dir, image_id)? else {
        return Ok(String::new());
    };
    connection
        .query_row(
            "SELECT COALESCE(MAX(published_at), '') FROM run WHERE state = 'published'",
            [],
            |row| row.get(0),
        )
        .map_err(Into::into)
}
