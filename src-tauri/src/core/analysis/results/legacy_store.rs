//! 旧版(LPP v1)插件结果 `analysis.db`:只读展示,不再写入。
//!
//! 旧案件里的运行、产物、时间线事件与结果节点保持原样;结果树和时间线
//! 重建照常读取它们([`crate::core::analysis::results::legacy_query`]、
//! [`visit_timeline_events`])。删除旧任务时仍删除它的运行([`remove_run`])。
//! 新的运行写入 `records.db`([`crate::core::analysis::results::record_store`])。

use std::path::Path;
use std::time::Duration;

use rusqlite::params;

use crate::core::casework::{
    case_paths::EvidenceDb,
    evidence_db::{self, StoreSchema},
};

/// `analysis.db` 结构(旧版,不迁移)。
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

/// 删除一次旧版运行,级联清空其产物 / 节点 / 事件 / 线索。返回是否存在。
pub fn remove_run(case_dir: &Path, image_id: &str, run_id: &str) -> anyhow::Result<bool> {
    let Some(connection) =
        evidence_db::open_existing(case_dir, image_id, &SCHEMA, SCHEMA.busy_timeout)?
    else {
        return Ok(false);
    };
    let removed = connection.execute(
        "DELETE FROM plugin_analysis_runs WHERE run_id = ?1",
        params![run_id],
    )?;
    Ok(removed > 0)
}

// ── 时间线投影源读取(core::timeline::timeline_project 消费)──────────────────────

/// 一条可投影的旧版插件时间线事件(含 run 归属信息)。
#[derive(Clone, Debug)]
pub struct StoredTimelineEventSource {
    pub run_id: String,
    pub plugin_id: String,
    pub event_id: String,
    pub timestamp: String,
    pub event_type: String,
    pub description: String,
}

/// 流式遍历检材的旧版插件时间线事件,逐行读取不整表驻留内存。
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

/// 旧版事件源水位:`max(completed_at)`(无行为空串)。
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

/// 旧版可投影事件总数(重建进度分母)。
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

/// Writes legacy runs for tests that read old cases; production never writes
/// `analysis.db`.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub struct LegacyArtifact<'a> {
        pub artifact_type: &'a str,
        pub title: &'a str,
        pub attributes: serde_json::Value,
    }

    pub struct LegacyRun<'a> {
        pub plugin_id: &'a str,
        pub analyzer: &'a str,
        pub display_name: &'a str,
        pub type_name: &'a str,
        pub node_id: &'a str,
        pub file_name: &'a str,
        pub completed_at: &'a str,
        /// `(node type, display name, [(column key, label)])`.
        pub nodes: Vec<(&'a str, &'a str, Vec<(&'a str, &'a str)>)>,
        pub artifacts: Vec<LegacyArtifact<'a>>,
        /// `(timestamp, event type, description)`.
        pub events: Vec<(&'a str, &'a str, &'a str)>,
    }

    pub fn persist(case_dir: &Path, image_id: &str, run: &LegacyRun<'_>) -> String {
        let connection =
            evidence_db::open_writer(case_dir, image_id, &SCHEMA).expect("legacy store");
        let run_id = format!("run_{}", uuid::Uuid::new_v4().simple());
        connection
            .execute(
                "INSERT INTO plugin_analysis_runs (
                    run_id, plugin_id, plugin_version, analyzer, display_name, type_name,
                    node_id, file_name, summary, started_at, completed_at
                ) VALUES (?1, ?2, '1', ?3, ?4, ?5, ?6, ?7, NULL, ?8, ?8)",
                params![
                    run_id,
                    run.plugin_id,
                    run.analyzer,
                    run.display_name,
                    run.type_name,
                    run.node_id,
                    run.file_name,
                    run.completed_at
                ],
            )
            .expect("legacy run");
        for (node_type, display_name, columns) in &run.nodes {
            let columns: Vec<serde_json::Value> = columns
                .iter()
                .map(|(key, label)| serde_json::json!({ "key": key, "label": label }))
                .collect();
            connection
                .execute(
                    "INSERT INTO plugin_result_nodes (run_id, node_type, display_name, columns_json)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        run_id,
                        node_type,
                        display_name,
                        serde_json::Value::Array(columns).to_string()
                    ],
                )
                .expect("legacy node");
        }
        for artifact in &run.artifacts {
            connection
                .execute(
                    "INSERT INTO plugin_artifacts (
                        artifact_id, run_id, artifact_type, title, value_json, attributes_json
                    ) VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
                    params![
                        format!("artifact_{}", uuid::Uuid::new_v4().simple()),
                        run_id,
                        artifact.artifact_type,
                        artifact.title,
                        artifact.attributes.to_string()
                    ],
                )
                .expect("legacy artifact");
        }
        for (timestamp, event_type, description) in &run.events {
            connection
                .execute(
                    "INSERT INTO plugin_timeline_events (
                        event_id, run_id, timestamp, event_type, description, attributes_json
                    ) VALUES (?1, ?2, ?3, ?4, ?5, '{}')",
                    params![
                        format!("event_{}", uuid::Uuid::new_v4().simple()),
                        run_id,
                        timestamp,
                        event_type,
                        description
                    ],
                )
                .expect("legacy event");
        }
        run_id
    }
}
