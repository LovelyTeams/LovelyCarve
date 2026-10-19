//! Inventory report source candidates through the read-only view plugins use.
use anyhow::Context;
use lovelycarve_lib::core::{
    extensions::plugin::HostView, filesystem::evidence_session::EvidenceSession,
};
use serde_json::json;
use std::sync::Arc;

fn main() -> anyhow::Result<()> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    anyhow::ensure!(
        arguments.len() == 3,
        "usage: probe_report_sources <image> <root> <output-json>"
    );
    let image = Arc::new(EvidenceSession::open(&arguments[0])?);
    let view = HostView::evidence("probe".to_owned(), "probe".to_owned(), image);
    let mut stack = vec![(arguments[1].clone(), 0)];
    let mut directories = 0_usize;
    let mut candidates = Vec::new();
    while let Some((path, depth)) = stack.pop() {
        anyhow::ensure!(depth <= 128, "directory depth exceeded");
        let entries = view
            .list(&path, 0, u64::MAX)
            .with_context(|| format!("list {path}"))?
            .nodes;
        for entry in entries {
            if entry.deleted {
                continue;
            }
            let child = format!("{path}/{}", entry.name);
            // Links are listed, never followed, so the walk cannot loop.
            if entry.kind == "dir" {
                directories += 1;
                anyhow::ensure!(directories <= 100_000, "directory count exceeded");
                stack.push((child, depth + 1));
                continue;
            }
            let name = entry.name.to_lowercase();
            if name.ends_with(".desktop")
                || name.ends_with(".eml")
                || [
                    ".doc", ".xls", ".ppt", ".docx", ".xlsx", ".pptx", ".odt", ".ods", ".odp",
                ]
                .iter()
                .any(|suffix| name.ends_with(suffix))
                || name.contains("sys_config")
                || name == "nginx.conf"
                || name == "apache2.conf"
                || name == "config.v2.json"
                || name == "repositories.json"
                || name == "local-kv.db"
            {
                let relative = child
                    .strip_prefix(&format!("{}/", arguments[1]))
                    .unwrap_or(&child)
                    .to_owned();
                candidates.push(json!({"path":relative,"node":entry}));
            }
        }
    }
    std::fs::write(&arguments[2], serde_json::to_vec_pretty(&candidates)?)?;
    println!(
        "{} source candidates in {directories} directories",
        candidates.len()
    );
    Ok(())
}
