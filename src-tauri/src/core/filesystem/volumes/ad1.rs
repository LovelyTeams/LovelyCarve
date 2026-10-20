//! Read-only AD1-source adapter for the FS tree.
//!
//! Node IDs are `ad1:` for the synthetic image root and `ad1:{index}` for an
//! item, `index` being its position in the image's depth-first item list.
//! Indices rather than paths, because AD1 allows two siblings with the same
//! name. AD1 timestamps carry no time zone; like FAT times they are surfaced as
//! UTC.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use chrono::{NaiveDateTime, TimeZone, Utc};

use crate::core::evidence::image::DiskImage;
use crate::core::evidence::readers::ad1_image::Ad1Image;
use crate::core::filesystem::tree::fs_tree::{
    detect_mime_bytes, shannon_entropy, FsNode, FsStat, Mactb, NodeKind,
};
use ad1::Ad1Entry;

const SAMPLE_BYTES: usize = 4096;
const ROOT_ID: &str = "ad1:";

/// A parsed AD1 node id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ad1NodeId {
    /// The synthetic root listing the image's top-level items.
    Root,
    /// One item, by index.
    Entry(usize),
}

pub fn parse_ad1_id(id: &str) -> Option<Ad1NodeId> {
    let rest = id.strip_prefix(ROOT_ID)?;
    if rest.is_empty() {
        return Some(Ad1NodeId::Root);
    }
    rest.parse().ok().map(Ad1NodeId::Entry)
}

fn entry_id(index: usize) -> String {
    format!("{ROOT_ID}{index}")
}

fn image_ad1(image: &Arc<DiskImage>) -> Result<&Ad1Image> {
    image
        .ad1
        .as_deref()
        .ok_or_else(|| anyhow!("image is not an AD1 source"))
}

