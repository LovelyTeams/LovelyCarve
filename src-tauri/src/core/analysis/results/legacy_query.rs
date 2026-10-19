//! 旧版结果(`analysis.db`)的只读查询:结果树的功能与结果项节点、分页记录
//! 与单条详情,投影成与新版记录相同的模型(`legacy = true`)。
//!
//! 每个 (插件, 功能, 输入文件) 只展示最新一次运行;结果项名与列定义来自
//! 当时插件提交的 `plugin_result_nodes`,有记录但未声明的类型按类型名合成
//! 节点。旧结果没有字段结构与来源定位,列一律按文本展示。

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, ToSql};
use serde::Deserialize;

use crate::core::{
    analysis::results::{
        legacy_store,
        models::{
            AnalysisRecord, AnalysisRecordsPage, AnalysisTreeNode, RecordColumn, RecordSource,
        },
        record_query::MAX_PAGE_LIMIT,
    },
    casework::evidence_db,
};

struct LatestRun {
    run_id: String,
    plugin_id: String,
    node_id: String,
    file_name: String,
    analyzer: String,
    display_name: String,
    type_name: String,
    completed_at: String,
}

#[derive(Deserialize)]
struct LegacyColumn {
    key: String,
    label: String,
}

/// The evidence's `analysis.db`, or `None` (no legacy results; never created).
fn open(case_dir: &std::path::Path, image_id: &str) -> anyhow::Result<Option<Connection>> {
    evidence_db::open_reader(case_dir, image_id, &legacy_store::SCHEMA)
}

/// `(category, function node)` of every latest legacy run with records.
pub fn function_nodes(
    case_dir: &std::path::Path,
    image_id: &str,
) -> anyhow::Result<Vec<(String, AnalysisTreeNode)>> {
    let Some(connection) = open(case_dir, image_id)? else {
        return Ok(Vec::new());
    };
    let mut functions = Vec::new();
    for run in latest_runs(&connection)? {
        let declared = declared_nodes(&connection, &run.run_id)?;
        let counts = artifact_counts(&connection, &run.run_id)?;
        let children = result_nodes(&declared, &counts, image_id, &run);
        if children.is_empty() {
            continue;
        }
        let category = if run.type_name.is_empty() {
            "analysis".to_owned()
        } else {
            run.type_name.clone()
        };
        let label = if run.display_name.is_empty() {
            run.analyzer.clone()
        } else {
            run.display_name.clone()
        };
        let target_label = if run.node_id.is_empty() {
            String::new()
        } else if run.file_name.is_empty() {
            run.node_id.clone()
        } else {
            run.file_name.clone()
        };
        functions.push((
            category.clone(),
            AnalysisTreeNode {
                node_id: scope_id(image_id, &run),
                kind: "function",
                label,
                image_id: image_id.to_owned(),
                type_name: category,
                plugin_id: run.plugin_id.clone(),
                analyzer: run.analyzer.clone(),
                function: format!("{}/{}", run.plugin_id, run.analyzer),
                source_node_id: (!run.node_id.is_empty()).then(|| run.node_id.clone()),
                target_label,
                artifact_type: String::new(),
                run_id: run.run_id.clone(),
                completed_at: run.completed_at.clone(),
                child_count: children.len() as u64,
                warning_count: 0,
                legacy: true,
                children,
            },
        ));
    }
    Ok(functions)
}

fn scope_id(image_id: &str, run: &LatestRun) -> String {
    format!(
        "{image_id}::legacy::{}/{}::{}",
        run.plugin_id,
        run.analyzer,
        hex::encode(run.node_id.as_bytes())
    )
}

/// Declared nodes first (zero-record ones skipped), then record types the
/// run stored without declaring, by type name.
fn result_nodes(
    declared: &[(String, String)],
    counts: &HashMap<String, u64>,
    image_id: &str,
    run: &LatestRun,
) -> Vec<AnalysisTreeNode> {
    let node = |record_type: &str, label: &str, count: u64| AnalysisTreeNode {
        node_id: format!("{}::{record_type}", scope_id(image_id, run)),
        kind: "result",
        label: label.to_owned(),
        image_id: image_id.to_owned(),
        type_name: String::new(),
        plugin_id: run.plugin_id.clone(),
        analyzer: run.analyzer.clone(),
        function: format!("{}/{}", run.plugin_id, run.analyzer),
        source_node_id: (!run.node_id.is_empty()).then(|| run.node_id.clone()),
        target_label: String::new(),
        artifact_type: record_type.to_owned(),
        run_id: run.run_id.clone(),
        completed_at: String::new(),
        child_count: count,
        warning_count: 0,
        legacy: true,
        children: Vec::new(),
    };
    let mut children = Vec::new();
    let mut seen = HashSet::new();
    for (record_type, label) in declared {
        if let Some(&count) = counts.get(record_type).filter(|count| **count > 0) {
            seen.insert(record_type.clone());
            children.push(node(record_type, label, count));
        }
    }
    let mut undeclared: Vec<(&String, &u64)> = counts
        .iter()
        .filter(|(record_type, count)| **count > 0 && !seen.contains(*record_type))
        .collect();
    undeclared.sort_by(|left, right| left.0.cmp(right.0));
    for (record_type, count) in undeclared {
        children.push(node(record_type, record_type, *count));
    }
    children
}

