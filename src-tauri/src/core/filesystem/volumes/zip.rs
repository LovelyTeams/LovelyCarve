//! Read-only zip-source adapter for the FS tree. Mirrors
//! `core::filesystem::volumes::tar` but walks a `.zip` archive (a logical extraction /
//! triage collection / evidence bundle) instead of a plain tar.
//!
//! Node IDs encode the archive-relative path as hex bytes: `zip:{hex(path)}`.
//! The empty-hex form `zip:` means the archive root; the dispatcher emits one
//! such root node in place of a partition list when `image.zip.is_some()`.

use std::sync::Arc;

use anyhow::{anyhow, bail, Result};

use crate::core::evidence::image::DiskImage;
use crate::core::evidence::readers::zip_image::{ZipEntry, ZipEntryKind};
use crate::core::filesystem::tree::fs_tree::{
    detect_mime_bytes, shannon_entropy, FsDataRun, FsNode, FsStat, Mactb, NodeKind,
};

const SAMPLE_BYTES: usize = 4096;

/// Parses a `zip:{hex(rel-path)}` id. Returns `Some(rel)` where `rel` is the
/// archive-relative UTF-8 path, or `Some("")` for the root. Returns `None` for
/// non-zip ids.
pub fn parse_zip_id(id: &str) -> Option<String> {
    let rest = id.strip_prefix("zip:")?;
    if rest.is_empty() {
        return Some(String::new());
    }
    let bytes = hex::decode(rest).ok()?;
    String::from_utf8(bytes).ok()
}

fn zip_id_for(rel: &str) -> String {
    let trimmed = rel.trim_matches('/');
    if trimmed.is_empty() {
        "zip:".to_string()
    } else {
        format!("zip:{}", hex::encode(trimmed))
    }
}

fn parent_id_for(rel: &str) -> Option<String> {
    let trimmed = rel.trim_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.rsplit_once('/') {
        Some((parent, _)) if !parent.is_empty() => Some(zip_id_for(parent)),
        _ => Some("zip:".to_string()),
    }
}

fn name_of(rel: &str) -> String {
    let trimmed = rel.trim_matches('/');
    if trimmed.is_empty() {
        return "zip:root".to_string();
    }
    trimmed
        .rsplit('/')
        .next()
        .map(|s| s.to_string())
        .unwrap_or_else(|| trimmed.to_string())
}

fn node_kind_of(kind: ZipEntryKind) -> NodeKind {
    match kind {
        ZipEntryKind::Dir => NodeKind::Dir,
        ZipEntryKind::Symlink => NodeKind::Symlink,
        ZipEntryKind::File => NodeKind::File,
        // Encrypted / unsupported-compression member: no readable byte content.
        // Mirror the tar adapter (Unallocated) so the export walk skips it and
        // the UI shows no read affordance.
        ZipEntryKind::Other => NodeKind::Unallocated,
    }
}

/// ZIP records a single MS-DOS modification time. Surface it as both `modified`
/// and `birth` (the closest analogue the timeline has) and leave the rest empty.
fn timestamps_of(entry: &ZipEntry) -> Mactb {
    let modified = (entry.mtime > 0)
        .then(|| chrono::DateTime::from_timestamp(entry.mtime, 0).map(|dt| dt.to_rfc3339()))
        .flatten();
    Mactb {
        modified: modified.clone(),
        accessed: None,
        created: None,
        mft_changed: None,
        birth: modified,
    }
}

