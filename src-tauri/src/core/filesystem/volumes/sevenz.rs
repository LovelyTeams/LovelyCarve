//! Read-only 7z-source adapter for the FS tree.
//!
//! Node IDs encode archive-relative paths as `sevenz:{hex(path)}`. The empty
//! form `sevenz:` is the synthetic archive root.

use std::sync::Arc;

use anyhow::{anyhow, bail, Result};

use crate::core::evidence::image::DiskImage;
use crate::core::evidence::readers::sevenz_image::{SevenzEntry, SevenzEntryKind};
use crate::core::filesystem::tree::fs_tree::{
    detect_mime_bytes, shannon_entropy, FsDataRun, FsNode, FsStat, Mactb, NodeKind,
};

const SAMPLE_BYTES: usize = 4096;

pub fn parse_sevenz_id(id: &str) -> Option<String> {
    let rest = id.strip_prefix("sevenz:")?;
    if rest.is_empty() {
        return Some(String::new());
    }
    let bytes = hex::decode(rest).ok()?;
    String::from_utf8(bytes).ok()
}

fn sevenz_id_for(rel: &str) -> String {
    let trimmed = rel.trim_matches('/');
    if trimmed.is_empty() {
        "sevenz:".to_string()
    } else {
        format!("sevenz:{}", hex::encode(trimmed))
    }
}

fn parent_id_for(rel: &str) -> Option<String> {
    let trimmed = rel.trim_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.rsplit_once('/') {
        Some((parent, _)) if !parent.is_empty() => Some(sevenz_id_for(parent)),
        _ => Some("sevenz:".to_string()),
    }
}

fn name_of(rel: &str) -> String {
    let trimmed = rel.trim_matches('/');
    if trimmed.is_empty() {
        return "7z:root".to_string();
    }
    trimmed
        .rsplit('/')
        .next()
        .map(str::to_string)
        .unwrap_or_else(|| trimmed.to_string())
}

fn node_kind_of(kind: SevenzEntryKind) -> NodeKind {
    match kind {
        SevenzEntryKind::Dir => NodeKind::Dir,
        SevenzEntryKind::File => NodeKind::File,
        SevenzEntryKind::Other => NodeKind::Unallocated,
    }
}

fn ts(value: Option<i64>) -> Option<String> {
    value
        .and_then(|epoch| chrono::DateTime::from_timestamp(epoch, 0))
        .map(|dt| dt.to_rfc3339())
}

fn timestamps_of(entry: &SevenzEntry) -> Mactb {
    Mactb {
        modified: ts(entry.modified),
        accessed: ts(entry.accessed),
        created: ts(entry.created),
        mft_changed: None,
        birth: ts(entry.created).or_else(|| ts(entry.modified)),
    }
}

fn root_node(image: &Arc<DiskImage>) -> FsNode {
    let label = image
        .sevenz
        .as_ref()
        .map(|sevenz| sevenz.label().to_string())
        .unwrap_or_else(|| "7z evidence".to_string());
    FsNode {
        id: "sevenz:".to_string(),
        name: crate::core::common::navigation_name::volume_name(&label),
        parent_id: None,
        kind: NodeKind::Dir,
        size: 0,
        deleted: false,
        inode: None,
        timestamps: Mactb::default(),
        link_target: None,
        has_snapshot: false,
    }
}

fn entry_to_node(entry: &SevenzEntry) -> FsNode {
    FsNode {
        id: sevenz_id_for(&entry.rel_path),
        name: name_of(&entry.rel_path),
        parent_id: parent_id_for(&entry.rel_path),
        kind: node_kind_of(entry.kind),
        size: entry.size,
        deleted: false,
        inode: None,
        timestamps: timestamps_of(entry),
        link_target: None,
        has_snapshot: false,
    }
}

pub fn root_children(image: &Arc<DiskImage>) -> Vec<FsNode> {
    vec![root_node(image)]
}

pub fn children(image: &Arc<DiskImage>, rel: &str) -> Result<Vec<FsNode>> {
    let sevenz = image
        .sevenz
        .as_ref()
        .ok_or_else(|| anyhow!("image is not a 7z source"))?;
    let mut out: Vec<FsNode> = sevenz
        .children(rel)
        .into_iter()
        .map(entry_to_node)
        .collect();
    out.sort_by(|a, b| match (a.kind, b.kind) {
        (NodeKind::Dir, NodeKind::Dir) => a.name.cmp(&b.name),
        (NodeKind::Dir, _) => std::cmp::Ordering::Less,
        (_, NodeKind::Dir) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });
    Ok(out)
}

pub fn read_file(image: &Arc<DiskImage>, rel: &str, max_bytes: usize) -> Result<Vec<u8>> {
    let sevenz = image
        .sevenz
        .as_ref()
        .ok_or_else(|| anyhow!("image is not a 7z source"))?;
    if rel.is_empty() {
        bail!("7z root is a directory");
    }
    sevenz.read(rel, max_bytes)
}

pub fn file_stat(image: &Arc<DiskImage>, rel: &str, node_id: &str) -> Result<FsStat> {
    let sevenz = image
        .sevenz
        .as_ref()
        .ok_or_else(|| anyhow!("image is not a 7z source"))?;
    let entry = sevenz
        .lookup(rel)
        .ok_or_else(|| anyhow!("not found in 7z: {rel}"))?;
    let is_readable = matches!(entry.kind, SevenzEntryKind::File);
    let sample = if is_readable && entry.size > 0 {
        sevenz.read(rel, SAMPLE_BYTES).unwrap_or_default()
    } else {
        Vec::new()
    };
    Ok(FsStat {
        node_id: node_id.to_string(),
        size: entry.size,
        inode: None,
        mode: Some(entry.mode as u16),
        mime: detect_mime_bytes(&sample).to_string(),
        magic_hex: if sample.is_empty() {
            None
        } else {
            Some(
                sample
                    .iter()
                    .take(16)
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        },
        entropy: if sample.is_empty() {
            None
        } else {
            Some(shannon_entropy(&sample))
        },
        data_runs: Vec::<FsDataRun>::new(),
    })
}

pub fn stream_file<F>(
    image: &Arc<DiskImage>,
    rel: &str,
    chunk_size: usize,
    visitor: F,
) -> Result<u64>
where
    F: FnMut(&[u8]) -> Result<()>,
{
    let sevenz = image
        .sevenz
        .as_ref()
        .ok_or_else(|| anyhow!("image is not a 7z source"))?;
    sevenz.stream_file(rel, chunk_size, visitor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_roundtrip_root_and_path() {
        assert_eq!(sevenz_id_for(""), "sevenz:");
        assert_eq!(parse_sevenz_id("sevenz:"), Some(String::new()));
        let id = sevenz_id_for("docs/report.pdf");
        assert_eq!(parse_sevenz_id(&id), Some("docs/report.pdf".to_string()));
    }
}