/// A page of one record type of a legacy run.
pub struct LegacyRecordsQuery<'a> {
    pub image_id: &'a str,
    pub run_id: &'a str,
    pub record_type: &'a str,
    pub keyword: Option<&'a str>,
    /// `title` or a column key; stored order when absent.
    pub sort: Option<&'a str>,
    pub descending: bool,
    pub skip: i64,
    pub limit: i64,
}

pub fn records_page(
    case_dir: &std::path::Path,
    query: &LegacyRecordsQuery<'_>,
) -> anyhow::Result<AnalysisRecordsPage> {
    let empty = AnalysisRecordsPage {
        total: 0,
        columns: Vec::new(),
        properties: None,
        items: Vec::new(),
    };
    let Some(connection) = open(case_dir, query.image_id)? else {
        return Ok(empty);
    };
    let Some(node_id) = run_node(&connection, query.run_id)? else {
        return Ok(empty);
    };
    let columns = declared_columns(&connection, query.run_id, query.record_type)?;
    let mut values: Vec<Box<dyn ToSql>> = vec![
        Box::new(query.run_id.to_owned()),
        Box::new(query.record_type.to_owned()),
    ];
    let mut filter = "";
    if let Some(keyword) = query
        .keyword
        .map(str::trim)
        .filter(|keyword| !keyword.is_empty())
    {
        filter = " AND (title LIKE ?3 ESCAPE '\\' OR attributes_json LIKE ?3 ESCAPE '\\')";
        values.push(Box::new(format!("%{}%", escape_like(keyword))));
    }
    let parameters: Vec<&dyn ToSql> = values.iter().map(AsRef::as_ref).collect();
    let total: i64 = connection.query_row(
        &format!(
            "SELECT COUNT(*) FROM plugin_artifacts
             WHERE run_id = ?1 AND artifact_type = ?2{filter}"
        ),
        parameters.as_slice(),
        |row| row.get(0),
    )?;
    let direction = if query.descending { "DESC" } else { "ASC" };
    let order = match query.sort {
        Some("title") => format!("title {direction}, rowid {direction}"),
        Some(key) if columns.iter().any(|column| column.id == key) => {
            // Column keys are plugin-declared; bind them, never splice them.
            values.push(Box::new(format!("$.\"{}\"", key.replace('"', ""))));
            format!(
                "json_extract(attributes_json, ?{}) {direction}, rowid {direction}",
                values.len()
            )
        }
        Some(_) | None => format!("rowid {direction}"),
    };
    let parameters: Vec<&dyn ToSql> = values.iter().map(AsRef::as_ref).collect();
    let limit = query.limit.clamp(1, MAX_PAGE_LIMIT);
    let skip = query.skip.max(0);
    let mut statement = connection.prepare(&format!(
        "SELECT artifact_id, artifact_type, title, value_json, attributes_json
         FROM plugin_artifacts WHERE run_id = ?1 AND artifact_type = ?2{filter}
         ORDER BY {order} LIMIT {limit} OFFSET {skip}"
    ))?;
    let items = statement
        .query_map(parameters.as_slice(), |row| {
            record_from_row(row, query.run_id, query.image_id, &node_id)
        })?
        .collect::<Result<_, _>>()?;
    Ok(AnalysisRecordsPage {
        total: u64::try_from(total).unwrap_or_default(),
        columns,
        properties: None,
        items,
    })
}

