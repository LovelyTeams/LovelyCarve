//! 插件结构化分析结果(runs / artifacts / timeline / findings / 结果节点),
//! 存在检材目录的 `analysis.db`。
//!
//! 写入并发:同一检材的多条插件任务可能同时提交。每次提交是一个 IMMEDIATE
//! 事务,开局即取写锁,其余提交在忙等上限内排队;单次结果受 LPP 帧上限约束,
//! 持锁时间有界。WAL 下读取(结果树、记录分页)不被提交阻塞,读路径从不
//! 建库。不同检材的提交写不同的库,互不排队。调用方都在阻塞线程上调用本
//! 模块。

use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Serialize;

use crate::core::{
    analysis::results::models::{
        PluginAnalysisArtifact, PluginAnalysisFinding, PluginAnalysisResult,
        PluginAnalysisTimelineEvent, PluginFindingSeverity, PluginResultNode,
        PluginResultNodeColumn,
    },
    casework::case_paths::EvidenceDb,
    casework::evidence_db::{self, StoreSchema},
};

/// `analysis.db` 结构。
pub(crate) const SCHEMA: StoreSchema = StoreSchema {
    db: EvidenceDb::Analysis,
    // ASCII "LCAN".
    application_id: 0x4C43_414E,
    version: 1,
    ddl: r#"
        CREATE TABLE plugin_analysis_runs (
            run_id TEXT PRIMARY KEY,
            plugin_id TEXT NOT NULL,
            plugin_version TEXT NOT NULL,
            analyzer TEXT NOT NULL,
            display_name TEXT NOT NULL,
            type_name TEXT NOT NULL,
            node_id TEXT NOT NULL,
            file_name TEXT NOT NULL,
            summary TEXT,
            started_at TEXT NOT NULL,
            completed_at TEXT NOT NULL
        );
        CREATE INDEX plugin_runs_by_node ON plugin_analysis_runs (node_id, completed_at DESC);
        CREATE INDEX plugin_runs_by_analyzer
            ON plugin_analysis_runs (analyzer, completed_at DESC);

        CREATE TABLE plugin_artifacts (
            artifact_id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL REFERENCES plugin_analysis_runs (run_id) ON DELETE CASCADE,
            artifact_type TEXT NOT NULL,
            title TEXT NOT NULL,
            value_json TEXT,
            attributes_json TEXT NOT NULL
        );
        CREATE INDEX plugin_artifacts_by_run ON plugin_artifacts (run_id, artifact_type);

        CREATE TABLE plugin_timeline_events (
            event_id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL REFERENCES plugin_analysis_runs (run_id) ON DELETE CASCADE,
            timestamp TEXT NOT NULL,
            event_type TEXT NOT NULL,
            description TEXT NOT NULL,
            attributes_json TEXT NOT NULL
        );
        CREATE INDEX plugin_timeline_by_run ON plugin_timeline_events (run_id, timestamp);

        CREATE TABLE plugin_findings (
            finding_id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL REFERENCES plugin_analysis_runs (run_id) ON DELETE CASCADE,
            severity TEXT NOT NULL,
            title TEXT NOT NULL,
            description TEXT NOT NULL,
            attributes_json TEXT NOT NULL
        );
        CREATE INDEX plugin_findings_by_run ON plugin_findings (run_id, severity);

        CREATE TABLE plugin_result_nodes (
            id INTEGER PRIMARY KEY,
            run_id TEXT NOT NULL REFERENCES plugin_analysis_runs (run_id) ON DELETE CASCADE,
            node_type TEXT NOT NULL,
            display_name TEXT NOT NULL,
            columns_json TEXT NOT NULL
        );
        CREATE INDEX plugin_result_nodes_by_run ON plugin_result_nodes (run_id);
    "#,
    busy_timeout: Duration::from_secs(60),
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredPluginAnalysis {
    pub run_id: String,
    pub plugin_id: String,
    pub plugin_version: String,
    pub analyzer: String,
    pub display_name: String,
    pub type_name: String,
    pub image_id: String,
    pub node_id: String,
    pub file_name: String,
    pub started_at: String,
    pub completed_at: String,
    pub result: PluginAnalysisResult,
}

pub struct NewPluginAnalysis<'a> {
    pub plugin_id: &'a str,
    pub plugin_version: &'a str,
    pub analyzer: &'a str,
    pub display_name: &'a str,
    pub type_name: &'a str,
    pub image_id: &'a str,
    pub node_id: &'a str,
    pub file_name: &'a str,
    pub started_at: &'a str,
    pub completed_at: &'a str,
    pub result: &'a PluginAnalysisResult,
}

