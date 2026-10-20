//! Read-only tar-source adapter for the FS tree. Mirrors
//! `core::filesystem::volumes::folder` / `core::filesystem::volumes::docker` but walks a plain
//! `.tar` archive (a phone / filesystem extraction) instead of an on-disk
//! directory or a merged docker image.
//!
//! Node IDs encode the archive-relative path as hex bytes: `tar:{hex(path)}`.
//! The empty-hex form `tar:` means the archive root; the dispatcher emits one
//! such root node in place of a partition list when `image.tar.is_some()`.

use std::sync::Arc;

use anyhow::{anyhow, bail, Result};

use crate::core::evidence::image::DiskImage;
use crate::core::evidence::readers::tar_image::{TarEntry, TarEntryKind};
use crate::core::filesystem::tree::fs_tree::{
    detect_mime_bytes, shannon_entropy, FsDataRun, FsNode, FsStat, Mactb, NodeKind,
};

const SAMPLE_BYTES: usize = 4096;

/// Parses a `tar:{hex(rel-path)}` id. Returns `Some(rel)` where `rel` is the
/// archive-relative UTF-8 path, or `Some("")` for the root. Returns `None` for
/// non-tar ids.
pub fn parse_tar_id(id: &str) -> Option<String> {
    let rest = id.strip_prefix("tar:")?;
    if rest.is_empty() {
        return Some(String::new());
    }
    let bytes = hex::decode(rest).ok()?;
    String::from_utf8(bytes).ok()
}

fn tar_id_for(rel: &str) -> String {
    let trimmed = rel.trim_matches('/');
    if trimmed.is_empty() {
        "tar:".to_string()
    } else {
        format!("tar:{}", hex::encode(trimmed))
    }
}

fn parent_id_for(rel: &str) -> Option<String> {
    let trimmed = rel.trim_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.rsplit_once('/') {
        Some((parent, _)) if !parent.is_empty() => Some(tar_id_for(parent)),
        _ => Some("tar:".to_string()),
    }
}

fn name_of(rel: &str) -> String {
    let trimmed = rel.trim_matches('/');
    if trimmed.is_empty() {
        return "tar:root".to_string();
    }
    trimmed
        .rsplit('/')
        .next()
        .map(|s| s.to_string())
        .unwrap_or_else(|| trimmed.to_string())
}

fn node_kind_of(kind: TarEntryKind) -> NodeKind {
    match kind {
        TarEntryKind::Dir => NodeKind::Dir,
        TarEntryKind::Symlink => NodeKind::Symlink,
        // Hardlinks resolve to a regular file's bytes, so they read like files.
        TarEntryKind::File | TarEntryKind::Hardlink => NodeKind::File,
        // Char / block device or FIFO: no readable byte content. Mirror the
        // docker adapter (Unallocated) so the export walk skips them and the UI
        // shows no read affordance — surfacing them as File would leave stray
        // zero-byte files on export and pollute directory stats, and every read
        // attempt would error.
        TarEntryKind::Other => NodeKind::Unallocated,
    }
}

