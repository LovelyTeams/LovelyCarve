//! Audit the provenance of every record in `probe_plugin` outputs: each
//! record's sources must resolve to a file of the image through the same
//! locator the UI uses. Inputs are local probe outputs; the evidence is only
//! opened for reading.
use anyhow::Context;
use lovelycarve_lib::core::{
    filesystem::evidence_session::EvidenceSession, filesystem::fs_source_location::source_chain,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

fn main() -> anyhow::Result<()> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    anyhow::ensure!(
        arguments.len() >= 2,
        "usage: probe_plugin_sources <image> <probe-json>..."
    );
    let image = Arc::new(EvidenceSession::open(&arguments[0])?);
    let mut sources: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut total = 0;
    for file in &arguments[1..] {
        let output: Value = serde_json::from_slice(&std::fs::read(file)?)
            .with_context(|| format!("invalid probe JSON: {file}"))?;
        let groups = output["records"]
            .as_object()
            .with_context(|| format!("missing records: {file}"))?;
        let mut records = 0;
        let mut missing = 0;
        for (record_type, items) in groups {
            for (index, record) in items.as_array().into_iter().flatten().enumerate() {
                records += 1;
                let owner = format!("{file}:{record_type}[{index}]");
                let cited: Vec<(String, String)> = record["sources"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|source| {
                        Some((
                            source["root"].as_str()?.to_owned(),
                            source["path"].as_str()?.to_owned(),
                        ))
                    })
                    .collect();
                if cited.is_empty() {
                    missing += 1;
                    failures.push(format!("no source with root and path: {owner}"));
                }
                for key in cited {
                    sources.entry(key).or_default().insert(file.clone());
                }
            }
        }
        total += records;
        println!("{file}: records={records} missing={missing}");
    }
    let mut resolved = 0;
    for ((root, path), owners) in &sources {
        match source_chain(&image, root, path) {
            Ok(chain) if !chain.is_empty() => resolved += 1,
            Ok(_) => failures.push(format!("empty navigation chain: {root} {path} {owners:?}")),
            Err(error) => failures.push(format!("{root} {path}: {error:#}; {owners:?}")),
        }
    }
    println!(
        "records={total} unique_sources={} resolved={resolved} failures={}",
        sources.len(),
        failures.len()
    );
    for failure in &failures {
        eprintln!("{failure}");
    }
    anyhow::ensure!(failures.is_empty(), "plugin provenance audit failed");
    Ok(())
}
