//! Read-only discovery of Docker Engine containers stored inside a Linux disk.
//!
//! Unlike [`crate::core::evidence::readers::docker_image`], which opens a `docker save`/OCI tar as
//! the evidence itself, this module finds `/var/lib/docker` on Ext4 volumes in
//! an ordinary disk image. Container metadata is indexed cheaply; OverlayFS
//! layers are composed only when a container node is expanded.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

use crate::core::evidence::image::PartitionKind;
use crate::core::filesystem::docker::host_docker_overlay::MergedContainerFs;
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::tree::fs_tree::{FsNode, FsSession, FsStat, Mactb, NodeKind};
use crate::core::filesystem::volumes::ext4::{open_ext4_source, Ext4Source, Ext4Volume};
use crate::core::filesystem::volumes::lvm::LvmVolume;

const GROUP_ID: &str = "host-docker:";
const EXT4_ROOT_INODE: u64 = 2;
const METADATA_LIMIT: usize = 4 * 1024 * 1024;

pub struct HostDocker {
    containers: Vec<Arc<HostDockerContainer>>,
    container_indexes: HashMap<String, usize>,
}

pub(crate) struct HostDockerContainer {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) image_reference: Option<String>,
    pub(crate) created: Option<String>,
    pub(crate) source: Ext4Source,
    pub(crate) docker_root_inode: u64,
    pub(crate) mount_id: String,
    pub(crate) init_id: Option<String>,
    pub(crate) lower_references: Vec<String>,
    pub(crate) runtime_files: Vec<(String, FsNode)>,
    pub(crate) merged: parking_lot::Mutex<Option<Arc<MergedContainerFs>>>,
    pub(crate) index_lock: parking_lot::Mutex<()>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostDockerNodeId {
    Group,
    Container { container_id: String, path: String },
}

impl fmt::Debug for HostDocker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostDocker")
            .field("container_count", &self.containers.len())
            .finish()
    }
}

pub fn parse_host_docker_id(id: &str) -> Option<HostDockerNodeId> {
    let rest = id.strip_prefix(GROUP_ID)?;
    if rest.is_empty() {
        return Some(HostDockerNodeId::Group);
    }
    let (container_id, encoded_path) = rest.split_once(':')?;
    if container_id.is_empty() {
        return None;
    }
    let path = if encoded_path.is_empty() {
        String::new()
    } else {
        String::from_utf8(hex::decode(encoded_path).ok()?).ok()?
    };
    Some(HostDockerNodeId::Container {
        container_id: container_id.to_string(),
        path,
    })
}

pub(crate) fn container_node_id(container_id: &str, path: &str) -> String {
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        format!("{GROUP_ID}{container_id}:")
    } else {
        format!(
            "{GROUP_ID}{container_id}:{}",
            hex::encode(format!("/{trimmed}"))
        )
    }
}

pub fn root_node(image: &Arc<EvidenceSession>) -> Option<FsNode> {
    let catalog = catalog(image)?;
    if catalog.containers.is_empty() {
        return None;
    }
    Some(FsNode {
        id: GROUP_ID.to_string(),
        name: "Docker 容器".to_string(),
        parent_id: None,
        kind: NodeKind::Dir,
        size: catalog.containers.len() as u64,
        deleted: false,
        inode: None,
        timestamps: Mactb::default(),
        link_target: None,
        has_snapshot: false,
    })
}

/// Metadata-only enumeration; never composes another container's layers.
pub(crate) fn volume_keys(image: &Arc<EvidenceSession>) -> Vec<String> {
    catalog(image)
        .map(|catalog| {
            catalog
                .containers
                .iter()
                .map(|container| container_node_id(&container.id, ""))
                .collect()
        })
        .unwrap_or_default()
}

/// Resolve the two directory shapes that are valid Dokan mount roots without
/// walking the physical partitions first. A whole-disk walk can hit its safety
/// cap before reaching the synthetic Docker peer, especially on large Linux
/// system volumes, so mount startup must address these nodes directly.
pub fn mount_root_node(
    image: &Arc<EvidenceSession>,
    id: &HostDockerNodeId,
) -> Result<Option<FsNode>> {
    match id {
        HostDockerNodeId::Group => Ok(root_node(image)),
        HostDockerNodeId::Container { container_id, path } if path.is_empty() => {
            let catalog =
                catalog(image).ok_or_else(|| anyhow!("disk image has no Docker containers"))?;
            Ok(Some(container_root_node(catalog.container(container_id)?)))
        }
        HostDockerNodeId::Container { .. } => Ok(None),
    }
}