/// One legacy record with its node's columns.
pub fn record_detail(
    case_dir: &std::path::Path,
    image_id: &str,
    artifact_id: &str,
) -> anyhow::Result<Option<(AnalysisRecord, Vec<RecordColumn>)>> {
    let Some(connection) = open(case_dir, image_id)? else {
        return Ok(None);
    };
    let row = connection
        .query_row(
            "SELECT a.artifact_id, a.artifact_type, a.title, a.value_json, a.attributes_json,
                    a.run_id, r.node_id
             FROM plugin_artifacts a JOIN plugin_analysis_runs r ON r.run_id = a.run_id
             WHERE a.artifact_id = ?1",
            [artifact_id],
            |row| {
                let run_id: String = row.get(5)?;
                let node_id: String = row.get(6)?;
                Ok((record_from_row(row, &run_id, image_id, &node_id)?, run_id))
            },
        )
        .optional()?;
    let Some((record, run_id)) = row else {
        return Ok(None);
    };
    let columns = declared_columns(&connection, &run_id, &record.record_type)?;
    Ok(Some((record, columns)))
}

fn record_from_row(
    row: &rusqlite::Row<'_>,
    run_id: &str,
    image_id: &str,
    node_id: &str,
) -> rusqlite::Result<AnalysisRecord> {
    let json =
        |index: usize, text: Option<String>| -> rusqlite::Result<Option<serde_json::Value>> {
            text.map(|text| serde_json::from_str(&text))
                .transpose()
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        index,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
        };
    let value = json(3, row.get(3)?)?;
    let attributes = json(4, Some(row.get(4)?))?;
    let mut values = match attributes {
        Some(serde_json::Value::Object(map)) => map,
        Some(_) | None => serde_json::Map::new(),
    };
    if let Some(value) = value.filter(|value| !value.is_null()) {
        values.entry("value").or_insert(value);
    }
    Ok(AnalysisRecord {
        record_id: row.get(0)?,
        run_id: run_id.to_owned(),
        record_type: row.get(1)?,
        title: row.get(2)?,
        severity: None,
        values,
        sources: if node_id.is_empty() {
            Vec::new()
        } else {
            vec![RecordSource {
                node_id: Some(node_id.to_owned()),
                ..RecordSource::default()
            }]
        },
        image_id: image_id.to_owned(),
        legacy: true,
    })
}

fn run_node(connection: &Connection, run_id: &str) -> anyhow::Result<Option<String>> {
    connection
        .query_row(
            "SELECT node_id FROM plugin_analysis_runs WHERE run_id = ?1",
            [run_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
}

/// The columns the run declared for `record_type`, as text columns.
fn declared_columns(
    connection: &Connection,
    run_id: &str,
    record_type: &str,
) -> anyhow::Result<Vec<RecordColumn>> {
    let columns: Option<String> = connection
        .query_row(
            "SELECT columns_json FROM plugin_result_nodes
             WHERE run_id = ?1 AND node_type = ?2 ORDER BY rowid LIMIT 1",
            [run_id, record_type],
            |row| row.get(0),
        )
        .optional()?;
    let Some(columns) = columns else {
        return Ok(Vec::new());
    };
    Ok(serde_json::from_str::<Vec<LegacyColumn>>(&columns)?
        .into_iter()
        .map(|column| RecordColumn {
            id: column.key,
            label: column.label,
            kind: "text",
            key: false,
            hidden: false,
            timeline: None,
            choices: Vec::new(),
        })
        .collect())
}

fn escape_like(keyword: &str) -> String {
    keyword
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// The latest run of every (plugin, analyzer, input file).
fn latest_runs(connection: &Connection) -> anyhow::Result<Vec<LatestRun>> {
    let mut statement = connection.prepare(
        "SELECT run_id, analyzer, display_name, type_name, completed_at, plugin_id, node_id,
                file_name
         FROM plugin_analysis_runs
         ORDER BY analyzer, completed_at DESC, run_id DESC",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(LatestRun {
            run_id: row.get(0)?,
            analyzer: row.get(1)?,
            display_name: row.get(2)?,
            type_name: row.get(3)?,
            completed_at: row.get(4)?,
            plugin_id: row.get(5)?,
            node_id: row.get(6)?,
            file_name: row.get(7)?,
        })
    })?;
    let mut latest = Vec::new();
    let mut seen = HashSet::new();
    for row in rows {
        let run = row?;
        if seen.insert((
            run.plugin_id.clone(),
            run.analyzer.clone(),
            run.node_id.clone(),
        )) {
            latest.push(run);
        }
    }
    Ok(latest)
}

