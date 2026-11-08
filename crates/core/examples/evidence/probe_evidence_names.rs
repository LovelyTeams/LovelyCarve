//! Read-only native filename inventory for independent plugin corpus verification.
use lovelycarve_core::{
    filesystem::evidence_session::EvidenceSession,
    filesystem::tree::fs_tree::{FsSession, NodeKind},
};
use std::{collections::HashSet, sync::Arc};

fn main() -> anyhow::Result<()> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    anyhow::ensure!(
        arguments.len() >= 3,
        "usage: probe_evidence_names <image> <root-id> <suffix>..."
    );
    let image = Arc::new(EvidenceSession::open(&arguments[0])?);
    let mut session = FsSession::new(&image);
    let mut stack = vec![(arguments[1].clone(), String::new(), 0usize)];
    let mut directories = HashSet::new();
    let mut files = Vec::new();
    let suffixes: Vec<_> = arguments[2..]
        .iter()
        .map(|value| value.to_ascii_lowercase())
        .collect();
    let mut visited = 0usize;
    while let Some((directory, parent, depth)) = stack.pop() {
        anyhow::ensure!(
            depth <= 128 && directories.insert(directory.clone()),
            "depth limit / repeated directory at {parent}"
        );
        for node in session.children(&directory)? {
            visited += 1;
            anyhow::ensure!(visited <= 4_000_000, "inventory exceeds 4000000 entries");
            if node.deleted || node.link_target.is_some() {
                continue;
            }
            let path = if parent.is_empty() {
                node.name.clone()
            } else {
                format!("{parent}/{}", node.name)
            };
            anyhow::ensure!(path.len() <= 4096, "path length limit at {path}");
            if node.kind == NodeKind::Dir {
                stack.push((node.id.clone(), path, depth + 1));
            } else if node.kind == NodeKind::File
                && suffixes
                    .iter()
                    .any(|suffix| node.name.to_ascii_lowercase().ends_with(suffix))
            {
                anyhow::ensure!(files.len() < 10000, "inventory exceeds 10000 matched files");
                files.push(serde_json::json!({"path": path, "node": node}));
            }
        }
    }
    println!(
        "{}",
        serde_json::json!({"visited": visited, "files": files})
    );
    Ok(())
}