/// 落库一次分析运行,返回 run id。
pub fn persist(case_dir: &Path, analysis: NewPluginAnalysis<'_>) -> anyhow::Result<String> {
    let connection = evidence_db::open_writer(case_dir, analysis.image_id, &SCHEMA)?;
    let transaction =
        evidence_db::write_transaction(&connection).context("begin analysis result write")?;
    let run_id = format!("run_{}", uuid::Uuid::new_v4().simple());
    transaction.execute(
        "INSERT INTO plugin_analysis_runs (
            run_id, plugin_id, plugin_version, analyzer, display_name, type_name,
            node_id, file_name, summary, started_at, completed_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            run_id,
            analysis.plugin_id,
            analysis.plugin_version,
            analysis.analyzer,
            analysis.display_name,
            analysis.type_name,
            analysis.node_id,
            analysis.file_name,
            analysis.result.summary,
            analysis.started_at,
            analysis.completed_at,
        ],
    )?;
    persist_nodes(&transaction, &run_id, &analysis.result.nodes)?;
    persist_artifacts(&transaction, &run_id, &analysis.result.artifacts)?;
    persist_timeline(&transaction, &run_id, &analysis.result.timeline)?;
    persist_findings(&transaction, &run_id, &analysis.result.findings)?;
    transaction.commit()?;
    Ok(run_id)
}

/// 一个文件节点上的全部分析运行,新的在前。
pub fn list_for_node(
    case_dir: &Path,
    image_id: &str,
    node_id: &str,
) -> anyhow::Result<Vec<StoredPluginAnalysis>> {
    let Some(connection) = evidence_db::open_reader(case_dir, image_id, &SCHEMA)? else {
        return Ok(Vec::new());
    };
    let mut statement = connection.prepare(
        "SELECT run_id, plugin_id, plugin_version, analyzer, display_name, type_name,
                node_id, file_name, summary, started_at, completed_at
         FROM plugin_analysis_runs
         WHERE node_id = ?1
         ORDER BY completed_at DESC, run_id DESC",
    )?;
    let rows = statement.query_map(params![node_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, Option<String>>(8)?,
            row.get::<_, String>(9)?,
            row.get::<_, String>(10)?,
        ))
    })?;
    let mut analyses = Vec::new();
    for row in rows {
        let (
            run_id,
            plugin_id,
            plugin_version,
            analyzer,
            display_name,
            type_name,
            node_id,
            file_name,
            summary,
            started_at,
            completed_at,
        ) = row?;
        analyses.push(StoredPluginAnalysis {
            result: PluginAnalysisResult {
                summary,
                nodes: load_nodes(&connection, &run_id)?,
                artifacts: load_artifacts(&connection, &run_id)?,
                timeline: load_timeline(&connection, &run_id)?,
                findings: load_findings(&connection, &run_id)?,
            },
            run_id,
            plugin_id,
            plugin_version,
            analyzer,
            display_name,
            type_name,
            image_id: image_id.to_string(),
            node_id,
            file_name,
            started_at,
            completed_at,
        });
    }
    Ok(analyses)
}

