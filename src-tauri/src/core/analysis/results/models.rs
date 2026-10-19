//! Analysis result projections shared by storage and IPC. LPP payloads and
//! record schemas stay in the plugin SDK; these models are what the result
//! tree, the record table and the record detail show.

use serde::{Deserialize, Serialize};

/// A node of the result tree: evidence → category → analyzer run → record
/// type. Labels are the defaults the plugin declared; the UI translates
/// them with the package's locale catalog.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisTreeNode {
    /// Stable id, composed level by level.
    pub node_id: String,
    /// `evidence`, `class`, `function` or `result`.
    pub kind: &'static str,
    pub label: String,
    pub image_id: String,
    /// The category (`baseinfo`, `vehicle`, …).
    pub type_name: String,
    pub plugin_id: String,
    pub analyzer: String,
    /// `<plugin id>/<analyzer>`.
    pub function: String,
    /// The analyzed file of a file run.
    pub source_node_id: Option<String>,
    /// What the run looked at (`Kali GNU/Linux · part:2`); empty for the
    /// whole evidence.
    pub target_label: String,
    /// The record type of a result node.
    pub artifact_type: String,
    /// The published run the node belongs to (function and result nodes).
    pub run_id: String,
    pub completed_at: String,
    /// Result: records; class / function: children.
    pub child_count: u64,
    /// Warnings the run kept (function nodes).
    pub warning_count: u64,
    /// A result of an earlier LovelyCarve version (`analysis.db`), shown
    /// read-only without schema or provenance.
    pub legacy: bool,
    pub children: Vec<AnalysisTreeNode>,
}

/// A table column of a record type.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordColumn {
    pub id: String,
    pub label: String,
    /// The field kind (`text`, `timestamp`, `size`, …).
    pub kind: &'static str,
    pub key: bool,
    pub hidden: bool,
    pub timeline: Option<String>,
    pub choices: Vec<ColumnChoice>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ColumnChoice {
    pub id: String,
    pub label: String,
}

/// Where a record came from.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordSource {
    /// Volume key (`part:2`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// Path inside that volume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// File-tree node of the source file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One record of the table or the detail pane.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisRecord {
    pub record_id: String,
    pub run_id: String,
    pub record_type: String,
    pub title: String,
    /// `info` … `critical` when the analyzer rated the record.
    pub severity: Option<String>,
    /// Values by column id.
    pub values: serde_json::Map<String, serde_json::Value>,
    pub sources: Vec<RecordSource>,
    pub image_id: String,
    pub legacy: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisRecordsPage {
    pub total: u64,
    pub columns: Vec<RecordColumn>,
    pub items: Vec<AnalysisRecord>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisRecordDetail {
    pub record: AnalysisRecord,
    pub columns: Vec<RecordColumn>,
}
