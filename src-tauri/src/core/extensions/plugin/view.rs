//! Host side of `view.*`: a run's read-only path tree over volumes of one
//! evidence item.
//!
//! A **system view** shows one detected operating system: its volume at the
//! root and the volumes it mounts (fstab, drive letters) at their mount
//! points. An **evidence view** shows every volume as a top-level directory
//! named by its volume key. Paths are POSIX, relative to the view root;
//! names match exactly first, then ASCII case-insensitively; a live entry
//! wins over a deleted one of the same name. Symbolic links are followed
//! by `stat` / `open`, with absolute targets resolved against the view root
//! — so `/etc/os-release` → `../usr/lib/os-release` and links into another
//! mounted volume work as on the running system. `..` is only honoured
//! inside link targets and never leaves the view.

use std::{collections::VecDeque, sync::Arc};

use lovelycarve_plugin::{
    glob::Glob,
    protocol::{MountInfo, NodeInfo, NodePage, SystemInfo, Timestamps, ViewInfo},
};
use parking_lot::Mutex;

use super::{directory_cache::DirectoryCache, PluginError, Result};
use crate::core::{
    analysis::identify::systems::DetectedSystem,
    evidence::partitions::PartitionKind,
    filesystem::{
        evidence_session::EvidenceSession,
        fs_all::{self, FsAll},
        tree::fs_tree::{FsNode, FsSession, Mactb, NodeKind},
    },
};

/// Symbolic links followed while resolving one path.
const MAX_LINK_HOPS: usize = 16;
/// Directories one `view.find` page walks.
const FIND_DIRECTORIES_PER_PAGE: usize = 64;
/// Paths one `view.find` page returns.
const FIND_PATHS_PER_PAGE: usize = 1024;
/// Nodes one find visits in total.
const FIND_MAX_VISITED: u64 = 4_000_000;
const MAX_PATH_BYTES: usize = 4096;
const MAX_DEPTH: usize = 128;

#[derive(Clone, Debug)]
struct Mount {
    path: Vec<String>,
    root: String,
    node: String,
    node_path: String,
    listed: bool,
}

/// A run's view.
pub struct HostView {
    info: ViewInfo,
    mounts: Vec<Mount>,
    image: Arc<EvidenceSession>,
    directories: Mutex<DirectoryCache>,
}

/// Where a resolved path lands.
#[derive(Clone, Debug)]
pub enum Located {
    /// A directory that exists only because mounts sit below it.
    Virtual,
    Node {
        node: Box<FsNode>,
        /// Volume key.
        root: String,
        /// Path inside the volume.
        root_path: String,
    },
}

/// A resolved path.
#[derive(Clone, Debug)]
pub struct Resolved {
    /// The path after following links.
    pub view_path: String,
    pub located: Located,
}

/// The file a path resolved to.
#[derive(Clone, Debug)]
pub struct LocatedFile {
    /// The path after following links.
    pub view_path: String,
    pub node: FsNode,
    /// Volume key.
    pub root: String,
    /// Path inside the volume.
    pub root_path: String,
}

impl HostView {
    /// The view of one detected system.
    pub fn system(view_id: String, system: &DetectedSystem, image: Arc<EvidenceSession>) -> Self {
        let mounts = system
            .mounts
            .iter()
            .map(|mount| Mount {
                path: components(&mount.path),
                root: mount.root.clone(),
                node: mount.node.clone(),
                node_path: mount.node_path.clone(),
                listed: mount.listed,
            })
            .collect();
        let info = ViewInfo {
            view_id,
            label: system.label.clone(),
            system: Some(SystemInfo {
                id: system.id.clone(),
                family: system.family.clone(),
                product: system.product.clone(),
                version: system.version.clone(),
                root: system.root.clone(),
            }),
            mounts: system
                .mounts
                .iter()
                .map(|mount| MountInfo {
                    path: mount.path.clone(),
                    root: mount.root.clone(),
                    source: mount.source.to_owned(),
                })
                .collect(),
        };
        Self::new(info, mounts, image)
    }

