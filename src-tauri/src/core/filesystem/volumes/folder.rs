//! Read-only folder-source adapter for the FS tree. Mirrors
//! `core::filesystem::volumes::docker` but walks an on-disk directory tree instead of
//! a merged docker archive.
//!
//! Node IDs encode the relative path inside the folder as hex bytes:
//! `folder:{hex(relative/path)}`. The empty-hex form `folder:` means the
//! folder root; the dispatcher emits one such root node in place of a
//! partition list when `image.folder.is_some()`.

use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};

use crate::core::evidence::image::DiskImage;
use crate::core::evidence::readers::folder_image::{FolderEntry, FolderEntryKind};
use crate::core::filesystem::tree::fs_tree::{
    detect_mime_bytes, shannon_entropy, FsDataRun, FsNode, FsStat, Mactb, NodeKind,
};

const SAMPLE_BYTES: usize = 4096;

/// Parses a `folder:{hex(rel-path)}` id. Returns `Some(rel)` where `rel`
/// is the relative UTF-8 path inside the folder root, or `Some("")` for
/// the root. Returns `None` for non-folder ids.
pub fn parse_folder_id(id: &str) -> Option<String> {
    let rest = id.strip_prefix("folder:")?;
    if rest.is_empty() {
        return Some(String::new());
    }
    let bytes = hex::decode(rest).ok()?;
    String::from_utf8(bytes).ok()
}

fn folder_id_for(rel: &str) -> String {
    let trimmed = rel.trim_matches('/');
    if trimmed.is_empty() {
        "folder:".to_string()
    } else {
        format!("folder:{}", hex::encode(trimmed))
    }
}

fn parent_id_for(rel: &str) -> Option<String> {
    let trimmed = rel.trim_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.rsplit_once('/') {
        Some((parent, _)) if !parent.is_empty() => Some(folder_id_for(parent)),
        _ => Some("folder:".to_string()),
    }
}

fn name_of(rel: &str) -> String {
    let trimmed = rel.trim_matches('/');
    if trimmed.is_empty() {
        return "folder:root".to_string();
    }
    trimmed
        .rsplit('/')
        .next()
        .map(|s| s.to_string())
        .unwrap_or_else(|| trimmed.to_string())
}

fn node_kind_of(kind: FolderEntryKind) -> NodeKind {
    match kind {
        FolderEntryKind::Dir => NodeKind::Dir,
        FolderEntryKind::Symlink => NodeKind::Symlink,
        FolderEntryKind::File => NodeKind::File,
    }
}

fn timestamps_of(entry: &FolderEntry) -> Mactb {
    fn format(t: Option<std::time::SystemTime>) -> Option<String> {
        let t = t?;
        let dur = t.duration_since(std::time::UNIX_EPOCH).ok()?;
        chrono::DateTime::from_timestamp(dur.as_secs() as i64, dur.subsec_nanos())
            .map(|dt| dt.to_rfc3339())
    }
    Mactb {
        modified: format(entry.modified),
        accessed: format(entry.accessed),
        created: format(entry.created),
        mft_changed: format(entry.status_changed),
        birth: format(entry.created),
    }
}

fn root_node(image: &Arc<DiskImage>) -> FsNode {
    let label = image
        .folder
        .as_ref()
        .map(|folder| folder.label().to_string())
        .unwrap_or_else(|| "folder evidence".to_string());
    FsNode {
        id: "folder:".to_string(),
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

fn entry_to_node(entry: &FolderEntry) -> FsNode {
    let name = name_of(&entry.rel_path);
    FsNode {
        id: folder_id_for(&entry.rel_path),
        name: if entry.is_backup_root() {
            crate::core::evidence::readers::mi_bak::backup_directory_name(&name).to_owned()
        } else {
            name
        },
        parent_id: parent_id_for(&entry.rel_path),
        kind: node_kind_of(entry.kind),
        size: entry.size,
        deleted: false,
        inode: entry.inode,
        timestamps: timestamps_of(entry),
        link_target: entry.link_target.clone(),
        has_snapshot: false,
    }
}

pub fn root_children(image: &Arc<DiskImage>) -> Vec<FsNode> {
    vec![root_node(image)]
}

pub fn children(image: &Arc<DiskImage>, rel: &str) -> Result<Vec<FsNode>> {
    let folder = image
        .folder
        .as_ref()
        .ok_or_else(|| anyhow!("image is not a folder source"))?;
    let mut out: Vec<FsNode> = folder
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
    let folder = image
        .folder
        .as_ref()
        .ok_or_else(|| anyhow!("image is not a folder source"))?;
    if rel.is_empty() {
        bail!("folder root is a directory");
    }
    folder.read(rel, max_bytes)
}

pub fn file_stat(image: &Arc<DiskImage>, rel: &str, node_id: &str) -> Result<FsStat> {
    let folder = image
        .folder
        .as_ref()
        .ok_or_else(|| anyhow!("image is not a folder source"))?;
    let entry = folder
        .lookup(rel)
        .ok_or_else(|| anyhow!("not found in folder: {rel}"))?;
    let sample = if matches!(entry.kind, FolderEntryKind::File) && entry.size > 0 {
        folder.read(rel, SAMPLE_BYTES).unwrap_or_default()
    } else {
        Vec::new()
    };
    let data_runs = if matches!(entry.kind, FolderEntryKind::File) && entry.size > 0 {
        // No on-disk LBA makes sense for an OS-mediated file read, but the
        // run table is the natural place to surface "where the bytes live".
        // Encode logical-offset = 0, physical-offset = 0, length = size so
        // the frontend's data-run panel still shows something coherent.
        vec![FsDataRun {
            logical_offset: 0,
            physical_offset: 0,
            length: entry.size,
        }]
    } else {
        Vec::new()
    };
    Ok(FsStat {
        node_id: node_id.to_string(),
        size: entry.size,
        inode: entry.inode,
        mode: entry
            .mode
            .map(u16::try_from)
            .transpose()
            .context("iOS file mode exceeds stat field width")?,
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
    rel: &str,
    chunk_size: usize,
    visitor: F,
) -> Result<u64>
where
    F: FnMut(&[u8]) -> Result<()>,
{
    let folder = image
        .folder
        .as_ref()
        .ok_or_else(|| anyhow!("image is not a folder source"))?;
    folder.stream_file(rel, chunk_size, visitor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_roundtrip_root_and_path() {
        assert_eq!(folder_id_for(""), "folder:");
        assert_eq!(parse_folder_id("folder:"), Some(String::new()));
        let id = folder_id_for("nested/file.txt");
        assert_eq!(parse_folder_id(&id), Some("nested/file.txt".to_string()));
    }

    #[test]
    fn parent_chain_walks_to_root() {
        assert_eq!(parent_id_for(""), None);
        assert_eq!(parent_id_for("a"), Some("folder:".to_string()));
        let id = parent_id_for("a/b/c").unwrap();
        assert_eq!(parse_folder_id(&id), Some("a/b".to_string()));
    }
}
