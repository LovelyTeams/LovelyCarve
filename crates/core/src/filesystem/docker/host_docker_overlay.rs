//! Overlay2 layer composition for Docker containers found inside disk images.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use crate::filesystem::docker::host_docker::{container_node_id, find_node, HostDockerContainer};
use crate::filesystem::evidence_session::EvidenceSession;
use crate::filesystem::tree::fs_tree::{FsNode, NodeKind};
use crate::filesystem::volumes::ext4::{open_ext4_source, Ext4Volume};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MergedEntry {
    source_node: FsNode,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MergedContainerFs {
    entries: BTreeMap<String, MergedEntry>,
    children: HashMap<String, Vec<String>>,
}

impl MergedContainerFs {
    pub(crate) fn entry_count(&self) -> u64 {
        self.entries.len() as u64
    }

    pub(crate) fn walk_entries(
        &self,
        container_id: &str,
    ) -> impl Iterator<Item = (String, String, FsNode)> + '_ {
        let container_id = container_id.to_owned();
        self.entries.iter().map(move |(path, entry)| {
            let parent = container_node_id(&container_id, &parent_path(path));
            let mut node = entry.source_node.clone();
            node.id = container_node_id(&container_id, path);
            node.parent_id = Some(parent.clone());
            node.deleted = false;
            (parent, path.clone(), node)
        })
    }

    pub fn children(&self, container_id: &str, path: &str) -> Result<Vec<FsNode>> {
        let normalized = normalize_path(path);
        let child_paths = self.children.get(&normalized).cloned().unwrap_or_default();
        let mut nodes = child_paths
            .into_iter()
            .filter_map(|child_path| {
                self.entries.get(&child_path).map(|entry| {
                    let mut node = entry.source_node.clone();
                    node.id = container_node_id(container_id, &child_path);
                    node.parent_id = Some(container_node_id(container_id, &normalized));
                    node.deleted = false;
                    node
                })
            })
            .collect::<Vec<_>>();
        nodes.sort_by(|left, right| {
            let left_rank = usize::from(left.kind != NodeKind::Dir);
            let right_rank = usize::from(right.kind != NodeKind::Dir);
            left_rank
                .cmp(&right_rank)
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(nodes)
    }

    pub fn source_node(&self, path: &str) -> Option<&FsNode> {
        self.entries
            .get(&normalize_path(path))
            .map(|entry| &entry.source_node)
    }
}

pub(crate) fn compose(
    image: &Arc<EvidenceSession>,
    container: &HostDockerContainer,
) -> Result<MergedContainerFs> {
    let volume = open_ext4_source(image, container.source)?;
    let mut layer_ids = resolve_lower_layer_ids(&volume, container)?;
    layer_ids.reverse();
    if let Some(init_id) = container.init_id.as_deref() {
        if !layer_ids.iter().any(|layer_id| layer_id == init_id) {
            layer_ids.push(init_id.to_string());
        }
    }
    layer_ids.push(container.mount_id.clone());
    let mut seen = HashSet::new();
    layer_ids.retain(|layer_id| seen.insert(layer_id.clone()));

    let mut entries = BTreeMap::new();
    for layer_id in &layer_ids {
        let layer_path = ["overlay2", layer_id.as_str(), "diff"];
        let diff = find_node(&volume, container.docker_root_inode, &layer_path)?
            .ok_or_else(|| anyhow!("overlay2 layer diff not found: {layer_id}"))?;
        let diff_inode = diff
            .inode
            .ok_or_else(|| anyhow!("overlay2 layer diff has no inode: {layer_id}"))?;
        apply_layer(&volume, diff_inode, &mut entries)
            .with_context(|| format!("apply overlay2 layer {layer_id}"))?;
    }

    // Docker bind-mounts these three container-specific files over the init
    // layer placeholders. Project the actual host-side bytes into the virtual
    // container view so previews match what the process saw.
    for (path, node) in &container.runtime_files {
        replace_entry(&mut entries, path, node.clone());
    }

    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    for path in entries.keys() {
        children
            .entry(parent_path(path))
            .or_default()
            .push(path.clone());
    }
    Ok(MergedContainerFs { entries, children })
}

fn resolve_lower_layer_ids(
    volume: &Ext4Volume,
    container: &HostDockerContainer,
) -> Result<Vec<String>> {
    let mut layer_ids = Vec::new();
    for reference in &container.lower_references {
        let trimmed = reference.trim_matches('/');
        let layer_id = if let Some(link_name) = trimmed.strip_prefix("l/") {
            let link_path = ["overlay2", "l", link_name];
            let link = find_node(volume, container.docker_root_inode, &link_path)?
                .ok_or_else(|| anyhow!("overlay2 lower link not found: {reference}"))?;
            let target = link
                .link_target
                .as_deref()
                .ok_or_else(|| anyhow!("overlay2 lower link has no target: {reference}"))?;
            layer_id_from_target(target)
                .ok_or_else(|| anyhow!("invalid overlay2 lower link target: {target}"))?
        } else {
            layer_id_from_target(trimmed).unwrap_or_else(|| trimmed.to_string())
        };
        layer_ids.push(layer_id);
    }
    Ok(layer_ids)
}

fn layer_id_from_target(target: &str) -> Option<String> {
    let components = target
        .split('/')
        .filter(|component| !component.is_empty() && *component != "." && *component != "..")
        .collect::<Vec<_>>();
    let diff_index = components
        .iter()
        .position(|component| *component == "diff")?;
    diff_index
        .checked_sub(1)
        .and_then(|index| components.get(index))
        .map(|value| (*value).to_string())
}

fn apply_layer(
    volume: &Ext4Volume,
    diff_inode: u64,
    entries: &mut BTreeMap<String, MergedEntry>,
) -> Result<()> {
    if is_opaque_directory(volume, diff_inode)? {
        entries.clear();
    }
    let mut stack = volume
        .children(diff_inode, false)?
        .into_iter()
        .rev()
        .map(|node| (String::new(), node))
        .collect::<Vec<_>>();
    while let Some((parent, node)) = stack.pop() {
        if node.deleted {
            continue;
        }
        let path = join_path(&parent, &node.name);
        let inode_number = node
            .inode
            .ok_or_else(|| anyhow!("overlay2 entry has no Ext4 inode: {path}"))?;
        if is_whiteout(volume, &node, inode_number)? {
            remove_path(entries, &path, true);
            continue;
        }

        if node.kind == NodeKind::Dir {
            if is_opaque_directory(volume, inode_number)? {
                remove_path(entries, &path, false);
            }
            replace_entry(entries, &path, node.clone());
            let children = volume.children(inode_number, false)?;
            stack.extend(
                children
                    .into_iter()
                    .rev()
                    .map(|child| (path.clone(), child)),
            );
        } else {
            replace_entry(entries, &path, node);
        }
    }
    Ok(())
}

fn is_whiteout(volume: &Ext4Volume, node: &FsNode, inode_number: u64) -> Result<bool> {
    let inode = volume.read_inode(inode_number)?;
    let is_character_zero = inode.mode & 0xF000 == 0x2000 && inode.block[..4] == [0, 0, 0, 0];
    if is_character_zero {
        return Ok(true);
    }
    if node.kind != NodeKind::File || node.size != 0 {
        return Ok(false);
    }
    Ok(volume
        .extended_attributes(inode_number)?
        .iter()
        .any(|attribute| attribute.name.ends_with(".overlay.whiteout")))
}

fn is_opaque_directory(volume: &Ext4Volume, inode_number: u64) -> Result<bool> {
    Ok(volume
        .extended_attributes(inode_number)?
        .iter()
        .any(|attribute| {
            attribute.name.ends_with(".overlay.opaque")
                && attribute.value.first().copied() == Some(b'y')
        }))
}

fn replace_entry(entries: &mut BTreeMap<String, MergedEntry>, path: &str, source_node: FsNode) {
    if source_node.kind != NodeKind::Dir {
        remove_path(entries, path, false);
    }
    entries.insert(path.to_string(), MergedEntry { source_node });
}

fn remove_path(entries: &mut BTreeMap<String, MergedEntry>, path: &str, include_exact: bool) {
    let prefix = format!("{}/", path.trim_end_matches('/'));
    entries.retain(|candidate, _| {
        (!include_exact || candidate != path) && !candidate.starts_with(&prefix)
    });
}

fn normalize_path(path: &str) -> String {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("/{trimmed}")
    }
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        format!("/{name}")
    } else {
        format!("{}/{name}", parent.trim_end_matches('/'))
    }
}

