//! 分析结果查询:结果树、分页表格与单条详情。
//!
//! 树层级 检材 → 分类(Type,如 baseinfo) → 功能(analyzer,如 linuxbaseinfo)
//! → 结果项(artifact_type,如 linux.user):每个 (检材, 插件, 功能, 输入文件) 只展示最新
//! run;结果项节点名与列定义全部来自插件提交(`plugin_result_nodes`),
//! 前端零硬编码。ChildCount 徽标 = 该结果项的记录数。

use std::collections::HashMap;

use rusqlite::{Connection, ToSql};
use serde::Serialize;

use crate::core::{
    analysis::results::models::{PluginResultNode, PluginResultNodeColumn},
    analysis::results::plugin_result_store,
    casework::evidence_db,
};

/// 单页记录上限,导出逐页拉取。
pub const MAX_PAGE_LIMIT: i64 = 500;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisTreeNode {
    /// 稳定节点 id:evidence/class/function/result 逐级拼接。
    pub node_id: String,
    /// evidence | class | function | result。
    pub kind: &'static str,
    /// 展示名(插件/检材提交,前端不造名)。
    pub label: String,
    pub image_id: String,
    pub type_name: String,
    pub analyzer: String,
    /// Fully qualified plugin/analyzer identity.
    pub function: String,
    pub source_node_id: Option<String>,
    pub artifact_type: String,
    /// function 级起非空:该结果项/功能归属的最新 run。
    pub run_id: String,
    pub completed_at: String,
    /// 数字徽标:result = 记录数;class/function = 下级有效节点数。
    pub child_count: u64,
    /// 仅 result 节点:表格列定义。
    pub columns: Vec<PluginResultNodeColumn>,
    pub children: Vec<AnalysisTreeNode>,
}

