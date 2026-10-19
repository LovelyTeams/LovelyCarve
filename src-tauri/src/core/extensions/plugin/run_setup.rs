//! Preparing one analyzer run: the view it looks at, the declared inputs it
//! is given, the options and profile facts it starts with, its staging run
//! in the evidence's `records.db` and the host context that serves the
//! plugin's calls. The task runner (and the probe example) feed the result
//! to [`super::TaskWorker::run`].

use std::{collections::BTreeMap, path::Path, sync::Arc};

use anyhow::Context as _;
use lovelycarve_plugin::{
    protocol::{Hello, RunStart, Target},
    Value,
};
use tokio_util::sync::CancellationToken;

use super::{AnalyzerManifest, FindScan, HostView, PluginDefinition, ProgressSink, RunContext};
use crate::core::{
    analysis::{
        identify::systems::DetectedSystem,
        results::{
            record_query,
            record_store::{self, NewRun, RecordWriter, RunInput, RunTarget},
        },
    },
    filesystem::{
        evidence_session::EvidenceSession,
        tree::fs_tree::{self, FsNode, Mactb, NodeKind},
    },
};

/// Most inputs one run is given.
const MAX_INPUTS: usize = 100_000;

/// What the run looks at; it must match the analyzer's scope.
#[derive(Clone, Debug, PartialEq)]
pub enum RunTargetSpec {
    /// One detected system (see [`crate::core::analysis::identify::systems`]).
    System { system_id: String },
    /// Every volume of the evidence.
    Evidence,
    /// One file of the evidence.
    File { node_id: String, name: String },
}

impl RunTargetSpec {
    /// The job's target key: the system id, the node id, or empty.
    pub fn key(&self) -> &str {
        match self {
            Self::System { system_id } => system_id,
            Self::Evidence => "",
            Self::File { node_id, .. } => node_id,
        }
    }
}

/// Everything a run is prepared from.
pub struct RunPlan<'a> {
    pub case_dir: &'a Path,
    pub image: &'a Arc<EvidenceSession>,
    pub definition: &'a PluginDefinition,
    pub analyzer: &'a AnalyzerManifest,
    pub target: &'a RunTargetSpec,
    /// Options the user chose; missing ones take their defaults.
    pub options: &'a serde_json::Map<String, serde_json::Value>,
    /// Systems of the evidence (system targets).
    pub systems: &'a [DetectedSystem],
    /// The worker library's verified `plugin.hello`.
    pub hello: &'a Hello,
    /// The evidence's display name.
    pub evidence_label: &'a str,
}

/// A prepared run: give `start` to the worker with `context` attached.
pub struct PreparedRun {
    pub context: Arc<RunContext>,
    pub start: RunStart,
}

/// An input found before the run's context exists.
struct FoundInput {
    view_path: Option<String>,
    node: FsNode,
    root: Option<String>,
    root_path: Option<String>,
}

