//! Validated analysis projections used by storage and IPC.
//! LPP/ABI types remain in the plugin SDK; these models hold JSON/UI values.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginAnalysisResult {
    pub summary: Option<String>,
    /// 插件提交的结果节点:
    /// 每个节点是分析结果树上的一个数据项(linux.user → 用户列表),
    /// 列定义随节点下发,前端零硬编码。
    pub nodes: Vec<PluginResultNode>,
    pub artifacts: Vec<PluginAnalysisArtifact>,
    pub timeline: Vec<PluginAnalysisTimelineEvent>,
    pub findings: Vec<PluginAnalysisFinding>,
}

/// 一个结果节点:数据项的类型名、显示名与表格列定义。
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginResultNode {
    pub node_type: String,
    pub display_name: String,
    pub columns: Vec<PluginResultNodeColumn>,
}

/// 一列 = 记录 attributes 里的属性键 + 展示列名。
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginResultNodeColumn {
    pub key: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginAnalysisArtifact {
    pub artifact_type: String,
    pub title: String,
    pub value: Option<serde_json::Value>,
    pub attributes: HashMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginAnalysisTimelineEvent {
    pub timestamp: String,
    pub event_type: String,
    pub description: String,
    pub attributes: HashMap<String, serde_json::Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginFindingSeverity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginAnalysisFinding {
    pub severity: PluginFindingSeverity,
    pub title: String,
    pub description: String,
    pub attributes: HashMap<String, serde_json::Value>,
}