/// An AD1 timestamp (`YYYYMMDDThhmmss`, optionally with fractional seconds)
/// as UTC RFC 3339.
fn timestamp(value: Option<&str>) -> Option<String> {
    let raw = value?.trim().trim_end_matches('Z');
    let parsed = NaiveDateTime::parse_from_str(raw, "%Y%m%dT%H%M%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(raw, "%Y%m%dT%H%M%S"));
    match parsed {
        Ok(naive) => Some(Utc.from_utc_datetime(&naive).to_rfc3339()),
        Err(error) => {
            tracing::debug!("AD1 timestamp {raw:?} not understood: {error}");
            None
        }
    }
}

fn leaf_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn entry_node(index: usize, entry: &Ad1Entry) -> FsNode {
    FsNode {
        id: entry_id(index),
        name: leaf_name(&entry.path).to_string(),
        parent_id: Some(entry.parent.map_or_else(|| ROOT_ID.to_string(), entry_id)),
        kind: if entry.is_dir {
            NodeKind::Dir
        } else {
            NodeKind::File
        },
        size: if entry.is_dir { 0 } else { entry.size },
        deleted: false,
        inode: None,
        timestamps: Mactb {
            modified: timestamp(entry.modified.as_deref()),
            accessed: timestamp(entry.accessed.as_deref()),
            created: None,
            mft_changed: timestamp(entry.changed.as_deref()),
            birth: None,
        },
        link_target: None,
        has_snapshot: false,
    }
}

fn root_node(image: &Arc<DiskImage>) -> FsNode {
    let label = image
        .ad1
        .as_ref()
        .map(|ad1| ad1.label().to_string())
        .unwrap_or_else(|| "AD1 evidence".to_string());
    FsNode {
        id: ROOT_ID.to_string(),
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

pub fn root_children(image: &Arc<DiskImage>) -> Vec<FsNode> {
    vec![root_node(image)]
}

/// The node for `id`, built directly from the item list.
pub fn node(image: &Arc<DiskImage>, id: Ad1NodeId) -> Result<FsNode> {
    match id {
        Ad1NodeId::Root => Ok(root_node(image)),
        Ad1NodeId::Entry(index) => {
            let entry = image_ad1(image)?
                .entry(index)
                .ok_or_else(|| anyhow!("AD1 entry {index} does not exist"))?;
            Ok(entry_node(index, entry))
        }
    }
}

pub fn children(image: &Arc<DiskImage>, id: Ad1NodeId) -> Result<Vec<FsNode>> {
    let ad1 = image_ad1(image)?;
    let indices = match id {
        Ad1NodeId::Root => ad1.roots(),
        Ad1NodeId::Entry(index) => ad1.children(index),
    };
    let mut out: Vec<FsNode> = indices
        .iter()
        .filter_map(|&index| ad1.entry(index).map(|entry| entry_node(index, entry)))
        .collect();
    out.sort_by(|a, b| match (a.kind, b.kind) {
        (NodeKind::Dir, NodeKind::Dir) => a.name.cmp(&b.name),
        (NodeKind::Dir, _) => std::cmp::Ordering::Less,
        (_, NodeKind::Dir) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });
    Ok(out)
}

fn file_index(id: Ad1NodeId) -> Result<usize> {
    match id {
        Ad1NodeId::Entry(index) => Ok(index),
        Ad1NodeId::Root => Err(anyhow!("AD1 root is a directory")),
    }
}

pub fn read_file(image: &Arc<DiskImage>, id: Ad1NodeId, max_bytes: usize) -> Result<Vec<u8>> {
    image_ad1(image)?.read(file_index(id)?, max_bytes)
}

/// Random-access read; AD1 data chunks inflate independently.
pub fn read_range(
    image: &Arc<DiskImage>,
    id: Ad1NodeId,
    offset: u64,
    length: usize,
) -> Result<Vec<u8>> {
    image_ad1(image)?.read_range(file_index(id)?, offset, length)
}

pub fn stream_file<F>(
    image: &Arc<DiskImage>,
    id: Ad1NodeId,
    chunk_size: usize,
    visitor: F,
) -> Result<u64>
where
    F: FnMut(&[u8]) -> Result<()>,
{
    image_ad1(image)?.stream_file(file_index(id)?, chunk_size, visitor)
}

pub fn file_stat(image: &Arc<DiskImage>, id: Ad1NodeId, node_id: &str) -> Result<FsStat> {
    let ad1 = image_ad1(image)?;
    let (size, sample) = match id {
        Ad1NodeId::Root => (0, Vec::new()),
        Ad1NodeId::Entry(index) => {
            let entry = ad1
                .entry(index)
                .ok_or_else(|| anyhow!("AD1 entry {index} does not exist"))?;
            let sample = if entry.is_dir || entry.size == 0 {
                Vec::new()
            } else {
                ad1.read(index, SAMPLE_BYTES)?
            };
            (if entry.is_dir { 0 } else { entry.size }, sample)
        }
    };
    Ok(FsStat {
        node_id: node_id.to_string(),
        size,
        inode: None,
        mode: None,
        mime: detect_mime_bytes(&sample).to_string(),
        magic_hex: (!sample.is_empty()).then(|| {
            sample
                .iter()
                .take(16)
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(" ")
        }),
        entropy: (!sample.is_empty()).then(|| shannon_entropy(&sample)),
        data_runs: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        assert_eq!(parse_ad1_id("ad1:"), Some(Ad1NodeId::Root));
        assert_eq!(parse_ad1_id(&entry_id(42)), Some(Ad1NodeId::Entry(42)));
        assert_eq!(parse_ad1_id("ad1:x"), None);
        assert_eq!(parse_ad1_id("zip:"), None);
    }

    #[test]
    fn timestamps_are_utc_without_a_zone() {
        assert_eq!(
            timestamp(Some("20241130T101500")).as_deref(),
            Some("2024-11-30T10:15:00+00:00")
        );
        assert_eq!(
            timestamp(Some("20241130T101500.250Z")).as_deref(),
            Some("2024-11-30T10:15:00.250+00:00")
        );
        assert_eq!(timestamp(Some("yesterday")), None);
        assert_eq!(timestamp(None), None);
    }
}
