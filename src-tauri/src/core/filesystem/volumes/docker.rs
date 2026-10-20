//! Read-only Docker / OCI image-archive adapter for the FS tree.
//!
//! `core::evidence::readers::docker_image::DockerImage` already merges every layer into a
//! single `BTreeMap<String, FileEntry>`; this module is the thin shim that
//! turns those entries into the `FsNode` / `FsStat` shapes the dispatcher
//! and the frontend understand.
//!
//! Node IDs encode the absolute path inside the merged image as hex bytes:
//! `docker:{hex(/path/inside/image)}`. The empty-hex form `docker:` means
//! the image root; the dispatcher emits one such root node in place of a
//! partition list when `image.docker.is_some()`.

use std::sync::Arc;

use anyhow::{anyhow, bail, Result};

use crate::core::evidence::image::DiskImage;
use crate::core::evidence::readers::docker_image::{EntryKind, FileEntry};
use crate::core::filesystem::tree::fs_tree::{
    detect_mime_bytes, shannon_entropy, FsDataRun, FsNode, FsStat, Mactb, NodeKind,
};

const SAMPLE_BYTES: usize = 4096;

/// Parses a `docker:{hex(path)}` id. Returns `Some(path)` where `path` is
/// the absolute (with leading `/`) UTF-8 path inside the merged image, or
/// `Some("")` for the root. Returns `None` for non-docker ids.
pub fn parse_docker_id(id: &str) -> Option<String> {
    let rest = id.strip_prefix("docker:")?;
    if rest.is_empty() {
        return Some(String::new());
    }
    let bytes = hex::decode(rest).ok()?;
    String::from_utf8(bytes).ok()
}

fn docker_id_for(path: &str) -> String {
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        "docker:".to_string()
    } else {
        format!("docker:{}", hex::encode(format!("/{trimmed}")))
    }
}

fn parent_id_for(path: &str) -> Option<String> {
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.rsplit_once('/') {
        Some((parent, _)) if !parent.is_empty() => Some(docker_id_for(parent)),
        _ => Some("docker:".to_string()),
    }
}

fn name_of(path: &str) -> String {
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        return "docker:image".to_string();
    }
    trimmed
        .rsplit('/')
        .next()
        .map(|s| s.to_string())
        .unwrap_or_else(|| trimmed.to_string())
}

fn node_kind_of(kind: EntryKind) -> NodeKind {
    match kind {
        EntryKind::Dir => NodeKind::Dir,
        EntryKind::Symlink => NodeKind::Symlink,
        EntryKind::File | EntryKind::Hardlink => NodeKind::File,
        EntryKind::CharDevice | EntryKind::BlockDevice | EntryKind::Fifo => NodeKind::Unallocated,
    }
}

fn timestamps_of(mtime: i64) -> Mactb {
    if mtime <= 0 {
        return Mactb::default();
    }
    let formatted = chrono::DateTime::from_timestamp(mtime, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default();
    if formatted.is_empty() {
        Mactb::default()
    } else {
        Mactb {
            modified: Some(formatted.clone()),
            accessed: None,
            created: None,
            mft_changed: None,
            birth: Some(formatted),
        }
    }
}

fn root_node(image: &Arc<DiskImage>) -> FsNode {
    let label = image
        .docker
        .as_ref()
        .and_then(|docker| docker.repo_tags().first().cloned())
        .unwrap_or_else(|| "Docker / OCI image".to_string());
    FsNode {
        id: "docker:".to_string(),
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

fn entry_to_node(entry: &FileEntry) -> FsNode {
    FsNode {
        id: docker_id_for(&entry.path),
        name: name_of(&entry.path),
        parent_id: parent_id_for(&entry.path),
        kind: node_kind_of(entry.kind),
        size: entry.size,
        deleted: false,
        inode: None,
        timestamps: timestamps_of(entry.mtime),
        link_target: None,
        has_snapshot: false,
    }
}

/// Returns the synthetic root listing the dispatcher hands back when
/// `parent_id` is empty / "root" on a docker-backed image.
pub fn root_children(image: &Arc<DiskImage>) -> Vec<FsNode> {
    vec![root_node(image)]
}

/// Lists the direct children of `path` inside the merged image. Used when
/// the dispatcher sees a `docker:...` parent id.
pub fn children(image: &Arc<DiskImage>, path: &str) -> Result<Vec<FsNode>> {
    let docker = image
        .docker
        .as_ref()
        .ok_or_else(|| anyhow!("image is not a docker / OCI archive"))?;
    let mut out: Vec<FsNode> = docker
        .children(path)
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

pub fn read_file(image: &Arc<DiskImage>, path: &str, max_bytes: usize) -> Result<Vec<u8>> {
    let docker = image
        .docker
        .as_ref()
        .ok_or_else(|| anyhow!("image is not a docker / OCI archive"))?;
    if path.is_empty() {
        bail!("docker root is a directory");
    }
    docker.read(path, max_bytes)
}

pub fn file_stat(image: &Arc<DiskImage>, path: &str, node_id: &str) -> Result<FsStat> {
    let docker = image
        .docker
        .as_ref()
        .ok_or_else(|| anyhow!("image is not a docker / OCI archive"))?;
    let entry = docker
        .lookup(path)
        .ok_or_else(|| anyhow!("not found in docker image: {path}"))?;
    let mode = entry.mode & 0xFFFF;
    let sample = if matches!(entry.kind, EntryKind::File | EntryKind::Hardlink) && entry.size > 0 {
        docker.read(path, SAMPLE_BYTES).unwrap_or_default()
    } else {
        Vec::new()
    };
    let data_runs = if matches!(entry.kind, EntryKind::File | EntryKind::Hardlink) && entry.size > 0
    {
        // Layer index serves as the "physical" hint — every file in a
        // docker image lives inside exactly one layer, so this is a useful
        // forensic signal even though there's no on-disk LBA we could
        // reasonably surface here.
        vec![FsDataRun {
            logical_offset: 0,
            physical_offset: entry.layer_index as u64,
            length: entry.size,
        }]
    } else {
        Vec::new()
    };
    Ok(FsStat {
        node_id: node_id.to_string(),
        size: entry.size,
        inode: None,
        mode: Some(mode as u16),
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
        data_runs,
    })
}

pub fn stream_file<F>(
    image: &Arc<DiskImage>,
    path: &str,
    chunk_size: usize,
    visitor: F,
) -> Result<u64>
where
    F: FnMut(&[u8]) -> Result<()>,
{
    let docker = image
        .docker
        .as_ref()
        .ok_or_else(|| anyhow!("image is not a docker / OCI archive"))?;
    if path.is_empty() {
        bail!("docker root is a directory");
    }
    docker.stream(path, chunk_size, visitor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_roundtrip_root_and_path() {
        assert_eq!(docker_id_for("/"), "docker:");
        assert_eq!(parse_docker_id("docker:"), Some(String::new()));

        let id = docker_id_for("/etc/passwd");
        assert_eq!(parse_docker_id(&id), Some("/etc/passwd".to_string()));
    }

    #[test]
    fn parent_chain_walks_to_root() {
        assert_eq!(parent_id_for("/"), None);
        assert_eq!(parent_id_for("/etc"), Some("docker:".to_string()));
        let id = parent_id_for("/etc/ssh/sshd_config").unwrap();
        assert_eq!(parse_docker_id(&id), Some("/etc/ssh".to_string()));
    }
}