/// `(node type, display name)` in declaration order.
fn declared_nodes(connection: &Connection, run_id: &str) -> anyhow::Result<Vec<(String, String)>> {
    let mut statement = connection.prepare(
        "SELECT node_type, display_name FROM plugin_result_nodes
         WHERE run_id = ?1 ORDER BY rowid",
    )?;
    let rows = statement.query_map([run_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect::<Result<_, _>>().map_err(Into::into)
}

fn artifact_counts(connection: &Connection, run_id: &str) -> anyhow::Result<HashMap<String, u64>> {
    let mut statement = connection.prepare(
        "SELECT artifact_type, COUNT(*) FROM plugin_artifacts
         WHERE run_id = ?1 GROUP BY artifact_type",
    )?;
    let rows = statement.query_map([run_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    rows.map(|row| {
        let (record_type, count) = row?;
        Ok((record_type, u64::try_from(count).unwrap_or_default()))
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::analysis::results::legacy_store::test_support::{
        persist, LegacyArtifact, LegacyRun,
    };

    fn run<'a>(completed_at: &'a str, artifacts: Vec<LegacyArtifact<'a>>) -> LegacyRun<'a> {
        LegacyRun {
            plugin_id: "org.lovelycarve.linuxinfo",
            analyzer: "linuxbaseinfo",
            display_name: "Linux基本信息",
            type_name: "baseinfo",
            node_id: "",
            file_name: "",
            completed_at,
            nodes: vec![
                ("linux.user", "用户列表", vec![("linux.username", "用户名")]),
                ("linux.usb", "USB使用记录", Vec::new()),
            ],
            artifacts,
            events: Vec::new(),
        }
    }

    fn user(title: &str, name: &str) -> LegacyArtifact<'static> {
        LegacyArtifact {
            artifact_type: "linux.user",
            title: Box::leak(title.to_owned().into_boxed_str()),
            attributes: serde_json::json!({ "linux.username": name }),
        }
    }

    #[test]
    fn latest_legacy_runs_become_read_only_function_nodes_and_pages() {
        let directory = tempfile::tempdir().expect("case");
        persist(
            directory.path(),
            "image-1",
            &run("2026-09-01T00:00:01Z", vec![user("用户 root", "root")]),
        );
        let latest = persist(
            directory.path(),
            "image-1",
            &run(
                "2026-09-02T00:00:01Z",
                vec![
                    user("用户 root", "root"),
                    user("用户 daemon", "daemon"),
                    LegacyArtifact {
                        artifact_type: "linux.unknown",
                        title: "未声明类型",
                        attributes: serde_json::json!({}),
                    },
                ],
            ),
        );
        let functions = function_nodes(directory.path(), "image-1").expect("tree");
        assert_eq!(functions.len(), 1);
        let (category, function) = &functions[0];
        assert_eq!(category, "baseinfo");
        assert!(function.legacy);
        assert_eq!(function.run_id, latest);
        assert_eq!(
            function
                .children
                .iter()
                .map(|node| (
                    node.artifact_type.as_str(),
                    node.label.as_str(),
                    node.child_count
                ))
                .collect::<Vec<_>>(),
            vec![
                ("linux.user", "用户列表", 2),
                ("linux.unknown", "linux.unknown", 1)
            ]
        );

        let page = records_page(
            directory.path(),
            &LegacyRecordsQuery {
                image_id: "image-1",
                run_id: &latest,
                record_type: "linux.user",
                keyword: Some("daemon"),
                sort: None,
                descending: false,
                skip: 0,
                limit: 100,
            },
        )
        .expect("page");
        assert_eq!(page.total, 1);
        assert_eq!(page.columns[0].id, "linux.username");
        let record = &page.items[0];
        assert_eq!(record.title, "用户 daemon");
        assert_eq!(record.values["linux.username"], "daemon");
        assert!(record.legacy);

        let sorted = records_page(
            directory.path(),
            &LegacyRecordsQuery {
                image_id: "image-1",
                run_id: &latest,
                record_type: "linux.user",
                keyword: None,
                sort: Some("linux.username"),
                descending: false,
                skip: 0,
                limit: 100,
            },
        )
        .expect("sorted page");
        assert_eq!(
            sorted
                .items
                .iter()
                .map(|record| record.values["linux.username"].as_str())
                .collect::<Vec<_>>(),
            vec![Some("daemon"), Some("root")]
        );

        let (detail, columns) = record_detail(directory.path(), "image-1", &record.record_id)
            .expect("detail")
            .expect("present");
        assert_eq!(&detail, record);
        assert_eq!(columns, page.columns);
    }

    #[test]
    fn evidence_without_legacy_results_reads_nothing_and_creates_nothing() {
        let directory = tempfile::tempdir().expect("case");
        assert!(function_nodes(directory.path(), "image-1")
            .expect("tree")
            .is_empty());
        assert!(!crate::core::casework::case_paths::evidence_db_path(
            directory.path(),
            "image-1",
            crate::core::casework::case_paths::EvidenceDb::Analysis
        )
        .exists());
    }
}