/// Tar headers carry only an mtime, recorded as a Unix epoch second. Surface it
/// as both `modified` and `birth` (the closest analogue the timeline has) and
/// leave the rest empty.
fn timestamps_of(entry: &TarEntry) -> Mactb {
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
        .tar
        .as_ref()
        .map(|tar| tar.label().to_string())
        .unwrap_or_else(|| "tar evidence".to_string());
    FsNode {
        id: "tar:".to_string(),
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

fn entry_to_node(entry: &TarEntry) -> FsNode {
    let link_target = match entry.kind {
        TarEntryKind::Symlink | TarEntryKind::Hardlink => entry.link_target.clone(),
        TarEntryKind::File | TarEntryKind::Dir | TarEntryKind::Other => None,
    };
    FsNode {
        id: tar_id_for(&entry.rel_path),
        name: name_of(&entry.rel_path),
        parent_id: parent_id_for(&entry.rel_path),
        kind: node_kind_of(entry.kind),
        // Hardlinks keep their own header size here (0) rather than the link
        // target's — so directory rollups in `fs_tree::directory_stats` count
        // shared bytes once (on the target, du-style). The detail card resolves
        // the target size separately in `file_stat`; that divergence is
        // intentional (see the note there).
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
    let tar = image
        .tar
        .as_ref()
        .ok_or_else(|| anyhow!("image is not a tar source"))?;
    let mut out: Vec<FsNode> = tar.children(rel).into_iter().map(entry_to_node).collect();
    out.sort_by(|a, b| match (a.kind, b.kind) {
        (NodeKind::Dir, NodeKind::Dir) => a.name.cmp(&b.name),
        (NodeKind::Dir, _) => std::cmp::Ordering::Less,
        (_, NodeKind::Dir) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });
    Ok(out)
}

pub fn read_file(image: &Arc<DiskImage>, rel: &str, max_bytes: usize) -> Result<Vec<u8>> {
    let tar = image
        .tar
        .as_ref()
        .ok_or_else(|| anyhow!("image is not a tar source"))?;
    if rel.is_empty() {
        bail!("tar root is a directory");
    }
    tar.read(rel, max_bytes)
}

pub fn file_stat(image: &Arc<DiskImage>, rel: &str, node_id: &str) -> Result<FsStat> {
    let tar = image
        .tar
        .as_ref()
        .ok_or_else(|| anyhow!("image is not a tar source"))?;
    let entry = tar
        .lookup(rel)
        .ok_or_else(|| anyhow!("not found in tar: {rel}"))?;
    // Sample / fingerprint only entries whose OWN size is the content size:
    // regular files and hardlinks (a hardlink reads its target's bytes). A
    // symlink is excluded — its `size` is the link-path length, so sampling the
    // resolved target would make the stat card's size, mime, and data-run
    // describe three different things (mirrors the docker adapter).
    let is_readable = matches!(entry.kind, TarEntryKind::File | TarEntryKind::Hardlink);
    // A hardlink header carries size 0 (its bytes live at the link target), so
    // resolve the real content size from the data run for readable entries —
    // otherwise the stat card would report size 0 with no mime/magic/entropy
    // while still emitting a non-empty data-run that points at the target's
    // bytes (an internally inconsistent stat). Falls back to the entry's own
    // size when there is no run (e.g. a legitimately empty regular file). This
    // is deliberately the target's size for a hardlink and so differs from the
    // tree node's `FsNode.size` (kept at 0); see `entry_to_node` — the rollup
    // must not double-count a hardlink's shared bytes.
    let content_size = if is_readable {
        tar.data_location(rel)
            .map(|(_, size)| size)
            .unwrap_or(entry.size)
    } else {
        entry.size
    };
    let sample = if is_readable && content_size > 0 {
        tar.read(rel, SAMPLE_BYTES).unwrap_or_default()
    } else {
        Vec::new()
    };
    // Unlike a folder source, tar reads ARE byte-offset reads into the same
    // image stream the hex view sees — so surface the real archive offset as
    // the physical run, letting the data-run panel jump hex to the file's bytes.
    let data_runs = if is_readable
        && image.info().source_format != crate::core::evidence::image::ImageFormat::AndroidBackup
    {
        match tar.data_location(rel) {
            Some((offset, size)) if size > 0 => vec![FsDataRun {
                logical_offset: 0,
                physical_offset: offset,
                length: size,
            }],
            _ => Vec::new(),
        }
    } else {
        Vec::new()
    };
    Ok(FsStat {
        node_id: node_id.to_string(),
        size: content_size,
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
    let tar = image
        .tar
        .as_ref()
        .ok_or_else(|| anyhow!("image is not a tar source"))?;
    tar.stream_file(rel, chunk_size, visitor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_roundtrip_root_and_path() {
        assert_eq!(tar_id_for(""), "tar:");
        assert_eq!(parse_tar_id("tar:"), Some(String::new()));
        let id = tar_id_for("system/app/x.apk");
        assert_eq!(parse_tar_id(&id), Some("system/app/x.apk".to_string()));
    }

    #[test]
    fn parent_chain_walks_to_root() {
        assert_eq!(parent_id_for(""), None);
        assert_eq!(parent_id_for("a"), Some("tar:".to_string()));
        let id = parent_id_for("a/b/c").unwrap();
        assert_eq!(parse_tar_id(&id), Some("a/b".to_string()));
    }

    #[test]
    fn non_tar_id_rejected() {
        assert_eq!(parse_tar_id("folder:6162"), None);
        assert_eq!(parse_tar_id("part:0"), None);
    }
}