    /// Every volume of the evidence as a top-level directory. LVM containers
    /// are left out: their logical volumes are volumes of their own.
    pub fn evidence(view_id: String, label: String, image: Arc<EvidenceSession>) -> Self {
        let volumes = FsAll::for_image(&image);
        let mut mounts = Vec::new();
        let mut infos = Vec::new();
        for entry in volumes
            .entries()
            .iter()
            .filter(|entry| entry.kind != PartitionKind::Lvm)
        {
            let node = match fs_all::directory_root(&image, &entry.volume_key) {
                Ok(node) => node,
                Err(error) => {
                    tracing::debug!(volume = %entry.volume_key, "volume left out of the view: {error:#}");
                    continue;
                }
            };
            mounts.push(Mount {
                path: vec![entry.volume_key.clone()],
                root: entry.volume_key.clone(),
                node,
                node_path: String::new(),
                listed: true,
            });
            infos.push(MountInfo {
                path: entry.volume_key.clone(),
                root: entry.volume_key.clone(),
                source: "volume".to_owned(),
            });
        }
        let info = ViewInfo {
            view_id,
            label,
            system: None,
            mounts: infos,
        };
        Self::new(info, mounts, image)
    }

    fn new(info: ViewInfo, mounts: Vec<Mount>, image: Arc<EvidenceSession>) -> Self {
        Self {
            info,
            mounts,
            image,
            directories: Mutex::new(DirectoryCache::default()),
        }
    }

    pub fn info(&self) -> &ViewInfo {
        &self.info
    }

    pub fn id(&self) -> &str {
        &self.info.view_id
    }

    /// The evidence the view reads.
    pub fn image(&self) -> &Arc<EvidenceSession> {
        &self.image
    }

    /// Reads up to `length` bytes at `offset` of the file at `path`
    /// (links followed), clamped at the end of the file.
    pub fn read(&self, path: &str, offset: u64, length: usize) -> Result<Vec<u8>> {
        let LocatedFile { node, .. } = self
            .locate_file(path)?
            .ok_or_else(|| path_error("VIEW_PATH_NOT_FOUND", path, "does not exist"))?;
        if offset >= node.size || length == 0 {
            return Ok(Vec::new());
        }
        let length = length.min(usize::try_from(node.size - offset).unwrap_or(usize::MAX));
        let mut bytes = crate::core::filesystem::tree::fs_tree::read_file_range(
            &self.image,
            &node.id,
            offset,
            length,
        )
        .map_err(|error| PluginError::Rpc {
            code: "FILE_READ_FAILED".to_owned(),
            message: format!("{path}: {error:#}"),
        })?;
        bytes.truncate(length);
        Ok(bytes)
    }

    /// Resolves `path`, following links. `None` when nothing is there.
    pub fn resolve(&self, path: &str) -> Result<Option<Resolved>> {
        let mut queue: VecDeque<String> = checked_components(path)?.into();
        let mut stack: Vec<String> = Vec::new();
        let mut current = self.position(&stack)?;
        let mut hops = 0;
        while let Some(component) = queue.pop_front() {
            if component == ".." {
                stack.pop();
                current = self.position(&stack)?;
                continue;
            }
            stack.push(component.clone());
            if let Some(mount) = self.mount_at(&stack) {
                current = Some(self.mount_position(mount, &stack));
                continue;
            }
            // A name the volume below lacks still resolves, as a virtual
            // directory, when mounts sit below it (`srv` of `srv/home`).
            let Some(position) = current.take() else {
                if self.mounts_below(&stack) {
                    continue;
                }
                return Ok(None);
            };
            let found = if position.node.kind == NodeKind::Dir {
                pick(&self.children(&position.node.id)?, &component)
            } else {
                None
            };
            let Some(found) = found else {
                if self.mounts_below(&stack) {
                    continue;
                }
                return Ok(None);
            };
            if found.kind == NodeKind::Symlink && hops < MAX_LINK_HOPS {
                if let Some(target) = found
                    .link_target
                    .clone()
                    .filter(|target| !target.is_empty())
                {
                    hops += 1;
                    stack.pop();
                    if target.starts_with('/') {
                        stack.clear();
                    }
                    let mut parts: VecDeque<String> = target
                        .split('/')
                        .filter(|part| !part.is_empty() && *part != ".")
                        .map(str::to_owned)
                        .collect();
                    parts.extend(queue);
                    queue = parts;
                    current = self.position(&stack)?;
                    continue;
                }
            }
            current = Some(Position {
                root_path: join(&position.root_path, &found.name),
                root: position.root,
                node: found,
            });
        }
        let view_path = stack.join("/");
        Ok(Some(Resolved {
            view_path,
            located: match current {
                Some(position) => Located::Node {
                    node: Box::new(position.node),
                    root: position.root,
                    root_path: position.root_path,
                },
                None => Located::Virtual,
            },
        }))
    }