/// Locates the run's target and inputs, writes its staging run (`run_id`)
/// and builds its context. On an error after the staging run was written,
/// the run is discarded again.
pub fn prepare_run(
    plan: &RunPlan<'_>,
    run_id: &str,
    progress: ProgressSink,
    cancel: &CancellationToken,
) -> anyhow::Result<PreparedRun> {
    let image_id = plan.image.id.clone();
    let view_id = format!("{run_id}.view");
    let (view, target, run_target, patterns) = match plan.target {
        RunTargetSpec::System { system_id } => {
            let system = plan
                .systems
                .iter()
                .find(|system| system.id == *system_id)
                .with_context(|| format!("系统 {system_id} 在检材上已识别不到"))?;
            let view = Arc::new(HostView::system(view_id, system, Arc::clone(plan.image)));
            let target = Target::System {
                view: view.info().clone(),
            };
            let run_target = RunTarget {
                kind: "system".to_owned(),
                id: system.id.clone(),
                label: system.label.clone(),
            };
            (Some(view), target, run_target, plan.analyzer.inputs.clone())
        }
        RunTargetSpec::Evidence => {
            let view = Arc::new(HostView::evidence(
                view_id,
                plan.evidence_label.to_owned(),
                Arc::clone(plan.image),
            ));
            let target = Target::Evidence {
                view: view.info().clone(),
            };
            let run_target = RunTarget {
                kind: "evidence".to_owned(),
                id: String::new(),
                label: plan.evidence_label.to_owned(),
            };
            // Evidence inputs apply under every volume.
            let patterns = plan
                .analyzer
                .inputs
                .iter()
                .map(|pattern| format!("*/{pattern}"))
                .collect();
            (Some(view), target, run_target, patterns)
        }
        RunTargetSpec::File { node_id, name } => {
            let run_target = RunTarget {
                kind: "files".to_owned(),
                id: node_id.clone(),
                label: name.clone(),
            };
            (None, Target::Files, run_target, Vec::new())
        }
    };

    let inputs = match &view {
        Some(view) if !patterns.is_empty() => find_inputs(view, &patterns, cancel)?,
        Some(_) => Vec::new(),
        None => {
            let stat = fs_tree::file_stat(plan.image, &run_target.id)
                .with_context(|| format!("无法读取目标文件 {}", run_target.label))?;
            vec![FoundInput {
                view_path: None,
                node: FsNode {
                    id: run_target.id.clone(),
                    name: run_target.label.clone(),
                    parent_id: None,
                    kind: NodeKind::File,
                    size: stat.size,
                    deleted: false,
                    inode: stat.inode,
                    timestamps: Mactb::default(),
                    link_target: None,
                    has_snapshot: false,
                },
                root: None,
                root_path: Some(run_target.label.clone()),
            }]
        }
    };
    if cancel.is_cancelled() {
        anyhow::bail!("任务已取消");
    }

    let mut options = serde_json::Map::new();
    for option in &plan.analyzer.options {
        let value = plan
            .options
            .get(&option.id)
            .cloned()
            .unwrap_or_else(|| option.default_json());
        options.insert(option.id.clone(), value);
    }
    let facts = match record_query::open(plan.case_dir, &image_id)? {
        Some(connection) => record_query::profile_facts(&connection, &run_target.id)?,
        None => BTreeMap::new(),
    };
    let run_inputs: Vec<RunInput> = inputs
        .iter()
        .map(|input| RunInput {
            path: input
                .view_path
                .clone()
                .unwrap_or_else(|| input.node.name.clone()),
            root: input.root.clone(),
            root_path: input.root_path.clone(),
            node_id: Some(input.node.id.clone()),
            size: input.node.size,
        })
        .collect();
    let manifest = &plan.definition.manifest;
    record_store::begin_run(
        plan.case_dir,
        &image_id,
        &NewRun {
            run_id,
            job_key: &format!("{}/{}#{}", manifest.id, plan.analyzer.id, plan.target.key()),
            plugin_id: &manifest.id,
            plugin_version: &manifest.version,
            analyzer: &plan.analyzer.id,
            analyzer_name: &plan.analyzer.name,
            category: &plan.analyzer.category,
            target: &run_target,
            options: &serde_json::Value::Object(options.clone()),
            profile: &serde_json::Value::Object(facts.clone().into_iter().collect()),
            inputs: &run_inputs,
            schema_hash: &plan.hello.schema_hash,
            started_at: &chrono::Utc::now().to_rfc3339(),
        },
    )?;
    let mut staging = StagingGuard {
        case_dir: plan.case_dir,
        image_id: &image_id,
        run_id,
        armed: true,
    };
    let writer = RecordWriter::open(plan.case_dir, &image_id, run_id, &plan.hello.records)?;
    let context = Arc::new(RunContext::new(
        run_id.to_owned(),
        Arc::clone(plan.image),
        view,
        manifest.permissions.iter().copied(),
        writer,
        progress,
    ));
    let mut files = Vec::with_capacity(inputs.len());
    for input in inputs {
        let path = input
            .view_path
            .clone()
            .unwrap_or_else(|| input.node.name.clone());
        match context.register_node(&input.node, input.view_path, input.root, input.root_path) {
            Ok(file) => files.push(file),
            // An input that cannot be opened is reported, not fatal.
            Err(error) => {
                context.host_warning("INPUT_UNAVAILABLE", &error.to_string(), Some(&path))
            }
        }
    }
    let start = RunStart {
        run_id: run_id.to_owned(),
        analyzer: plan.analyzer.id.clone(),
        target,
        inputs: files,
        options: options
            .iter()
            .map(|(key, value)| (key.clone(), json_to_value(value)))
            .collect(),
        profile: facts
            .iter()
            .map(|(key, value)| (key.clone(), json_to_value(value)))
            .collect(),
        locale: "zh-CN".to_owned(),
    };
    staging.armed = false;
    Ok(PreparedRun { context, start })
}

/// The files of `view` matching `patterns`, in path order.
fn find_inputs(
    view: &HostView,
    patterns: &[String],
    cancel: &CancellationToken,
) -> anyhow::Result<Vec<FoundInput>> {
    let mut scan = FindScan::new(patterns)?;
    let mut paths = Vec::new();
    while !scan.is_complete() {
        if cancel.is_cancelled() {
            anyhow::bail!("任务已取消");
        }
        paths.extend(view.find_page(&mut scan)?);
        if paths.len() > MAX_INPUTS {
            anyhow::bail!("声明的输入匹配了超过 {MAX_INPUTS} 个文件");
        }
    }
    paths.sort();
    let mut inputs = Vec::with_capacity(paths.len());
    for path in paths {
        if let Some(file) = view.locate_file(&path)? {
            inputs.push(FoundInput {
                view_path: Some(file.view_path),
                node: file.node,
                root: Some(file.root),
                root_path: Some(file.root_path),
            });
        }
    }
    Ok(inputs)
}

/// Discards the staging run unless preparation finished.
struct StagingGuard<'a> {
    case_dir: &'a Path,
    image_id: &'a str,
    run_id: &'a str,
    armed: bool,
}

impl Drop for StagingGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            if let Err(error) = record_store::discard(self.case_dir, self.image_id, self.run_id) {
                tracing::warn!(run_id = %self.run_id, "staging run not discarded: {error:#}");
            }
        }
    }
}

/// A JSON value as MessagePack.
fn json_to_value(value: &serde_json::Value) -> Value {
    match value {
        serde_json::Value::Null => Value::Nil,
        serde_json::Value::Bool(flag) => Value::Boolean(*flag),
        serde_json::Value::Number(number) => {
            if let Some(unsigned) = number.as_u64() {
                Value::from(unsigned)
            } else if let Some(signed) = number.as_i64() {
                Value::from(signed)
            } else {
                Value::from(number.as_f64().unwrap_or_default())
            }
        }
        serde_json::Value::String(text) => Value::from(text.as_str()),
        serde_json::Value::Array(items) => Value::Array(items.iter().map(json_to_value).collect()),
        serde_json::Value::Object(fields) => Value::Map(
            fields
                .iter()
                .map(|(key, item)| (Value::from(key.as_str()), json_to_value(item)))
                .collect(),
        ),
    }
}