pub fn children(image: &Arc<EvidenceSession>, id: &HostDockerNodeId) -> Result<Vec<FsNode>> {
    let catalog = catalog(image).ok_or_else(|| anyhow!("disk image has no Docker containers"))?;
    match id {
        HostDockerNodeId::Group => Ok(catalog
            .containers
            .iter()
            .map(|container| container_root_node(container))
            .collect()),
        HostDockerNodeId::Container { container_id, path } => {
            let container = catalog.container(container_id)?;
            container.merged(image)?.children(container_id, path)
        }
    }
}

pub fn read_file(
    image: &Arc<EvidenceSession>,
    id: &HostDockerNodeId,
    max_bytes: usize,
) -> Result<Vec<u8>> {
    let source_id = source_file_id(image, id)?;
    let mut session = FsSession::new(image);
    session.read_file_limited(&source_id, max_bytes)
}

pub fn file_stat(
    image: &Arc<EvidenceSession>,
    id: &HostDockerNodeId,
    node_id: &str,
) -> Result<FsStat> {
    if matches!(id, HostDockerNodeId::Group)
        || matches!(id, HostDockerNodeId::Container { path, .. } if path.is_empty())
    {
        return Ok(FsStat {
            node_id: node_id.to_string(),
            size: 0,
            inode: None,
            mode: Some(0o555),
            mime: "inode/directory".to_string(),
            magic_hex: None,
            entropy: None,
            data_runs: Vec::new(),
        });
    }
    let source_id = source_file_id(image, id)?;
    let mut session = FsSession::new(image);
    let mut stat = session.file_stat(&source_id)?;
    stat.node_id = node_id.to_string();
    Ok(stat)
}

pub fn stream_file<F>(
    image: &Arc<EvidenceSession>,
    id: &HostDockerNodeId,
    chunk_size: usize,
    visitor: F,
) -> Result<u64>
where
    F: FnMut(&[u8]) -> Result<()>,
{
    let source_id = source_file_id(image, id)?;
    let mut session = FsSession::new(image);
    session.stream_file(&source_id, chunk_size, visitor)
}

/// The node id of the host-filesystem file backing a container file (the
/// topmost overlay layer that provides `path`).
pub(crate) fn source_file_id(
    image: &Arc<EvidenceSession>,
    id: &HostDockerNodeId,
) -> Result<String> {
    let HostDockerNodeId::Container { container_id, path } = id else {
        bail!("Docker container group is a directory");
    };
    if path.is_empty() {
        bail!("Docker container root is a directory");
    }
    let catalog = catalog(image).ok_or_else(|| anyhow!("disk image has no Docker containers"))?;
    let container = catalog.container(container_id)?;
    Ok(container
        .merged(image)?
        .source_node(path)
        .ok_or_else(|| anyhow!("path not found in Docker container: {path}"))?
        .id
        .clone())
}

impl HostDocker {
    pub(crate) fn container(&self, id: &str) -> Result<&Arc<HostDockerContainer>> {
        let index = self
            .container_indexes
            .get(id)
            .copied()
            .ok_or_else(|| anyhow!("Docker container not found: {id}"))?;
        self.containers
            .get(index)
            .ok_or_else(|| anyhow!("Docker container index is invalid: {id}"))
    }
}

impl HostDockerContainer {
    pub(crate) fn merged(&self, image: &Arc<EvidenceSession>) -> Result<Arc<MergedContainerFs>> {
        let mut merged = self.merged.lock();
        if let Some(cached) = merged.as_ref() {
            return Ok(Arc::clone(cached));
        }
        let cache_dir = image.host_docker_cache_dir.read().clone();
        let cached = cache_dir.as_ref().and_then(|directory| {
            crate::core::filesystem::docker::mapping_cache::load_mapping(directory, image, &self.id)
        });
        let ready = Arc::new(match cached {
            Some(cached) => cached,
            None => crate::core::filesystem::docker::host_docker_overlay::compose(image, self)?,
        });
        *merged = Some(Arc::clone(&ready));
        Ok(ready)
    }
}

