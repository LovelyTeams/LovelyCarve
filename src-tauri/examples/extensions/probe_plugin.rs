//! Run a real evidence plugin read-only and save its structured result for inspection.
use anyhow::Context;
use lovelycarve_lib::core::{
    analysis::results::analysis_query::{self, AnalysisRecordsQuery},
    analysis::results::plugin_result_store::{self, NewPluginAnalysis},
    extensions::plugin::{EvidenceAnalysisRequest, FileAnalysisRequest, PluginManager},
    extensions::task_scheduler::{function_name, FunctionRegistry},
    filesystem::evidence_session::EvidenceSession,
    filesystem::fs_source_location::source_chain,
};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    anyhow::ensure!(
        arguments.len() == 4 || arguments.len() == 6,
        "usage: probe_plugin <image> <plugin-id> <analyzer> <output-json> [<root-id> <file-path>]"
    );
    let image = Arc::new(EvidenceSession::open(&arguments[0])?);
    let image_id = image.id.clone();
    let plugins = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../plugins");
    let manager = PluginManager::discover(&[plugins])?;
    let functions = FunctionRegistry::from_definitions(manager.definitions());
    let function = functions
        .get(&function_name(&arguments[1], &arguments[2]))
        .context("analyzer was not registered in the task center")?;
    let input = if arguments.len() == 6 {
        anyhow::ensure!(!function.evidence_level, "expected a file-level task");
        let chain = source_chain(&image, &arguments[4], &arguments[5])?;
        Some(chain.last().context("input file was not located")?.clone())
    } else {
        anyhow::ensure!(function.evidence_level, "expected an evidence-level task");
        None
    };
    let result = if let Some(input) = &input {
        manager
            .analyze_file(FileAnalysisRequest {
                plugin_id: arguments[1].clone(),
                analyzer: arguments[2].clone(),
                image_id: image.id.clone(),
                image,
                node_id: input.id.clone(),
                name: input.name.clone(),
            })
            .await
    } else {
        manager
            .analyze_evidence(EvidenceAnalysisRequest {
                plugin_id: arguments[1].clone(),
                analyzer: arguments[2].clone(),
                image_id: image.id.clone(),
                image,
                params: serde_json::json!({"platform": function.platform}),
            })
            .await
    };
    manager.shutdown().await;
    let result = result.context("plugin analysis failed")?;
    std::fs::write(&arguments[3], serde_json::to_vec_pretty(&result)?)?;
    let mut counts = BTreeMap::new();
    for artifact in &result.artifacts {
        *counts.entry(&artifact.artifact_type).or_insert(0) += 1;
    }
    // Exercise the same persistence and paged query used by the results UI,
    // in a temporary case rather than changing the user's open case.
    let case = tempfile::tempdir()?;
    let completed_at = chrono::Utc::now().to_rfc3339();
    let run_id = plugin_result_store::persist(
        case.path(),
        NewPluginAnalysis {
            plugin_id: &arguments[1],
            plugin_version: &function.plugin_version,
            analyzer: &arguments[2],
            display_name: &function.display_name,
            type_name: &function.type_name,
            image_id: &image_id,
            node_id: input.as_ref().map_or("", |node| node.id.as_str()),
            file_name: input.as_ref().map_or("", |node| node.name.as_str()),
            started_at: &completed_at,
            completed_at: &completed_at,
            result: &result,
        },
    )?;
    for (artifact_type, count) in &counts {
        let page = analysis_query::records_page(
            case.path(),
            &AnalysisRecordsQuery {
                image_id: &image_id,
                analyzer: &arguments[2],
                run_id: &run_id,
                artifact_type,
                keyword: None,
                skip: 0,
                limit: 100,
            },
        )?;
        anyhow::ensure!(
            page.total == *count,
            "stored record count mismatch for {artifact_type}"
        );
        anyhow::ensure!(
            page.items.len() as u64 == (*count).min(100),
            "stored page length mismatch for {artifact_type}"
        );
        for record in page.items {
            anyhow::ensure!(
                record.image_id == image_id
                    && record.source_node_id.as_deref()
                        == input.as_ref().map(|node| node.id.as_str()),
                "stored provenance mismatch"
            );
            anyhow::ensure!(
                result
                    .artifacts
                    .iter()
                    .any(|artifact| artifact.artifact_type == record.artifact_type
                        && artifact.title == record.title
                        && artifact.attributes == record.attributes),
                "stored record content mismatch"
            );
        }
    }
    println!("{}", result.summary.as_deref().unwrap_or_default());
    println!("counts={counts:?} timeline={}", result.timeline.len());
    Ok(())
}
