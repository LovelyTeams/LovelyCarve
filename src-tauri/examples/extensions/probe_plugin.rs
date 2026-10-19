//! Run one analyzer of a built plugin package on an image, through the same
//! pipeline as a task (worker process, view, declared inputs, records.db),
//! and save the published records and the run audit as JSON.
//!
//! usage: probe_plugin <image> <plugin-id> <analyzer> <output-json>
//!        [--system <system id> | --file <root id> <path>]
use anyhow::Context;
use lovelycarve_lib::core::{
    analysis::identify::systems,
    analysis::results::{record_query, record_store},
    extensions::plugin::{prepare_run, AnalyzerScope, PluginManager, RunPlan, RunTargetSpec},
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
        arguments.len() >= 4,
        "usage: probe_plugin <image> <plugin-id> <analyzer> <output-json> \
         [--system <system id> | --file <root id> <path>]"
    );
    let image = Arc::new(EvidenceSession::open(&arguments[0])?);
    let plugins = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../plugins");
    let manager = PluginManager::discover(&[plugins], None)?;
    let definition = manager.definition(&arguments[1])?;
    let analyzer = definition
        .manifest
        .analyzer(&arguments[2])
        .context("the package does not declare this analyzer")?
        .clone();
    let detected = systems::discover(&image);
    for system in &detected {
        println!("system {} ({})", system.id, system.label);
    }
    let target = match (analyzer.scope, arguments.get(4).map(String::as_str)) {
        (AnalyzerScope::System, Some("--system")) => RunTargetSpec::System {
            system_id: arguments.get(5).context("--system needs an id")?.clone(),
        },
        (AnalyzerScope::System, _) => RunTargetSpec::System {
            system_id: detected
                .iter()
                .find(|system| analyzer.applies_to_family(&system.family))
                .context("no detected system fits the analyzer")?
                .id
                .clone(),
        },
        (AnalyzerScope::Evidence, _) => RunTargetSpec::Evidence,
        (AnalyzerScope::Files, Some("--file")) => {
            let root = arguments.get(5).context("--file needs a root id")?;
            let path = arguments.get(6).context("--file needs a path")?;
            let chain = source_chain(&image, root, path)?;
            let node = chain.last().context("input file was not located")?;
            RunTargetSpec::File {
                node_id: node.id.clone(),
                name: node.name.clone(),
            }
        }
        (AnalyzerScope::Files, _) => anyhow::bail!("a files analyzer needs --file <root> <path>"),
    };

    // A temporary case, so the user's open case is never touched.
    let case = tempfile::tempdir()?;
    let worker = manager.start_worker(&arguments[1]).await?;
    let run_id = format!("run_{}", uuid::Uuid::new_v4().simple());
    let label = image
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let prepared = prepare_run(
        &RunPlan {
            case_dir: case.path(),
            image: &image,
            definition: &definition,
            analyzer: &analyzer,
            target: &target,
            options: &serde_json::Map::new(),
            systems: &detected,
            hello: worker.hello(),
            evidence_label: &label,
        },
        &run_id,
        Arc::new(|current, total| tracing::info!("progress {current}/{total:?}")),
        &tokio_util::sync::CancellationToken::new(),
    )?;
    let started = std::time::Instant::now();
    let finish = worker
        .run(Arc::clone(&prepared.context), &prepared.start)
        .await;
    if let Err(error) = worker.shutdown().await {
        tracing::debug!("worker shutdown: {error}");
    }
    let finish = finish.context("the run failed")?;
    let elapsed = started.elapsed();
    record_store::publish(
        case.path(),
        &image.id,
        &run_id,
        &record_store::RunCompletion {
            summary: finish.summary.as_deref(),
            finished_at: &chrono::Utc::now().to_rfc3339(),
            read_faults: &prepared.context.read_faults(),
        },
    )?
    .context("the run was not published")?;
    manager.shutdown().await;

    let connection = record_query::open(case.path(), &image.id)?.context("records.db")?;
    let mut records = BTreeMap::new();
    let mut counts = BTreeMap::new();
    for record_type in record_query::run_record_types(&connection, &run_id)? {
        let mut items = Vec::new();
        let mut skip = 0;
        loop {
            let page = record_query::records_page(
                &connection,
                &record_query::RecordsQuery {
                    run_id: &run_id,
                    record_type: &record_type.schema.record_type,
                    keyword: None,
                    sort: None,
                    descending: false,
                    skip,
                    limit: record_query::MAX_PAGE_LIMIT,
                },
            )?;
            let fetched = page.items.len() as i64;
            items.extend(page.items);
            skip += fetched;
            if fetched == 0 || skip as u64 >= page.total {
                break;
            }
        }
        counts.insert(record_type.schema.record_type.clone(), items.len());
        records.insert(record_type.schema.record_type.clone(), items);
    }
    let audit = record_query::run_audit(&connection, &run_id)?;
    std::fs::write(
        &arguments[3],
        serde_json::to_vec_pretty(&serde_json::json!({
            "imageId": image.id,
            "run": audit,
            "records": records,
        }))?,
    )?;
    println!("{}", finish.summary.as_deref().unwrap_or_default());
    println!("counts={counts:?} elapsed={elapsed:?}");
    Ok(())
}