pub(crate) fn catalog(image: &Arc<EvidenceSession>) -> Option<&Arc<HostDocker>> {
    image
        .host_docker
        .get_or_init(|| match discover(image) {
            Ok(catalog) => catalog.map(Arc::new),
            Err(error) => {
                tracing::warn!(
                    "Docker runtime discovery failed for {}: {error:#}",
                    image.id
                );
                None
            }
        })
        .as_ref()
}

fn discover(image: &Arc<EvidenceSession>) -> Result<Option<HostDocker>> {
    let mut containers = Vec::new();
    for source in ext4_sources(image) {
        let volume = match open_ext4_source(image, source) {
            Ok(volume) => volume,
            Err(error) => {
                tracing::debug!("skip Docker probe on unreadable Ext4 volume: {error:#}");
                continue;
            }
        };
        let Some(docker_root) = find_node(&volume, EXT4_ROOT_INODE, &["var", "lib", "docker"])?
        else {
            continue;
        };
        let Some(docker_root_inode) = docker_root.inode else {
            continue;
        };
        containers.extend(discover_volume_containers(
            &volume,
            source,
            docker_root_inode,
        )?);
    }
    if containers.is_empty() {
        return Ok(None);
    }
    let mut seen_container_ids = HashSet::new();
    containers.retain(|container| seen_container_ids.insert(container.id.clone()));
    containers.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.id.cmp(&right.id))
    });
    let container_indexes = containers
        .iter()
        .enumerate()
        .map(|(index, container)| (container.id.clone(), index))
        .collect();
    Ok(Some(HostDocker {
        containers,
        container_indexes,
    }))
}

fn ext4_sources(image: &Arc<EvidenceSession>) -> Vec<Ext4Source> {
    let mut sources = Vec::new();
    for partition in &image.partitions {
        match partition.kind {
            PartitionKind::Ext4 => sources.push(Ext4Source::Partition(partition.id)),
            PartitionKind::Lvm => match LvmVolume::open(image, partition) {
                Ok(lvm) => {
                    for (lv_index, logical_volume) in lvm.logical_volumes.iter().enumerate() {
                        if logical_volume.kind == PartitionKind::Ext4 {
                            sources.push(Ext4Source::Lvm {
                                part_id: partition.id,
                                lv_index,
                            });
                        }
                    }
                }
                Err(error) => tracing::debug!(
                    "skip Docker probe in unreadable LVM partition {}: {error:#}",
                    partition.id
                ),
            },
            _ => {}
        }
    }
    sources
}

fn discover_volume_containers(
    volume: &Ext4Volume,
    source: Ext4Source,
    docker_root_inode: u64,
) -> Result<Vec<Arc<HostDockerContainer>>> {
    let Some(containers_dir) = find_node(volume, docker_root_inode, &["containers"])? else {
        return Ok(Vec::new());
    };
    let Some(containers_inode) = containers_dir.inode else {
        return Ok(Vec::new());
    };
    let mut containers = Vec::new();
    for directory in volume.children(containers_inode, false)? {
        if directory.deleted || directory.kind != NodeKind::Dir {
            continue;
        }
        match discover_container(volume, source, docker_root_inode, &directory) {
            Ok(Some(container)) => containers.push(Arc::new(container)),
            Ok(None) => {}
            Err(error) => tracing::warn!(
                "skip malformed Docker container metadata {}: {error:#}",
                directory.name
            ),
        }
    }
    Ok(containers)
}

