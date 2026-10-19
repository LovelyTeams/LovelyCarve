//! 分析结果查询:结果树、分页记录、单条详情与运行审计。
//!
//! 树层级:检材 → 分类 → 分析器运行(每个作业一份已发布运行;旧版结果每个
//! (插件, 功能, 输入文件) 的最新运行)→ 记录类型。新版运行来自
//! `records.db`([`record_query`]),旧版结果来自 `analysis.db`
//! ([`legacy_query`],只读)。节点名与列定义都来自插件声明。

use std::path::Path;

use crate::core::analysis::results::{
    legacy_query::{self, LegacyRecordsQuery},
    models::{AnalysisRecord, AnalysisRecordDetail, AnalysisRecordsPage, AnalysisTreeNode},
    record_query::{self, PublishedRun, RecordsQuery, RunAudit, StoredRecord},
};

/// An evidence item of the case, as the tree shows it.
pub struct AnalysisTreeEvidence {
    pub image_id: String,
    pub label: String,
}

/// The whole result tree. Evidence without results is left out.
pub fn analysis_tree(
    case_dir: &Path,
    evidence: &[AnalysisTreeEvidence],
) -> anyhow::Result<Vec<AnalysisTreeNode>> {
    let mut tree = Vec::new();
    for item in evidence {
        let mut functions: Vec<(String, AnalysisTreeNode)> = Vec::new();
        if let Some(connection) = record_query::open(case_dir, &item.image_id)? {
            for run in record_query::published_runs(&connection)? {
                let types = record_query::run_record_types(&connection, &run.run_id)?;
                if types.is_empty() && run.warning_count == 0 {
                    continue;
                }
                let children = types
                    .iter()
                    .map(|record_type| {
                        result_node(
                            &item.image_id,
                            &run,
                            &record_type.schema.record_type,
                            &record_type.schema.label,
                            record_type.record_count,
                        )
                    })
                    .collect::<Vec<_>>();
                functions.push((
                    run.category.clone(),
                    function_node(&item.image_id, &run, children),
                ));
            }
        }
        functions.extend(legacy_query::function_nodes(case_dir, &item.image_id)?);
        if functions.is_empty() {
            continue;
        }
        let mut classes: Vec<AnalysisTreeNode> = Vec::new();
        for (category, function) in functions {
            let position = match classes.iter().position(|class| class.type_name == category) {
                Some(position) => position,
                None => {
                    classes.push(class_node(&item.image_id, &category));
                    classes.len() - 1
                }
            };
            classes[position].children.push(function);
        }
        for class in &mut classes {
            class.child_count = class.children.len() as u64;
        }
        tree.push(AnalysisTreeNode {
            node_id: item.image_id.clone(),
            kind: "evidence",
            label: item.label.clone(),
            image_id: item.image_id.clone(),
            type_name: String::new(),
            plugin_id: String::new(),
            analyzer: String::new(),
            function: String::new(),
            source_node_id: None,
            target_label: String::new(),
            artifact_type: String::new(),
            run_id: String::new(),
            completed_at: String::new(),
            child_count: classes.len() as u64,
            warning_count: 0,
            legacy: false,
            children: classes,
        });
    }
    Ok(tree)
}

fn class_node(image_id: &str, category: &str) -> AnalysisTreeNode {
    AnalysisTreeNode {
        node_id: format!("{image_id}::{category}"),
        kind: "class",
        label: category.to_owned(),
        image_id: image_id.to_owned(),
        type_name: category.to_owned(),
        plugin_id: String::new(),
        analyzer: String::new(),
        function: String::new(),
        source_node_id: None,
        target_label: String::new(),
        artifact_type: String::new(),
        run_id: String::new(),
        completed_at: String::new(),
        child_count: 0,
        warning_count: 0,
        legacy: false,
        children: Vec::new(),
    }
}

fn function_node(
    image_id: &str,
    run: &PublishedRun,
    children: Vec<AnalysisTreeNode>,
) -> AnalysisTreeNode {
    AnalysisTreeNode {
        node_id: format!("{image_id}::{}", run.job_key),
        kind: "function",
        label: run.analyzer_name.clone(),
        image_id: image_id.to_owned(),
        type_name: run.category.clone(),
        plugin_id: run.plugin_id.clone(),
        analyzer: run.analyzer.clone(),
        function: format!("{}/{}", run.plugin_id, run.analyzer),
        source_node_id: (run.target.kind == "files" && !run.target.id.is_empty())
            .then(|| run.target.id.clone()),
        target_label: run.target.label.clone(),
        artifact_type: String::new(),
        run_id: run.run_id.clone(),
        completed_at: run.published_at.clone(),
        child_count: children.len() as u64,
        warning_count: run.warning_count,
        legacy: false,
        children,
    }
}