    pub fn stat(&self, path: &str) -> Result<Option<NodeInfo>> {
        Ok(self.resolve(path)?.map(|resolved| match resolved.located {
            Located::Virtual => directory_info(last_name(&resolved.view_path)),
            Located::Node { node, .. } => node_info(&node),
        }))
    }

    /// One page of a directory, mount points included.
    pub fn list(&self, path: &str, skip: u64, limit: u64) -> Result<NodePage> {
        let nodes = self.entries(path)?;
        let total = nodes.len() as u64;
        if skip > total {
            return Err(PluginError::Rpc {
                code: "INVALID_PARAMS".to_owned(),
                message: "directory cursor exceeds the entry count".to_owned(),
            });
        }
        Ok(NodePage {
            nodes: nodes
                .into_iter()
                .skip(usize::try_from(skip).unwrap_or(usize::MAX))
                .take(usize::try_from(limit).unwrap_or(usize::MAX))
                .collect(),
            total,
        })
    }

    /// Every entry of a directory: its own children, with the mount points
    /// directly below it as directories.
    fn entries(&self, path: &str) -> Result<Vec<NodeInfo>> {
        let resolved = self
            .resolve(path)?
            .ok_or_else(|| path_error("VIEW_PATH_NOT_FOUND", path, "does not exist"))?;
        let mut nodes = match &resolved.located {
            Located::Virtual => Vec::new(),
            Located::Node { node, .. } if node.kind == NodeKind::Dir => {
                self.children(&node.id)?.iter().map(node_info).collect()
            }
            Located::Node { .. } => {
                return Err(path_error(
                    "VIEW_NOT_A_DIRECTORY",
                    path,
                    "is not a directory",
                ))
            }
        };
        let parent = components(&resolved.view_path);
        for mount in self.mounts.iter().filter(|mount| mount.listed) {
            if mount.path.len() != parent.len() + 1
                || !mount
                    .path
                    .iter()
                    .zip(&parent)
                    .all(|(left, right)| left.eq_ignore_ascii_case(right))
            {
                continue;
            }
            let name = &mount.path[parent.len()];
            nodes.retain(|node| !node.name.eq_ignore_ascii_case(name));
            nodes.push(directory_info(name));
        }
        // Hidden ancestors of deeper mounts (`mnt` of `mnt/data`).
        for mount in self.mounts.iter().filter(|mount| mount.listed) {
            if mount.path.len() > parent.len() + 1
                && mount
                    .path
                    .iter()
                    .zip(&parent)
                    .all(|(left, right)| left.eq_ignore_ascii_case(right))
            {
                let name = &mount.path[parent.len()];
                if !nodes
                    .iter()
                    .any(|node| node.name.eq_ignore_ascii_case(name))
                {
                    nodes.push(directory_info(name));
                }
            }
        }
        if matches!(resolved.located, Located::Virtual) {
            nodes.sort_by(|left, right| left.name.cmp(&right.name));
        }
        Ok(nodes)
    }

    /// The fs-tree node behind a resolved file path (for `view.open`).
    pub fn locate_file(&self, path: &str) -> Result<Option<LocatedFile>> {
        let Some(resolved) = self.resolve(path)? else {
            return Ok(None);
        };
        match resolved.located {
            Located::Node {
                node,
                root,
                root_path,
            } if node.kind == NodeKind::File => Ok(Some(LocatedFile {
                view_path: resolved.view_path,
                node: *node,
                root,
                root_path,
            })),
            Located::Node { .. } | Located::Virtual => {
                Err(path_error("VIEW_NOT_A_FILE", path, "is not a file"))
            }
        }
    }