fn discover_container(
    volume: &Ext4Volume,
    source: Ext4Source,
    docker_root_inode: u64,
    directory: &FsNode,
) -> Result<Option<HostDockerContainer>> {
    let container_id = directory.name.clone();
    let Some(container_dir_inode) = directory.inode else {
        return Ok(None);
    };
    let Some(config_node) = find_node(volume, container_dir_inode, &["config.v2.json"])? else {
        return Ok(None);
    };
    let config_text = read_text(volume, &config_node, METADATA_LIMIT)
        .with_context(|| format!("read {container_id}/config.v2.json"))?;
    let config: Value = serde_json::from_str(&config_text)
        .with_context(|| format!("parse {container_id}/config.v2.json"))?;
    let name = config
        .get("Name")
        .and_then(Value::as_str)
        .map(|name| name.trim_start_matches('/').to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| container_id.chars().take(12).collect());
    let image_reference = config
        .pointer("/Config/Image")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let created = config
        .get("Created")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let mount_path = [
        "image",
        "overlay2",
        "layerdb",
        "mounts",
        container_id.as_str(),
        "mount-id",
    ];
    let Some(mount_node) = find_node(volume, docker_root_inode, &mount_path)? else {
        // Containers using another graph driver are intentionally not exposed
        // as an inaccurate OverlayFS view.
        return Ok(None);
    };
    let mount_id = read_text(volume, &mount_node, 4096)?.trim().to_string();
    if mount_id.is_empty() {
        return Ok(None);
    }
    let init_path = [
        "image",
        "overlay2",
        "layerdb",
        "mounts",
        container_id.as_str(),
        "init-id",
    ];
    let init_id = find_node(volume, docker_root_inode, &init_path)?
        .map(|node| read_text(volume, &node, 4096))
        .transpose()?
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let lower_path = ["overlay2", mount_id.as_str(), "lower"];
    let lower_references = find_node(volume, docker_root_inode, &lower_path)?
        .map(|node| read_text(volume, &node, METADATA_LIMIT))
        .transpose()?
        .unwrap_or_default()
        .trim()
        .split(':')
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect();

    let mut runtime_files = Vec::new();
    for name in ["hostname", "hosts", "resolv.conf"] {
        if let Some(node) = find_node(volume, container_dir_inode, &[name])? {
            runtime_files.push((format!("/etc/{name}"), node));
        }
    }

    Ok(Some(HostDockerContainer {
        id: container_id,
        name,
        image_reference,
        created,
        source,
        docker_root_inode,
        mount_id,
        init_id,
        lower_references,
        runtime_files,
        merged: parking_lot::Mutex::new(None),
        index_lock: parking_lot::Mutex::new(()),
    }))
}

fn container_root_node(container: &HostDockerContainer) -> FsNode {
    let name = match container.image_reference.as_deref() {
        Some(image) => format!("{}_{image}", container.name),
        None => container.name.clone(),
    };
    FsNode {
        id: container_node_id(&container.id, ""),
        name: crate::core::common::navigation_name::volume_name(&name),
        parent_id: Some(GROUP_ID.to_string()),
        kind: NodeKind::Dir,
        size: 0,
        deleted: false,
        inode: None,
        timestamps: Mactb {
            created: container.created.clone(),
            birth: container.created.clone(),
            ..Mactb::default()
        },
        link_target: None,
        has_snapshot: false,
    }
}

pub(crate) fn find_node(
    volume: &Ext4Volume,
    start_inode: u64,
    components: &[&str],
) -> Result<Option<FsNode>> {
    let mut parent_inode = start_inode;
    for (index, component) in components.iter().enumerate() {
        let node = volume
            .children(parent_inode, false)?
            .into_iter()
            .find(|node| !node.deleted && node.name == *component);
        let Some(node) = node else {
            return Ok(None);
        };
        if index + 1 == components.len() {
            return Ok(Some(node));
        }
        if node.kind != NodeKind::Dir {
            return Ok(None);
        }
        let Some(inode) = node.inode else {
            return Ok(None);
        };
        parent_inode = inode;
    }
    Ok(None)
}

fn read_text(volume: &Ext4Volume, node: &FsNode, max_bytes: usize) -> Result<String> {
    let inode_number = node
        .inode
        .ok_or_else(|| anyhow!("Ext4 node has no inode: {}", node.name))?;
    let inode = volume.read_inode(inode_number)?;
    let bytes = volume.read_inode_data(&inode, max_bytes)?;
    String::from_utf8(bytes).context("Docker metadata is not UTF-8")
}