fn result_node(
    image_id: &str,
    run: &PublishedRun,
    record_type: &str,
    label: &str,
    count: u64,
) -> AnalysisTreeNode {
    AnalysisTreeNode {
        node_id: format!("{image_id}::{}::{record_type}", run.job_key),
        kind: "result",
        label: label.to_owned(),
        image_id: image_id.to_owned(),
        type_name: run.category.clone(),
        plugin_id: run.plugin_id.clone(),
        analyzer: run.analyzer.clone(),
        function: format!("{}/{}", run.plugin_id, run.analyzer),
        source_node_id: (run.target.kind == "files" && !run.target.id.is_empty())
            .then(|| run.target.id.clone()),
        target_label: run.target.label.clone(),
        artifact_type: record_type.to_owned(),
        run_id: run.run_id.clone(),
        completed_at: String::new(),
        child_count: count,
        warning_count: 0,
        legacy: false,
        children: Vec::new(),
    }
}

/// A page of one record type of one run (the run the tree node pins, so
/// paging never mixes two publications).
pub struct AnalysisRecordsQuery<'a> {
    pub image_id: &'a str,
    pub run_id: &'a str,
    pub record_type: &'a str,
    pub legacy: bool,
    pub keyword: Option<&'a str>,
    /// A column id, or `title`.
    pub sort: Option<&'a str>,
    pub descending: bool,
    pub skip: i64,
    pub limit: i64,
}

pub fn records_page(
    case_dir: &Path,
    query: &AnalysisRecordsQuery<'_>,
) -> anyhow::Result<AnalysisRecordsPage> {
    if query.legacy {
        return legacy_query::records_page(
            case_dir,
            &LegacyRecordsQuery {
                image_id: query.image_id,
                run_id: query.run_id,
                record_type: query.record_type,
                keyword: query.keyword,
                sort: query.sort,
                descending: query.descending,
                skip: query.skip,
                limit: query.limit,
            },
        );
    }
    let Some(connection) = record_query::open(case_dir, query.image_id)? else {
        return Ok(AnalysisRecordsPage {
            total: 0,
            columns: Vec::new(),
            properties: None,
            items: Vec::new(),
        });
    };
    let page = record_query::records_page(
        &connection,
        &RecordsQuery {
            run_id: query.run_id,
            record_type: query.record_type,
            keyword: query.keyword,
            sort: query.sort,
            descending: query.descending,
            skip: query.skip,
            limit: query.limit,
        },
    )?;
    Ok(AnalysisRecordsPage {
        total: page.total,
        columns: page.columns,
        properties: page.properties,
        items: page
            .items
            .into_iter()
            .map(|record| analysis_record(record, query.image_id))
            .collect(),
    })
}

/// One record by id: a number is a `records.db` record, anything else a
/// legacy artifact id.
pub fn record_detail(
    case_dir: &Path,
    image_id: &str,
    record_id: &str,
) -> anyhow::Result<Option<AnalysisRecordDetail>> {
    if let Ok(record_id) = record_id.parse::<i64>() {
        let Some(connection) = record_query::open(case_dir, image_id)? else {
            return Ok(None);
        };
        return Ok(record_query::record_detail(&connection, record_id)?.map(
            |(record, columns)| AnalysisRecordDetail {
                record: analysis_record(record, image_id),
                columns,
            },
        ));
    }
    Ok(legacy_query::record_detail(case_dir, image_id, record_id)?
        .map(|(record, columns)| AnalysisRecordDetail { record, columns }))
}

/// The audit of one run (inputs, options, warnings, read faults).
pub fn run_audit(
    case_dir: &Path,
    image_id: &str,
    run_id: &str,
) -> anyhow::Result<Option<RunAudit>> {
    let Some(connection) = record_query::open(case_dir, image_id)? else {
        return Ok(None);
    };
    record_query::run_audit(&connection, run_id)
}

fn analysis_record(record: StoredRecord, image_id: &str) -> AnalysisRecord {
    AnalysisRecord {
        record_id: record.record_id,
        run_id: record.run_id,
        record_type: record.record_type,
        title: record.title,
        severity: record.severity,
        values: record.values,
        sources: record.sources,
        image_id: image_id.to_owned(),
        legacy: false,
    }
}