/// 检材侧树输入(命令层由案件证据列表生成)。
pub struct AnalysisTreeEvidence {
    pub image_id: String,
    pub label: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisRecord {
    pub artifact_id: String,
    pub run_id: String,
    pub artifact_type: String,
    pub title: String,
    pub value: Option<serde_json::Value>,
    pub attributes: HashMap<String, serde_json::Value>,
    /// Ownership and input file are host-persisted, independent of plugin attributes.
    pub image_id: String,
    pub source_node_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisRecordPage {
    pub total: u64,
    pub items: Vec<AnalysisRecord>,
}

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

impl LatestRun {
    fn function(&self) -> String {
        format!("{}/{}", self.plugin_id, self.analyzer)
    }
    fn scope_id(&self, image_id: &str) -> String {
        format!(
            "{image_id}::{}/{}::{}",
            self.plugin_id,
            self.analyzer,
            hex::encode(self.node_id.as_bytes())
        )
    }
}

/// 构建整棵分析结果树。只包含有结果 run 的检材;同一 (检材, plugin_id, analyzer, node_id)
/// 取最新 run,一个功能保留一份当前结果。逐检材读各自的 `analysis.db`。
pub fn analysis_tree(
    case_dir: &std::path::Path,
    evidence: &[AnalysisTreeEvidence],
) -> anyhow::Result<Vec<AnalysisTreeNode>> {
    let mut tree = Vec::new();
    for item in evidence {
        let Some(connection) = open_readonly(case_dir, &item.image_id)? else {
            continue;
        };
        let image_runs = latest_runs(&connection)?;
        // 分类层按 Type 分组,组内按完成时间倒序(最新分析在前)。
        let mut class_order: Vec<String> = Vec::new();
        let mut classes: HashMap<&str, Vec<&LatestRun>> = HashMap::new();
        for run in &image_runs {
            let class_key = if run.type_name.is_empty() {
                "analysis"
            } else {
                run.type_name.as_str()
            };
            if !class_order.iter().any(|key| key == class_key) {
                class_order.push(class_key.to_owned());
            }
            classes.entry(class_key).or_default().push(run);
        }
        let mut class_nodes = Vec::new();
        for class_key in class_order {
            let function_runs = &classes[class_key.as_str()];
            let mut function_nodes = Vec::new();
            for run in function_runs {
                let nodes = load_nodes(&connection, &run.run_id)?;
                let counts = artifact_counts(&connection, &run.run_id)?;
                let result_children = merge_result_nodes(&nodes, &counts, &item.image_id, run);
                if result_children.is_empty() {
                    continue;
                }
                let child_count = result_children.len() as u64;
                function_nodes.push(AnalysisTreeNode {
                    node_id: run.scope_id(&item.image_id),
                    kind: "function",
                    label: {
                        let label = if run.display_name.is_empty() {
                            &run.analyzer
                        } else {
                            &run.display_name
                        };
                        if run.node_id.is_empty() {
                            label.clone()
                        } else {
                            format!(
                                "{label} · {}",
                                if run.file_name.is_empty() {
                                    &run.node_id
                                } else {
                                    &run.file_name
                                }
                            )
                        }
                    },
                    image_id: item.image_id.clone(),
                    type_name: class_key.clone(),
                    analyzer: run.analyzer.clone(),
                    function: run.function(),
                    source_node_id: (!run.node_id.is_empty()).then(|| run.node_id.clone()),
                    artifact_type: String::new(),
                    run_id: run.run_id.clone(),
                    completed_at: run.completed_at.clone(),
                    child_count,
                    columns: Vec::new(),
                    children: result_children,
                });
            }
            if function_nodes.is_empty() {
                continue;
            }
            let class_child_count = function_nodes.len() as u64;
            class_nodes.push(AnalysisTreeNode {
                node_id: format!("{}::{}", item.image_id, class_key),
                kind: "class",
                label: class_label(&class_key),
                image_id: item.image_id.clone(),
                type_name: class_key.clone(),
                analyzer: String::new(),
                function: String::new(),
                source_node_id: None,
                artifact_type: String::new(),
                run_id: String::new(),
                completed_at: String::new(),
                child_count: class_child_count,
                columns: Vec::new(),
                children: function_nodes,
            });
        }
        if class_nodes.is_empty() {
            continue;
        }
        let evidence_child_count = class_nodes.len() as u64;
        tree.push(AnalysisTreeNode {
            node_id: item.image_id.clone(),
            kind: "evidence",
            label: item.label.clone(),
            image_id: item.image_id.clone(),
            type_name: String::new(),
            analyzer: String::new(),
            function: String::new(),
            source_node_id: None,
            artifact_type: String::new(),
            run_id: String::new(),
            completed_at: String::new(),
            child_count: evidence_child_count,
            columns: Vec::new(),
            children: class_nodes,
        });
    }
    Ok(tree)
}

/// 声明的节点在前(带列定义),有记录但未声明的类型按类型名合成节点;
/// 零记录的声明节点不展示(该子项未勾选或源数据为空)。结果节点自带
/// (检材, 功能, run) 归属,表格查询不需要回溯父节点。
fn merge_result_nodes(
    declared: &[PluginResultNode],
    counts: &HashMap<String, u64>,
    image_id: &str,
    run: &LatestRun,
) -> Vec<AnalysisTreeNode> {
    fn result_node(
        node_type: String,
        label: String,
        count: u64,
        columns: Vec<PluginResultNodeColumn>,
        image_id: &str,
        run: &LatestRun,
    ) -> AnalysisTreeNode {
        AnalysisTreeNode {
            node_id: format!("{}::{node_type}", run.scope_id(image_id)),
            kind: "result",
            label,
            image_id: image_id.to_owned(),
            type_name: String::new(),
            analyzer: run.analyzer.clone(),
            function: run.function(),
            source_node_id: (!run.node_id.is_empty()).then(|| run.node_id.clone()),
            artifact_type: node_type,
            run_id: run.run_id.clone(),
            completed_at: String::new(),
            child_count: count,
            columns,
            children: Vec::new(),
        }
    }

    let mut children = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for node in declared {
        let Some(count) = counts.get(&node.node_type) else {
            continue;
        };
        if *count == 0 {
            continue;
        }
        seen.insert(node.node_type.clone());
        children.push(result_node(
            node.node_type.clone(),
            node.display_name.clone(),
            *count,
            node.columns.clone(),
            image_id,
            run,
        ));
    }
    let mut undeclared: Vec<(&String, &u64)> = counts
        .iter()
        .filter(|(node_type, count)| **count > 0 && !seen.contains(*node_type))
        .collect();
    undeclared.sort_by(|left, right| left.0.cmp(right.0));
    for (node_type, count) in undeclared {
        children.push(result_node(
            (*node_type).clone(),
            node_type.clone(),
            *count,
            Vec::new(),
            image_id,
            run,
        ));
    }
    children
}

pub struct AnalysisRecordsQuery<'a> {
    pub image_id: &'a str,
    pub analyzer: &'a str,
    pub run_id: &'a str,
    pub artifact_type: &'a str,
    pub keyword: Option<&'a str>,
    pub skip: i64,
    pub limit: i64,
}

/// 按结果项分页读取记录,关键词过滤命中内容/标题。
/// 固定读取选中结果树节点的 run,避免分页期间混入另一份结果。
pub fn records_page(
    case_dir: &std::path::Path,
    query: &AnalysisRecordsQuery<'_>,
) -> anyhow::Result<AnalysisRecordPage> {
    let Some(connection) = open_readonly(case_dir, query.image_id)? else {
        return Ok(AnalysisRecordPage {
            total: 0,
            items: Vec::new(),
        });
    };
    let Some(run_id) = selected_run(&connection, query.run_id, query.analyzer)? else {
        return Ok(AnalysisRecordPage {
            total: 0,
            items: Vec::new(),
        });
    };
    let limit = query.limit.clamp(1, MAX_PAGE_LIMIT);
    let skip = query.skip.max(0);
    let keyword = query
        .keyword
        .map(str::trim)
        .filter(|keyword| !keyword.is_empty());
    let (filter_sql, filter_values): (&str, Vec<String>) = match keyword {
        Some(keyword) => (
            " AND (title LIKE ?3 OR attributes_json LIKE ?3)",
            vec![format!("%{}%", escape_like(keyword))],
        ),
        None => ("", Vec::new()),
    };
    let mut values: Vec<Box<dyn ToSql>> = vec![
        Box::new(run_id.clone()),
        Box::new(query.artifact_type.to_owned()),
    ];
    values.extend(filter_values.iter().cloned().map(|value| {
        let boxed: Box<dyn ToSql> = Box::new(value);
        boxed
    }));
    let total: u64 = {
        let sql = format!(
            "SELECT COUNT(*) FROM plugin_artifacts
             WHERE run_id = ?1 AND artifact_type = ?2{filter_sql}"
        );
        let params: Vec<&dyn ToSql> = values.iter().map(|value| value.as_ref()).collect();
        connection.query_row(&sql, params.as_slice(), |row| {
            row.get::<_, i64>(0).map(|count| count as u64)
        })?
    };
    let sql = format!(
        "SELECT artifact_id, run_id, artifact_type, title, value_json, attributes_json,
                (SELECT NULLIF(node_id, '') FROM plugin_analysis_runs r WHERE r.run_id = plugin_artifacts.run_id)
         FROM plugin_artifacts
         WHERE run_id = ?1 AND artifact_type = ?2{filter_sql}
         ORDER BY rowid LIMIT {limit} OFFSET {skip}"
    );
    let params: Vec<&dyn ToSql> = values.iter().map(|value| value.as_ref()).collect();
    let mut statement = connection.prepare(&sql)?;
    let items = statement
        .query_map(params.as_slice(), |row| {
            record_from_row(row, query.image_id)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(AnalysisRecordPage { total, items })
}

/// 单条记录详情:在检材的 `analysis.db` 里按 artifact_id 定位。
pub fn record_detail(
    case_dir: &std::path::Path,
    image_id: &str,
    artifact_id: &str,
) -> anyhow::Result<Option<AnalysisRecord>> {
    let Some(connection) = open_readonly(case_dir, image_id)? else {
        return Ok(None);
    };
    match connection.query_row(
        "SELECT artifact_id, run_id, artifact_type, title, value_json, attributes_json,
                (SELECT NULLIF(node_id, '') FROM plugin_analysis_runs r WHERE r.run_id = plugin_artifacts.run_id)
         FROM plugin_artifacts WHERE artifact_id = ?1",
        [artifact_id],
        |row| record_from_row(row, image_id),
    ) {
        Ok(record) => Ok(Some(record)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// `%`/`_` 不作为通配符解释。
fn escape_like(keyword: &str) -> String {
    keyword
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn record_from_row(row: &rusqlite::Row<'_>, image_id: &str) -> rusqlite::Result<AnalysisRecord> {
    let value_json = row.get::<_, Option<String>>(4)?;
    let attributes_json = row.get::<_, String>(5)?;
    let value = value_json
        .map(|value| serde_json::from_str(&value))
        .transpose()
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                4,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
    let attributes = serde_json::from_str(&attributes_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(AnalysisRecord {
        artifact_id: row.get(0)?,
        run_id: row.get(1)?,
        artifact_type: row.get(2)?,
        title: row.get(3)?,
        value,
        attributes,
        image_id: image_id.to_string(),
        source_node_id: row.get(6)?,
    })
}

/// 检材的 `analysis.db`;不存在时返回 `None`(按「无结果」处理,不建空库)。
fn open_readonly(case_dir: &std::path::Path, image_id: &str) -> anyhow::Result<Option<Connection>> {
    evidence_db::open_reader(case_dir, image_id, &plugin_result_store::SCHEMA)
}

/// 每个插件功能和输入文件各自最新的一次运行。
fn latest_runs(connection: &Connection) -> anyhow::Result<Vec<LatestRun>> {
    let mut statement = connection.prepare(
        "SELECT run_id, analyzer, display_name, type_name, completed_at, plugin_id, node_id, file_name
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
    let mut latest: Vec<LatestRun> = Vec::new();
    let mut seen: std::collections::HashSet<(String, String, String)> =
        std::collections::HashSet::new();
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

fn selected_run(
    connection: &Connection,
    run_id: &str,
    analyzer: &str,
) -> anyhow::Result<Option<String>> {
    let mut statement = connection.prepare(
        "SELECT run_id FROM plugin_analysis_runs
         WHERE run_id = ?1 AND analyzer = ?2",
    )?;
    let mut rows = statement.query([run_id, analyzer])?;
    Ok(rows.next()?.map(|row| row.get(0)).transpose()?)
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
            columns: serde_json::from_str::<Vec<PluginResultNodeColumn>>(&columns)?,
        })
    })
    .collect()
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
        let (node_type, count) = row?;
        Ok((node_type, count as u64))
    })
    .collect()
}

/// 分类层展示名:Type 裸值可读性差,映射常见类别(与快速分析对话框的
/// typeLabel 同源语义),未映射类别原样展示。
fn class_label(type_name: &str) -> String {
    match type_name {
        "baseinfo" => "基本信息".to_owned(),
        "registry" => "注册表".to_owned(),
        "eventlog" => "系统日志".to_owned(),
        "filerecover" => "文件恢复".to_owned(),
        "database" => "数据库".to_owned(),
        "application" => "应用程序".to_owned(),
        "analysis" => "分析".to_owned(),
        other => other.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::core::analysis::results::models::{
        PluginAnalysisArtifact, PluginAnalysisResult, PluginResultNode, PluginResultNodeColumn,
    };
    use crate::core::analysis::results::plugin_result_store::{persist, NewPluginAnalysis};

    fn node(node_type: &str, name: &str, columns: &[(&str, &str)]) -> PluginResultNode {
        PluginResultNode {
            node_type: node_type.to_owned(),
            display_name: name.to_owned(),
            columns: columns
                .iter()
                .map(|(key, label)| PluginResultNodeColumn {
                    key: (*key).to_owned(),
                    label: (*label).to_owned(),
                })
                .collect(),
        }
    }

    fn artifact(artifact_type: &str, title: &str, username: &str) -> PluginAnalysisArtifact {
        let mut attributes = HashMap::new();
        attributes.insert(
            "linux.username".to_owned(),
            serde_json::Value::String(username.to_owned()),
        );
        PluginAnalysisArtifact {
            artifact_type: artifact_type.to_owned(),
            title: title.to_owned(),
            value: None,
            attributes,
        }
    }

    fn store_run(
        directory: &std::path::Path,
        analyzer: &str,
        completed_at: &str,
        result: &PluginAnalysisResult,
    ) {
        persist(
            directory,
            NewPluginAnalysis {
                plugin_id: "org.lovelycarve.linuxinfo",
                plugin_version: "2.0.6",
                analyzer,
                display_name: "Linux基本信息",
                type_name: "baseinfo",
                image_id: "image-1",
                node_id: "",
                file_name: "",
                started_at: "2026-09-01T00:00:00Z",
                completed_at,
                result,
            },
        )
        .expect("persist run");
    }

    #[test]
    fn tree_groups_latest_run_and_counts_result_items() {
        let directory = tempfile::tempdir().expect("temp directory");
        let older = PluginAnalysisResult {
            summary: None,
            nodes: vec![node(
                "linux.user",
                "用户列表",
                &[("linux.username", "用户名")],
            )],
            artifacts: vec![artifact("linux.user", "用户 root", "root")],
            timeline: Vec::new(),
            findings: Vec::new(),
        };
        store_run(
            directory.path(),
            "linuxbaseinfo",
            "2026-09-01T00:00:01Z",
            &older,
        );
        let latest = PluginAnalysisResult {
            summary: None,
            nodes: vec![
                node("linux.user", "用户列表", &[("linux.username", "用户名")]),
                node("linux.usb", "USB使用记录", &[]),
            ],
            artifacts: vec![
                artifact("linux.user", "用户 root", "root"),
                artifact("linux.user", "用户 daemon", "daemon"),
                artifact("linux.unknown", "未声明类型", "x"),
            ],
            timeline: Vec::new(),
            findings: Vec::new(),
        };
        store_run(
            directory.path(),
            "linuxbaseinfo",
            "2026-09-02T00:00:01Z",
            &latest,
        );

        let evidence = [AnalysisTreeEvidence {
            image_id: "image-1".to_owned(),
            label: "kali.vmdk".to_owned(),
        }];
        let tree = analysis_tree(directory.path(), &evidence).expect("tree");
        assert_eq!(tree.len(), 1);
        let evidence_node = &tree[0];
        assert_eq!(evidence_node.kind, "evidence");
        assert_eq!(evidence_node.label, "kali.vmdk");
        let class = &evidence_node.children[0];
        assert_eq!(class.label, "基本信息");
        let function = &class.children[0];
        assert_eq!(function.kind, "function");
        assert_eq!(function.label, "Linux基本信息");
        // 声明的用户列表 + 未声明但有记录的 unknown = 2 个结果项
        // (零记录的 usb 声明被跳过)。
        assert_eq!(function.child_count, 2);
        // 声明节点按提交顺序:用户列表(2 条,带列定义)在前;零记录的
        // usb 不展示;未声明但有记录的类型合成节点。
        let user = &function.children[0];
        assert_eq!(user.kind, "result");
        assert_eq!(user.artifact_type, "linux.user");
        assert_eq!(user.child_count, 2);
        assert_eq!(user.columns[0].label, "用户名");
        let unknown = &function.children[1];
        assert_eq!(unknown.artifact_type, "linux.unknown");
        assert_eq!(unknown.label, "linux.unknown");
        assert_eq!(unknown.child_count, 1);

        // 分页 + 关键词过滤。
        let page = records_page(
            directory.path(),
            &AnalysisRecordsQuery {
                image_id: "image-1",
                analyzer: "linuxbaseinfo",
                run_id: &latest_runs(
                    &open_readonly(directory.path(), "image-1")
                        .expect("open")
                        .expect("database"),
                )
                .expect("runs")[0]
                    .run_id,
                artifact_type: "linux.user",
                keyword: Some("daemon"),
                skip: 0,
                limit: 100,
            },
        )
        .expect("page");
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].title, "用户 daemon");
        assert!(page.items[0]
            .attributes
            .get("linux.username")
            .is_some_and(serde_json::Value::is_string));
        // 详情按 id 命中。
        let artifact_id = page.items[0].artifact_id.clone();
        let detail = record_detail(directory.path(), "image-1", &artifact_id)
            .expect("detail")
            .expect("present");
        assert_eq!(detail.artifact_type, "linux.user");
    }

    #[test]
    fn record_sources_come_from_the_persisted_run_in_page_and_detail() {
        let directory = tempfile::tempdir().expect("case");
        let result = PluginAnalysisResult {
            summary: None,
            nodes: Vec::new(),
            artifacts: vec![artifact("demo.record", "input file", "")],
            timeline: Vec::new(),
            findings: Vec::new(),
        };
        store_run(
            directory.path(),
            "file-analyzer",
            "2026-10-04T00:00:00Z",
            &result,
        );
        let connection = open_readonly(directory.path(), "image-1")
            .expect("database")
            .expect("present");
        connection.execute("UPDATE plugin_analysis_runs SET node_id = 'input-node' WHERE analyzer = 'file-analyzer'", []).expect("input source");
        let page = records_page(
            directory.path(),
            &AnalysisRecordsQuery {
                image_id: "image-1",
                analyzer: "file-analyzer",
                run_id: &latest_runs(&connection).expect("runs")[0].run_id,
                artifact_type: "demo.record",
                keyword: None,
                skip: 0,
                limit: 100,
            },
        )
        .expect("page");
        let record = page.items.first().expect("record");
        assert_eq!(
            (record.image_id.as_str(), record.source_node_id.as_deref()),
            ("image-1", Some("input-node"))
        );
        let detail = record_detail(directory.path(), "image-1", &record.artifact_id)
            .expect("detail")
            .expect("record");
        assert_eq!(
            serde_json::to_value(record).expect("page DTO"),
            serde_json::to_value(detail).expect("detail DTO")
        );
    }

    #[test]
    fn tree_skips_evidence_without_runs_and_pages_clamp() {
        let directory = tempfile::tempdir().expect("temp directory");
        let evidence = [
            AnalysisTreeEvidence {
                image_id: "image-1".to_owned(),
                label: "missing.vmdk".to_owned(),
            },
            AnalysisTreeEvidence {
                image_id: "image-2".to_owned(),
                label: "no-results.vmdk".to_owned(),
            },
        ];
        let tree = analysis_tree(directory.path(), &evidence).expect("tree");
        assert!(tree.is_empty());
        // 不存在的库 → 空页而非报错。
        let page = records_page(
            directory.path(),
            &AnalysisRecordsQuery {
                image_id: "image-1",
                analyzer: "linuxbaseinfo",
                run_id: "missing",
                artifact_type: "linux.user",
                keyword: None,
                skip: 0,
                limit: 9_999,
            },
        )
        .expect("page");
        assert_eq!(page.total, 0);
        assert!(page.items.is_empty());
    }
    #[test]
    fn result_scopes_and_pages_preserve_plugin_input_and_run_identity() {
        let directory = tempfile::tempdir().expect("case");
        let result = PluginAnalysisResult {
            artifacts: vec![artifact("demo.record", "row", "")],
            ..PluginAnalysisResult::default()
        };
        let save = |plugin_id, node_id, completed_at| {
            persist(
                directory.path(),
                NewPluginAnalysis {
                    plugin_id,
                    plugin_version: "1",
                    analyzer: "shared",
                    display_name: "Shared",
                    type_name: "analysis",
                    image_id: "image",
                    node_id,
                    file_name: node_id,
                    started_at: "2026-10-18T00:00:00Z",
                    completed_at,
                    result: &result,
                },
            )
            .expect("save")
        };
        let old = save("plugin.a", "file1", "2026-10-18T00:00:01Z");
        let latest = save("plugin.a", "file1", "2026-10-18T00:00:02Z");
        let second = save("plugin.a", "file2", "2026-10-18T00:00:03Z");
        let other = save("plugin.b", "file1", "2026-10-18T00:00:04Z");
        let tree = analysis_tree(
            directory.path(),
            &[AnalysisTreeEvidence {
                image_id: "image".into(),
                label: "Evidence".into(),
            }],
        )
        .expect("tree");
        let functions = &tree[0].children[0].children;
        let ids: std::collections::HashSet<_> =
            functions.iter().map(|node| node.run_id.clone()).collect();
        assert_eq!(ids, [latest, second, other].into_iter().collect());
        assert_eq!(
            functions
                .iter()
                .map(|node| &node.node_id)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3
        );
        for node in functions {
            assert!(node.function.starts_with("plugin."));
            assert!(node.source_node_id.is_some());
        }
        let page = records_page(
            directory.path(),
            &AnalysisRecordsQuery {
                image_id: "image",
                analyzer: "shared",
                run_id: &old,
                artifact_type: "demo.record",
                keyword: None,
                skip: 0,
                limit: 100,
            },
        )
        .expect("pinned page");
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].run_id, old);
    }
}