#[cfg(test)]
#[path = "host_docker_persistence_tests.rs"]
mod persistence_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_ids_roundtrip_group_root_and_path() {
        assert_eq!(
            parse_host_docker_id(GROUP_ID),
            Some(HostDockerNodeId::Group)
        );
        let root = container_node_id("abc", "");
        assert_eq!(
            parse_host_docker_id(&root),
            Some(HostDockerNodeId::Container {
                container_id: "abc".to_string(),
                path: String::new(),
            })
        );
        let path = container_node_id("abc", "/etc/passwd");
        assert_eq!(
            parse_host_docker_id(&path),
            Some(HostDockerNodeId::Container {
                container_id: "abc".to_string(),
                path: "/etc/passwd".to_string(),
            })
        );
    }

    #[test]
    fn detects_real_runtime_when_env_path_is_set() {
        let Some(path) = std::env::var_os("LOVELYCARVE_DOCKER_VMDK_TEST") else {
            return;
        };
        let image = Arc::new(EvidenceSession::open(&path).expect("open Docker VMDK"));
        let root = root_node(&image).expect("Docker group");
        assert_eq!(
            mount_root_node(&image, &HostDockerNodeId::Group)
                .expect("resolve Docker group mount root")
                .expect("Docker group mount root")
                .id,
            root.id
        );
        let containers =
            children(&image, &parse_host_docker_id(&root.id).unwrap()).expect("container roots");
        assert!(!containers.is_empty());
        let container_id = parse_host_docker_id(&containers[0].id).unwrap();
        assert_eq!(
            mount_root_node(&image, &container_id)
                .expect("resolve container mount root")
                .expect("container mount root")
                .id,
            containers[0].id
        );
        let case_dir = tempfile::tempdir().expect("temporary case");
        let mut case = crate::core::casework::case::test_support::case_with_evidence(
            case_dir.path(),
            &image.id,
        );
        let target = crate::core::catalog::host_docker_index::CaseTarget::of(
            &case,
            &image.id,
            &containers[0].id,
        );
        let prepared = crate::core::catalog::host_docker_index::prepare(
            &image,
            &containers[0].id,
            Some(&target),
            crate::core::catalog::host_docker_index::PrepareMode::Build,
            "2026-09-19T00:00:00.000Z".into(),
        )
        .expect("persist selected container");
        let staged = prepared.staged.expect("staged container volume");
        assert!(prepared.ready);
        assert!(
            crate::core::catalog::host_docker_index::publish(&mut case, staged).expect("publish")
        );
        let root_entries = children(&image, &container_id).expect("merged container root");
        assert!(root_entries.iter().any(|node| node.name == "etc"));

        let mut pending = root_entries;
        let mut visited = 0usize;
        let mut readable_file = None;
        let mut node_ids = std::collections::HashSet::new();
        while let Some(node) = pending.pop() {
            visited += 1;
            assert!(
                visited <= 100_000,
                "container traversal exceeded safety cap"
            );
            assert!(
                node_ids.insert(node.id.clone()),
                "duplicate virtual node id"
            );
            if node.kind == NodeKind::Dir {
                let id = parse_host_docker_id(&node.id).expect("virtual directory id");
                pending.extend(children(&image, &id).expect("virtual directory children"));
            } else if readable_file.is_none() && node.kind == NodeKind::File && node.size > 0 {
                readable_file = Some(node);
            }
        }
        assert!(visited > 100, "container view is unexpectedly small");
        if containers[0].name.starts_with("bloodhound-build") {
            assert_eq!(visited, 37_039, "known Kali fixture entry count changed");
        }
        let readable_file = readable_file.expect("container has a non-empty regular file");
        let id = parse_host_docker_id(&readable_file.id).expect("virtual file id");
        assert!(!read_file(&image, &id, 4096)
            .expect("read virtual container file")
            .is_empty());
        assert_eq!(
            file_stat(&image, &id, &readable_file.id)
                .expect("stat virtual container file")
                .node_id,
            readable_file.id
        );
        let expected_bytes = read_file(&image, &id, 4096).expect("warm preview");
        let reopened =
            Arc::new(EvidenceSession::open_with_id(image.id.clone(), &path).expect("reopen image"));
        let target = crate::core::catalog::host_docker_index::CaseTarget::of(
            &case,
            &reopened.id,
            &containers[0].id,
        );
        let cached = crate::core::catalog::host_docker_index::prepare(
            &reopened,
            &containers[0].id,
            Some(&target),
            crate::core::catalog::host_docker_index::PrepareMode::Status,
            "unused".into(),
        )
        .expect("reuse persisted index");
        assert!(cached.ready && cached.persisted && cached.staged.is_none());
        assert_eq!(cached.entry_count, visited as u64);
        assert_eq!(
            read_file(&reopened, &id, 4096).expect("cold preview"),
            expected_bytes
        );
    }
}