    /// Continues a find (see [`FindScan`]).
    pub fn find_page(&self, scan: &mut FindScan) -> Result<Vec<String>> {
        let mut paths = Vec::new();
        let mut walked = 0;
        while walked < FIND_DIRECTORIES_PER_PAGE && paths.len() < FIND_PATHS_PER_PAGE {
            let Some((directory, depth)) = scan.pending.pop() else {
                break;
            };
            walked += 1;
            let entries = match self.entries(&directory) {
                Ok(entries) => entries,
                // A directory that vanished or cannot be listed is skipped;
                // the walk reports what it could read.
                Err(PluginError::Rpc { code, .. }) if code.starts_with("VIEW_") => continue,
                Err(error) => return Err(error),
            };
            for entry in entries {
                scan.visited += 1;
                if scan.visited > FIND_MAX_VISITED {
                    return Err(PluginError::ResourceLimit(format!(
                        "view.find visits at most {FIND_MAX_VISITED} entries"
                    )));
                }
                if entry.deleted {
                    continue;
                }
                let path = if directory.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{directory}/{}", entry.name)
                };
                if path.len() > MAX_PATH_BYTES {
                    continue;
                }
                match entry.kind.as_str() {
                    "dir"
                        if depth < MAX_DEPTH
                            && scan.globs.iter().any(|glob| glob.may_match_below(&path)) =>
                    {
                        scan.pending.push((path, depth + 1));
                    }
                    "file" if scan.globs.iter().any(|glob| glob.matches(&path)) => {
                        paths.push(path);
                    }
                    _ => {}
                }
            }
        }
        Ok(paths)
    }

    fn mount_at(&self, stack: &[String]) -> Option<&Mount> {
        self.mounts.iter().find(|mount| {
            mount.path.len() == stack.len()
                && mount
                    .path
                    .iter()
                    .zip(stack)
                    .all(|(left, right)| left.eq_ignore_ascii_case(right))
        })
    }

    fn mounts_below(&self, stack: &[String]) -> bool {
        self.mounts.iter().any(|mount| {
            mount.path.len() > stack.len()
                && mount
                    .path
                    .iter()
                    .zip(stack)
                    .all(|(left, right)| left.eq_ignore_ascii_case(right))
        })
    }

    fn mount_position(&self, mount: &Mount, stack: &[String]) -> Position {
        Position {
            node: FsNode {
                id: mount.node.clone(),
                name: stack.last().cloned().unwrap_or_default(),
                parent_id: None,
                kind: NodeKind::Dir,
                size: 0,
                deleted: false,
                inode: None,
                timestamps: Mactb::default(),
                link_target: None,
                has_snapshot: false,
            },
            root: mount.root.clone(),
            root_path: mount.node_path.clone(),
        }
    }

    /// The position of an already-resolved path (no links), `None` for a
    /// virtual directory.
    fn position(&self, stack: &[String]) -> Result<Option<Position>> {
        let deepest = (0..=stack.len())
            .rev()
            .find_map(|depth| self.mount_at(&stack[..depth]).map(|mount| (depth, mount)));
        let Some((depth, mount)) = deepest else {
            return Ok(None);
        };
        let mut position = self.mount_position(mount, &stack[..depth]);
        for component in &stack[depth..] {
            if position.node.kind != NodeKind::Dir {
                return Ok(None);
            }
            let children = self.children(&position.node.id)?;
            let Some(found) = pick(&children, component) else {
                return Ok(None);
            };
            position = Position {
                root_path: join(&position.root_path, &found.name),
                root: position.root,
                node: found,
            };
        }
        Ok(Some(position))
    }

    fn children(&self, node_id: &str) -> Result<Arc<Vec<FsNode>>> {
        if let Some(nodes) = self.directories.lock().get(node_id) {
            return Ok(nodes);
        }
        let nodes = Arc::new(
            FsSession::new(&self.image)
                .children(node_id)
                .map_err(|error| {
                    tracing::debug!(%node_id, "plugin view listing failed: {error:#}");
                    PluginError::Rpc {
                        code: "VIEW_LIST_FAILED".to_owned(),
                        message: "the directory could not be listed".to_owned(),
                    }
                })?,
        );
        self.directories
            .lock()
            .insert(node_id.to_owned(), Arc::clone(&nodes));
        Ok(nodes)
    }
}