fn parent_path(path: &str) -> String {
    let trimmed = path.trim_start_matches('/');
    match trimmed.rsplit_once('/') {
        Some((parent, _)) if !parent.is_empty() => format!("/{parent}"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::tree::fs_tree::Mactb;

    fn node(name: &str, kind: NodeKind) -> FsNode {
        FsNode {
            id: format!("source:{name}"),
            name: name.to_string(),
            parent_id: None,
            kind,
            size: 1,
            deleted: false,
            inode: Some(1),
            timestamps: Mactb::default(),
            link_target: None,
            has_snapshot: false,
        }
    }

    #[test]
    fn replacement_of_file_removes_lower_directory_children() {
        let mut entries = BTreeMap::new();
        replace_entry(&mut entries, "/etc", node("etc", NodeKind::Dir));
        replace_entry(&mut entries, "/etc/lower", node("lower", NodeKind::File));
        replace_entry(&mut entries, "/etc", node("etc", NodeKind::File));
        assert_eq!(entries.keys().cloned().collect::<Vec<_>>(), vec!["/etc"]);
        assert_eq!(
            entries.get("/etc").unwrap().source_node.kind,
            NodeKind::File
        );
    }

    #[test]
    fn opaque_removal_preserves_directory_itself() {
        let mut entries = BTreeMap::new();
        replace_entry(&mut entries, "/tmp", node("tmp", NodeKind::Dir));
        replace_entry(&mut entries, "/tmp/lower", node("lower", NodeKind::File));
        remove_path(&mut entries, "/tmp", false);
        assert!(entries.contains_key("/tmp"));
        assert!(!entries.contains_key("/tmp/lower"));
    }

    #[test]
    fn extracts_layer_id_from_overlay_link_target() {
        assert_eq!(
            layer_id_from_target("../abc123/diff"),
            Some("abc123".to_string())
        );
    }
}