/// 删除一次运行,级联清空其产物 / 节点 / 事件 / 线索。
pub fn remove_run(case_dir: &Path, image_id: &str, run_id: &str) -> anyhow::Result<Vec<String>> {
    let Some(connection) =
        evidence_db::open_existing(case_dir, image_id, &SCHEMA, SCHEMA.busy_timeout)?
    else {
        return Ok(Vec::new());
    };
    // 同一事务内按固定运行身份选出并删除,不触碰同功能的其他运行。
    let transaction = evidence_db::write_transaction(&connection)?;
    let run_ids: Vec<String> = {
        let mut statement =
            transaction.prepare("SELECT run_id FROM plugin_analysis_runs WHERE run_id = ?1")?;
        let ids = statement
            .query_map(params![run_id], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        ids
    };
    if run_ids.is_empty() {
        return Ok(run_ids);
    }
    transaction.execute(
        "DELETE FROM plugin_analysis_runs WHERE run_id = ?1",
        params![run_id],
    )?;
    transaction.commit()?;
    Ok(run_ids)
}

fn persist_nodes(
    transaction: &Transaction<'_>,
    run_id: &str,
    nodes: &[PluginResultNode],
) -> anyhow::Result<()> {
    let mut statement = transaction.prepare(
        "INSERT INTO plugin_result_nodes (run_id, node_type, display_name, columns_json)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    for node in nodes {
        statement.execute(params![
            run_id,
            node.node_type,
            node.display_name,
            serde_json::to_string(&node.columns)?,
        ])?;
    }
    Ok(())
}

fn persist_artifacts(
    transaction: &Transaction<'_>,
    run_id: &str,
    artifacts: &[PluginAnalysisArtifact],
) -> anyhow::Result<()> {
    let mut statement = transaction.prepare(
        "INSERT INTO plugin_artifacts (
            artifact_id, run_id, artifact_type, title, value_json, attributes_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for artifact in artifacts {
        statement.execute(params![
            format!("artifact_{}", uuid::Uuid::new_v4().simple()),
            run_id,
            artifact.artifact_type,
            artifact.title,
            artifact
                .value
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
            serde_json::to_string(&artifact.attributes)?,
        ])?;
    }
    Ok(())
}

fn persist_timeline(
    transaction: &Transaction<'_>,
    run_id: &str,
    timeline: &[PluginAnalysisTimelineEvent],
) -> anyhow::Result<()> {
    let mut statement = transaction.prepare(
        "INSERT INTO plugin_timeline_events (
            event_id, run_id, timestamp, event_type, description, attributes_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for event in timeline {
        statement.execute(params![
            format!("event_{}", uuid::Uuid::new_v4().simple()),
            run_id,
            event.timestamp,
            event.event_type,
            event.description,
            serde_json::to_string(&event.attributes)?,
        ])?;
    }
    Ok(())
}

fn persist_findings(
    transaction: &Transaction<'_>,
    run_id: &str,
    findings: &[PluginAnalysisFinding],
) -> anyhow::Result<()> {
    let mut statement = transaction.prepare(
        "INSERT INTO plugin_findings (
            finding_id, run_id, severity, title, description, attributes_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for finding in findings {
        statement.execute(params![
            format!("finding_{}", uuid::Uuid::new_v4().simple()),
            run_id,
            severity_name(finding.severity),
            finding.title,
            finding.description,
            serde_json::to_string(&finding.attributes)?,
        ])?;
    }
    Ok(())
}

fn load_nodes(connection: &Connection, run_id: &str) -> anyhow::Result<Vec<PluginResultNode>> {
    let mut statement = connection.prepare(
        "SELECT node_type, display_name, columns_json
         FROM plugin_result_nodes WHERE run_id = ?1 ORDER BY rowid",
    )?;
    let rows = statement.query_map([run_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    rows.map(|row| {
        let (node_type, display_name, columns) = row?;
        Ok(PluginResultNode {
            node_type,
            display_name,
            columns: serde_json::from_str::<Vec<PluginResultNodeColumn>>(&columns)
                .context("decode plugin result node columns")?,
        })
    })
    .collect()
}

fn load_artifacts(
    connection: &Connection,
    run_id: &str,
) -> anyhow::Result<Vec<PluginAnalysisArtifact>> {
    let mut statement = connection.prepare(
        "SELECT artifact_type, title, value_json, attributes_json
         FROM plugin_artifacts WHERE run_id = ?1 ORDER BY rowid",
    )?;
    let rows = statement.query_map([run_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    rows.map(|row| {
        let (artifact_type, title, value, attributes) = row?;
        Ok(PluginAnalysisArtifact {
            artifact_type,
            title,
            value: value
                .map(|value| serde_json::from_str(&value))
                .transpose()
                .context("decode plugin artifact value")?,
            attributes: serde_json::from_str(&attributes)
                .context("decode plugin artifact attributes")?,
        })
    })
    .collect()
}

fn load_timeline(
    connection: &Connection,
    run_id: &str,
) -> anyhow::Result<Vec<PluginAnalysisTimelineEvent>> {
    let mut statement = connection.prepare(
        "SELECT timestamp, event_type, description, attributes_json
         FROM plugin_timeline_events WHERE run_id = ?1 ORDER BY timestamp, rowid",
    )?;
    let rows = statement.query_map([run_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    rows.map(|row| {
        let (timestamp, event_type, description, attributes) = row?;
        Ok(PluginAnalysisTimelineEvent {
            timestamp,
            event_type,
            description,
            attributes: serde_json::from_str(&attributes)
                .context("decode plugin timeline attributes")?,
        })
    })
    .collect()
}

fn load_findings(
    connection: &Connection,
    run_id: &str,
) -> anyhow::Result<Vec<PluginAnalysisFinding>> {
    let mut statement = connection.prepare(
        "SELECT severity, title, description, attributes_json
         FROM plugin_findings WHERE run_id = ?1 ORDER BY rowid",
    )?;
    let rows = statement.query_map([run_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    rows.map(|row| {
        let (severity, title, description, attributes) = row?;
        Ok(PluginAnalysisFinding {
            severity: parse_severity(&severity)?,
            title,
            description,
            attributes: serde_json::from_str(&attributes)
                .context("decode plugin finding attributes")?,
        })
    })
    .collect()
}

fn severity_name(severity: PluginFindingSeverity) -> &'static str {
    match severity {
        PluginFindingSeverity::Info => "info",
        PluginFindingSeverity::Low => "low",
        PluginFindingSeverity::Medium => "medium",
        PluginFindingSeverity::High => "high",
        PluginFindingSeverity::Critical => "critical",
    }
}

// ── 时间线投影源读取（core::timeline::timeline_project 消费）──────────────────────

/// 一条可投影的插件时间线事件(含 run 归属信息)。
#[derive(Clone, Debug)]
pub struct StoredTimelineEventSource {
    pub run_id: String,
    pub plugin_id: String,
    pub event_id: String,
    pub timestamp: String,
    pub event_type: String,
    pub description: String,
}

/// 流式遍历检材的插件时间线事件,逐行读取不整表驻留内存,供时间线重建
/// 投影消费。
pub fn visit_timeline_events(
    case_dir: &Path,
    image_id: &str,
    mut visit: impl FnMut(&StoredTimelineEventSource) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let Some(connection) = evidence_db::open_reader(case_dir, image_id, &SCHEMA)? else {
        return Ok(());
    };
    let mut statement = connection.prepare(
        "SELECT r.run_id, r.plugin_id, e.event_id, e.timestamp, e.event_type, e.description
         FROM plugin_timeline_events e
         JOIN plugin_analysis_runs r ON r.run_id = e.run_id
         ORDER BY e.rowid",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let event = StoredTimelineEventSource {
            run_id: row.get(0)?,
            plugin_id: row.get(1)?,
            event_id: row.get(2)?,
            timestamp: row.get(3)?,
            event_type: row.get(4)?,
            description: row.get(5)?,
        };
        visit(&event)?;
    }
    Ok(())
}

/// 单个 run 的事件(增量钩子:任务级级联删 + 重投影)。run 不存在时为
/// `None`。
pub fn run_events(
    case_dir: &Path,
    image_id: &str,
    run_id: &str,
) -> anyhow::Result<Option<StoredRunEvents>> {
    let Some(connection) = evidence_db::open_reader(case_dir, image_id, &SCHEMA)? else {
        return Ok(None);
    };
    let run_meta = connection
        .query_row(
            "SELECT plugin_id, completed_at FROM plugin_analysis_runs WHERE run_id = ?1",
            [run_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((plugin_id, completed_at)) = run_meta else {
        return Ok(None);
    };
    let mut statement = connection.prepare(
        "SELECT event_id, timestamp, event_type, description
         FROM plugin_timeline_events WHERE run_id = ?1 ORDER BY rowid",
    )?;
    let events = statement
        .query_map([run_id], |row| {
            Ok(StoredTimelineEventSource {
                run_id: run_id.to_string(),
                plugin_id: plugin_id.clone(),
                event_id: row.get(0)?,
                timestamp: row.get(1)?,
                event_type: row.get(2)?,
                description: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(StoredRunEvents {
        completed_at,
        events,
    }))
}

/// 一个 run 的可投影事件包。
#[derive(Clone, Debug)]
pub struct StoredRunEvents {
    /// 该 run 的完成时间(时间线事件源水位的推进值)。
    pub completed_at: String,
    pub events: Vec<StoredTimelineEventSource>,
}

/// 事件源水位:检材 `plugin_analysis_runs` 的 `max(completed_at)`(无行为
/// 空串)。时间线 staleness 判定基准。
pub fn max_completed_at(case_dir: &Path, image_id: &str) -> anyhow::Result<String> {
    let Some(connection) = evidence_db::open_reader(case_dir, image_id, &SCHEMA)? else {
        return Ok(String::new());
    };
    connection
        .query_row(
            "SELECT COALESCE(MAX(completed_at), '') FROM plugin_analysis_runs",
            [],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

/// 检材可投影事件总数(重建进度分母)。
pub fn count_timeline_events(case_dir: &Path, image_id: &str) -> anyhow::Result<u64> {
    let Some(connection) = evidence_db::open_reader(case_dir, image_id, &SCHEMA)? else {
        return Ok(0);
    };
    connection
        .query_row("SELECT COUNT(*) FROM plugin_timeline_events", [], |row| {
            row.get::<_, i64>(0).map(|count| count as u64)
        })
        .map_err(Into::into)
}

fn parse_severity(value: &str) -> anyhow::Result<PluginFindingSeverity> {
    match value {
        "info" => Ok(PluginFindingSeverity::Info),
        "low" => Ok(PluginFindingSeverity::Low),
        "medium" => Ok(PluginFindingSeverity::Medium),
        "high" => Ok(PluginFindingSeverity::High),
        "critical" => Ok(PluginFindingSeverity::Critical),
        value => anyhow::bail!("invalid stored plugin finding severity {value}"),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn structured_results_round_trip_through_case_database() {
        let directory = tempfile::tempdir().expect("temp directory");
        let result = PluginAnalysisResult {
            summary: Some("done".to_owned()),
            nodes: vec![PluginResultNode {
                node_type: "linux.user".to_owned(),
                display_name: "用户列表".to_owned(),
                columns: vec![PluginResultNodeColumn {
                    key: "linux.username".to_owned(),
                    label: "用户名".to_owned(),
                }],
            }],
            artifacts: vec![PluginAnalysisArtifact {
                artifact_type: "browser.url".to_owned(),
                title: "URL".to_owned(),
                value: Some(serde_json::json!("https://example.test")),
                attributes: HashMap::new(),
            }],
            timeline: vec![PluginAnalysisTimelineEvent {
                timestamp: "2026-08-11T00:00:00Z".to_owned(),
                event_type: "browser.visit".to_owned(),
                description: "visit".to_owned(),
                attributes: HashMap::new(),
            }],
            findings: vec![PluginAnalysisFinding {
                severity: PluginFindingSeverity::High,
                title: "Token".to_owned(),
                description: "token".to_owned(),
                attributes: HashMap::new(),
            }],
        };
        let run_id = persist(
            directory.path(),
            NewPluginAnalysis {
                plugin_id: "org.test",
                plugin_version: "1",
                analyzer: "test.analyzer",
                display_name: "测试功能",
                type_name: "baseinfo",
                image_id: "image",
                node_id: "node",
                file_name: "file",
                started_at: "2026-08-11T00:00:00Z",
                completed_at: "2026-08-11T00:00:01Z",
                result: &result,
            },
        )
        .expect("persist");
        let stored = list_for_node(directory.path(), "image", "node").expect("list");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].run_id, run_id);
        assert_eq!(stored[0].display_name, "测试功能");
        assert_eq!(stored[0].type_name, "baseinfo");
        assert_eq!(stored[0].result, result);
    }

    /// Several plugin tasks finishing at once each persist through their own
    /// connection; every run must land whole.
    #[test]
    fn concurrent_persists_each_commit_a_whole_run() {
        let directory = tempfile::tempdir().expect("temp directory");
        let result = PluginAnalysisResult {
            summary: Some("done".to_owned()),
            nodes: Vec::new(),
            artifacts: (0..500)
                .map(|index| PluginAnalysisArtifact {
                    artifact_type: "test.record".to_owned(),
                    title: format!("record {index}"),
                    value: None,
                    attributes: HashMap::new(),
                })
                .collect(),
            timeline: Vec::new(),
            findings: Vec::new(),
        };
        std::thread::scope(|scope| {
            for worker in 0..8 {
                let result = &result;
                let case_dir = directory.path();
                scope.spawn(move || {
                    let node_id = format!("node-{worker}");
                    persist(
                        case_dir,
                        NewPluginAnalysis {
                            plugin_id: "org.test",
                            plugin_version: "1",
                            analyzer: "test.analyzer",
                            display_name: "测试功能",
                            type_name: "baseinfo",
                            image_id: "image",
                            node_id: &node_id,
                            file_name: "file",
                            started_at: "2026-10-04T00:00:00Z",
                            completed_at: "2026-10-04T00:00:01Z",
                            result,
                        },
                    )
                    .expect("persist");
                });
            }
        });
        for worker in 0..8 {
            let stored =
                list_for_node(directory.path(), "image", &format!("node-{worker}")).expect("list");
            assert_eq!(stored.len(), 1);
            assert_eq!(stored[0].result, result);
        }
    }

    #[test]
    fn remove_run_preserves_later_and_other_plugin_results() {
        let directory = tempfile::tempdir().expect("temp directory");
        let result = PluginAnalysisResult {
            summary: None,
            nodes: Vec::new(),
            artifacts: vec![PluginAnalysisArtifact {
                artifact_type: "linux.user".to_owned(),
                title: "用户".to_owned(),
                value: None,
                attributes: HashMap::new(),
            }],
            timeline: Vec::new(),
            findings: Vec::new(),
        };
        let save = |plugin_id| {
            persist(
                directory.path(),
                NewPluginAnalysis {
                    plugin_id,
                    plugin_version: "1",
                    analyzer: "shared",
                    display_name: "Analysis",
                    type_name: "analysis",
                    image_id: "image",
                    node_id: "input",
                    file_name: "input.db",
                    started_at: "2026-10-18T00:00:00Z",
                    completed_at: "2026-10-18T00:00:01Z",
                    result: &result,
                },
            )
            .expect("persist")
        };
        let old = save("plugin.a");
        let later = save("plugin.a");
        let other = save("plugin.b");
        assert_eq!(
            remove_run(directory.path(), "image", &old).expect("remove"),
            vec![old]
        );
        let remaining = list_for_node(directory.path(), "image", "input").expect("list");
        let ids: std::collections::HashSet<_> =
            remaining.iter().map(|run| run.run_id.clone()).collect();
        assert_eq!(ids, [later, other].into_iter().collect());
        assert!(remaining.iter().all(|run| run.result == result));
        let connection = evidence_db::open_reader(directory.path(), "image", &SCHEMA)
            .expect("reader")
            .expect("exists");
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM plugin_artifacts", [], |row| {
                row.get(0)
            })
            .expect("count");
        assert_eq!(count, 2);
    }
}