/// A view position: a node of a volume and its path inside that volume.
#[derive(Clone, Debug)]
struct Position {
    node: FsNode,
    root: String,
    root_path: String,
}

/// A find in progress: the directories still to walk.
pub struct FindScan {
    globs: Vec<Glob>,
    pending: Vec<(String, usize)>,
    pub visited: u64,
}

impl FindScan {
    pub fn new(patterns: &[String]) -> Result<Self> {
        if patterns.is_empty() || patterns.len() > 64 {
            return Err(PluginError::Rpc {
                code: "INVALID_PARAMS".to_owned(),
                message: "view.find takes 1..=64 patterns".to_owned(),
            });
        }
        let globs = patterns
            .iter()
            .map(|pattern| {
                Glob::new(pattern).map_err(|message| PluginError::Rpc {
                    code: "INVALID_PARAMS".to_owned(),
                    message: format!("{pattern:?}: {message}"),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            globs,
            pending: vec![(String::new(), 0)],
            visited: 0,
        })
    }

    pub fn is_complete(&self) -> bool {
        self.pending.is_empty()
    }
}

/// Exact name first, then ASCII case-insensitive; live entries before
/// deleted ones.
fn pick(children: &[FsNode], name: &str) -> Option<FsNode> {
    let candidates = |exact: bool| {
        children.iter().filter(move |node| {
            if exact {
                node.name == name
            } else {
                node.name.eq_ignore_ascii_case(name)
            }
        })
    };
    [true, false].into_iter().find_map(|exact| {
        candidates(exact)
            .find(|node| !node.deleted)
            .or_else(|| candidates(exact).next())
            .cloned()
    })
}

fn checked_components(path: &str) -> Result<Vec<String>> {
    if path.len() > MAX_PATH_BYTES || path.contains('\\') || path.contains('\0') {
        return Err(path_error("VIEW_PATH_INVALID", path, "is not a view path"));
    }
    let parts = components(path);
    if parts.iter().any(|part| part == "..") {
        return Err(path_error(
            "VIEW_PATH_INVALID",
            path,
            "must stay inside the view",
        ));
    }
    Ok(parts)
}

fn components(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(str::to_owned)
        .collect()
}

fn join(base: &str, name: &str) -> String {
    if base.is_empty() {
        name.to_owned()
    } else {
        format!("{base}/{name}")
    }
}

fn last_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or_default()
}

fn path_error(code: &str, path: &str, problem: &str) -> PluginError {
    PluginError::Rpc {
        code: code.to_owned(),
        message: format!("{path} {problem}"),
    }
}

fn directory_info(name: &str) -> NodeInfo {
    NodeInfo {
        name: name.to_owned(),
        kind: "dir".to_owned(),
        size: 0,
        deleted: false,
        link_target: None,
        timestamps: Timestamps::default(),
    }
}

pub(crate) fn node_info(node: &FsNode) -> NodeInfo {
    NodeInfo {
        name: node.name.clone(),
        kind: match node.kind {
            NodeKind::Dir => "dir",
            NodeKind::File => "file",
            NodeKind::Symlink => "link",
            NodeKind::Unallocated => "other",
        }
        .to_owned(),
        size: node.size,
        deleted: node.deleted,
        link_target: node.link_target.clone(),
        timestamps: timestamps(&node.timestamps),
    }
}

pub(crate) fn timestamps(times: &Mactb) -> Timestamps {
    Timestamps {
        modified: times.modified.clone(),
        accessed: times.accessed.clone(),
        created: times.created.clone(),
        changed: times.mft_changed.clone(),
        birth: times.birth.clone(),
    }
}

#[cfg(test)]
mod tests;
