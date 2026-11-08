//! Walk and directory-stat helpers for filesystem tree nodes.
//!
//! `FsSession` keeps the cached traversal machinery in `fs_tree`; this module
//! owns the public walk facades and aggregate result types so the dispatcher
//! file can stay focused on routing.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::filesystem::evidence_session::EvidenceSession;
use crate::filesystem::tree::fs_tree::{FsNode, FsSession, Mactb, NodeKind};
use crate::filesystem::volumes::ad1 as ad1_volume_mod;
use crate::filesystem::volumes::apfs::parse_apfs_id;
use crate::filesystem::volumes::btrfs::{open_btrfs_source, parse_btrfs_id};
use crate::filesystem::volumes::ext4::{open_ext4_source, parse_ext4_id};
use crate::filesystem::volumes::lvm::parse_lvm_id;
use crate::filesystem::volumes::sevenz as sevenz_volume_mod;
use crate::filesystem::volumes::tar as tar_volume_mod;
use crate::filesystem::volumes::zip as zip_volume_mod;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsWalkEntry {
    pub node: FsNode,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsExtensionCount {
    pub extension: String,
    pub count: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsDirStats {
    pub node_id: String,
    pub path: String,
    pub file_count: u64,
    pub dir_count: u64,
    pub total_bytes: u64,
    pub largest_files: Vec<FsWalkEntry>,
    pub extensions: Vec<FsExtensionCount>,
    pub earliest_timestamp: Option<String>,
    pub latest_timestamp: Option<String>,
    pub truncated: bool,
}

pub fn walk_image(
    image: &Arc<EvidenceSession>,
    max_entries: usize,
) -> anyhow::Result<Vec<FsWalkEntry>> {
    walk_image_from(image, "", max_entries)
}

/// Like [`walk_image`] but rooted at `start_id`'s children. An empty
/// `start_id` walks the whole image; a directory node id scopes the walk to
/// that subtree.
pub fn walk_image_from(
    image: &Arc<EvidenceSession>,
    start_id: &str,
    max_entries: usize,
) -> anyhow::Result<Vec<FsWalkEntry>> {
    let mut out = Vec::new();
    let mut session = FsSession::new(image);
    session.walk_visit_from(start_id, &mut |entry| {
        if out.len() >= max_entries {
            return Ok(false);
        }
        out.push(entry.clone());
        Ok(out.len() < max_entries)
    })?;
    Ok(out)
}

pub fn directory_stats(
    image: &Arc<EvidenceSession>,
    node_id: &str,
    path_hint: &str,
    max_entries: usize,
) -> anyhow::Result<FsDirStats> {
    let mut session = FsSession::new(image);
    let start = if node_id.trim().is_empty() {
        FsNode {
            id: "root".to_string(),
            name: "镜像".to_string(),
            parent_id: None,
            kind: NodeKind::Dir,
            size: 0,
            deleted: false,
            inode: None,
            timestamps: Mactb::default(),
            link_target: None,
            has_snapshot: false,
        }
    } else if let Some((source, inode_number)) = parse_ext4_id(node_id) {
        let fs = open_ext4_source(image, source)?;
        let inode = fs.read_inode(inode_number)?;
        fs.node_from_inode(
            2,
            inode_number,
            path_hint.rsplit('/').next().unwrap_or("目录"),
            2,
            &inode,
        )
    } else if let Some((source, subvol, inode_number)) = parse_btrfs_id(node_id) {
        let fs = open_btrfs_source(image, source)?;
        let inode = fs.read_inode(subvol, inode_number)?;
        fs.node_from_inode(
            subvol,
            inode_number,
            inode_number,
            path_hint.rsplit('/').next().unwrap_or("目录"),
            2,
            &inode,
        )
    } else if let Some((part_id, _)) = parse_lvm_id(node_id) {
        session
            .children(&format!("part:{part_id}"))?
            .into_iter()
            .find(|node| node.id == node_id)
            .ok_or_else(|| anyhow::anyhow!("logical volume node not found: {node_id}"))?
    } else if let Some((source, path)) = parse_apfs_id(node_id) {
        if path.is_empty() {
            session
                .children(&format!("part:{}", source.part_id()))?
                .into_iter()
                .find(|node| node.id == node_id)
                .ok_or_else(|| anyhow::anyhow!("APFS volume node not found: {node_id}"))?
        } else {
            FsNode {
                id: node_id.to_string(),
                name: path_hint
                    .rsplit('/')
                    .find(|segment| !segment.is_empty())
                    .unwrap_or("目录")
                    .to_string(),
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
    } else if let Some(rel) = tar_volume_mod::parse_tar_id(node_id) {
        if rel.is_empty() {
            session
                .children("")?
                .into_iter()
                .find(|node| node.id == node_id)
                .ok_or_else(|| anyhow::anyhow!("tar root node not found: {node_id}"))?
        } else {
            FsNode {
                id: node_id.to_string(),
                name: path_hint
                    .rsplit('/')
                    .find(|segment| !segment.is_empty())
                    .map(|segment| segment.to_string())
                    .or_else(|| rel.rsplit('/').next().map(|segment| segment.to_string()))
                    .unwrap_or_else(|| "目录".to_string()),
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
    } else if let Some(rel) = zip_volume_mod::parse_zip_id(node_id) {
        if rel.is_empty() {
            session
                .children("")?
                .into_iter()
                .find(|node| node.id == node_id)
                .ok_or_else(|| anyhow::anyhow!("zip root node not found: {node_id}"))?
        } else {
            FsNode {
                id: node_id.to_string(),
                name: path_hint
                    .rsplit('/')
                    .find(|segment| !segment.is_empty())
                    .map(|segment| segment.to_string())
                    .or_else(|| rel.rsplit('/').next().map(|segment| segment.to_string()))
                    .unwrap_or_else(|| "目录".to_string()),
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
    } else if let Some(id) = ad1_volume_mod::parse_ad1_id(node_id) {
        ad1_volume_mod::node(image, id)?
    } else if let Some(rel) = sevenz_volume_mod::parse_sevenz_id(node_id) {
        if rel.is_empty() {
            session
                .children("")?
                .into_iter()
                .find(|node| node.id == node_id)
                .ok_or_else(|| anyhow::anyhow!("7z root node not found: {node_id}"))?
        } else {
            FsNode {
                id: node_id.to_string(),
                name: path_hint
                    .rsplit('/')
                    .find(|segment| !segment.is_empty())
                    .map(|segment| segment.to_string())
                    .or_else(|| rel.rsplit('/').next().map(|segment| segment.to_string()))
                    .unwrap_or_else(|| "目录".to_string()),
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
    } else {
        session
            .children("")?
            .into_iter()
            .find(|node| node.id == node_id)
            .ok_or_else(|| anyhow::anyhow!("directory node not found: {node_id}"))?
    };

    let start_name = start.name.clone();
    let mut entries = Vec::new();
    session.walk_visit_subtree(start, &mut |entry| {
        if entries.len() >= max_entries {
            return Ok(false);
        }
        entries.push(entry.clone());
        Ok(entries.len() < max_entries)
    })?;
    let truncated = entries.len() >= max_entries;
    let mut file_count = 0u64;
    let mut dir_count = 0u64;
    let mut total_bytes = 0u64;
    let mut ext_map = std::collections::BTreeMap::<String, (u64, u64)>::new();
    let mut timestamps = Vec::new();
    let mut file_indices: Vec<usize> = Vec::new();

    for (idx, entry) in entries.iter().enumerate().skip(1) {
        collect_timestamps(&entry.node, &mut timestamps);
        match entry.node.kind {
            NodeKind::Dir => dir_count += 1,
            NodeKind::File => {
                file_count += 1;
                total_bytes += entry.node.size;
                file_indices.push(idx);
                let ext = extension_of(&entry.node.name);
                let item = ext_map.entry(ext).or_insert((0, 0));
                item.0 += 1;
                item.1 += entry.node.size;
            }
            _ => {}
        }
    }

    file_indices.sort_by_key(|&idx| std::cmp::Reverse(entries[idx].node.size));
    let largest_files: Vec<_> = file_indices
        .iter()
        .take(8)
        .map(|&idx| entries[idx].clone())
        .collect();
    let mut extensions = ext_map
        .into_iter()
        .map(|(extension, (count, bytes))| FsExtensionCount {
            extension,
            count,
            bytes,
        })
        .collect::<Vec<_>>();
    extensions.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| b.count.cmp(&a.count)));
    timestamps.sort_by_key(|(epoch, _)| *epoch);

    Ok(FsDirStats {
        node_id: node_id.to_string(),
        path: if path_hint.is_empty() {
            format!("/{start_name}")
        } else {
            path_hint.to_string()
        },
        file_count,
        dir_count,
        total_bytes,
        largest_files,
        extensions: extensions.into_iter().take(12).collect(),
        earliest_timestamp: timestamps.first().map(|(_, s)| s.clone()),
        latest_timestamp: timestamps.last().map(|(_, s)| s.clone()),
        truncated,
    })
}

fn collect_timestamps(node: &FsNode, out: &mut Vec<(i64, String)>) {
    for ts in [
        &node.timestamps.modified,
        &node.timestamps.accessed,
        &node.timestamps.created,
        &node.timestamps.mft_changed,
        &node.timestamps.birth,
    ]
    .into_iter()
    .flatten()
    {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
            out.push((dt.timestamp(), ts.clone()));
        }
    }
}

fn extension_of(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .filter(|ext| !ext.is_empty() && ext.len() <= 16)
        .unwrap_or_else(|| "(none)".to_string())
}