fn root_node(image: &Arc<DiskImage>) -> FsNode {
    let label = image
        .zip
        .as_ref()
        .map(|zip| zip.label().to_string())
        .unwrap_or_else(|| "zip evidence".to_string());
    FsNode {
        id: "zip:".to_string(),
        name: label,
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

fn entry_to_node(entry: &ZipEntry) -> FsNode {
    let name = name_of(&entry.rel_path);
    let link_target = match entry.kind {
        ZipEntryKind::Symlink => entry.link_target.clone(),
        ZipEntryKind::File | ZipEntryKind::Dir | ZipEntryKind::Other => None,
    };
    FsNode {
        id: zip_id_for(&entry.rel_path),
        name: if entry.is_backup_root() {
            crate::core::evidence::readers::mi_bak::backup_directory_name(&name).to_owned()
        } else {
            name
        },
        parent_id: parent_id_for(&entry.rel_path),
        kind: node_kind_of(entry.kind),
        size: entry.size,
        deleted: false,
        inode: None,
        timestamps: timestamps_of(entry),
        link_target,
        has_snapshot: false,
    }
}

pub fn root_children(image: &Arc<DiskImage>) -> Vec<FsNode> {
    vec![root_node(image)]
}

pub fn children(image: &Arc<DiskImage>, rel: &str) -> Result<Vec<FsNode>> {
    let zip = image
        .zip
        .as_ref()
        .ok_or_else(|| anyhow!("image is not a zip source"))?;
    let mut out: Vec<FsNode> = zip.children(rel).into_iter().map(entry_to_node).collect();
    out.sort_by(|a, b| match (a.kind, b.kind) {
        (NodeKind::Dir, NodeKind::Dir) => a.name.cmp(&b.name),
        (NodeKind::Dir, _) => std::cmp::Ordering::Less,
        (_, NodeKind::Dir) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });
    Ok(out)
}

pub fn read_file(image: &Arc<DiskImage>, rel: &str, max_bytes: usize) -> Result<Vec<u8>> {
    let zip = image
        .zip
        .as_ref()
        .ok_or_else(|| anyhow!("image is not a zip source"))?;
    if rel.is_empty() {
        bail!("zip root is a directory");
    }
    zip.read(rel, max_bytes)
}

pub fn file_stat(image: &Arc<DiskImage>, rel: &str, node_id: &str) -> Result<FsStat> {
    let zip = image
        .zip
        .as_ref()
        .ok_or_else(|| anyhow!("image is not a zip source"))?;
    let entry = zip
        .lookup(rel)
        .ok_or_else(|| anyhow!("not found in zip: {rel}"))?;
    // Sample / fingerprint only regular files. A symlink is excluded — its size
    // is the link-path length, so sampling the resolved target would make the
    // stat card's size, mime, and data-run describe three different things
    // (mirrors the tar / docker adapters).
    let is_readable = matches!(entry.kind, ZipEntryKind::File);
    let sample = if is_readable && entry.size > 0 {
        zip.read(rel, SAMPLE_BYTES).unwrap_or_default()
    } else {
        Vec::new()
    };
    // A data-run is only meaningful for STORED members, where the archived bytes
    // ARE the uncompressed content the hex view would show. DEFLATE members have
    // no logical-to-physical mapping (their bytes are compressed), so
    // `data_location` returns `None` and the panel offers no hex jump.
    let data_runs = match zip.data_location(rel) {
        Some((offset, size)) if is_readable && size > 0 => vec![FsDataRun {
            logical_offset: 0,
            physical_offset: offset,
            length: size,
        }],
        _ => Vec::new(),
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
    let zip = image
        .zip
        .as_ref()
        .ok_or_else(|| anyhow!("image is not a zip source"))?;
    zip.stream_file(rel, chunk_size, visitor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_roundtrip_root_and_path() {
        assert_eq!(zip_id_for(""), "zip:");
        assert_eq!(parse_zip_id("zip:"), Some(String::new()));
        let id = zip_id_for("docs/report.pdf");
        assert_eq!(parse_zip_id(&id), Some("docs/report.pdf".to_string()));
    }

    #[test]
    fn parent_chain_walks_to_root() {
        assert_eq!(parent_id_for(""), None);
        assert_eq!(parent_id_for("a"), Some("zip:".to_string()));
        let id = parent_id_for("a/b/c").unwrap();
        assert_eq!(parse_zip_id(&id), Some("a/b".to_string()));
    }

    #[test]
    fn non_zip_id_rejected() {
        assert_eq!(parse_zip_id("tar:6162"), None);
        assert_eq!(parse_zip_id("part:0"), None);
    }
}
