//! Shell navigation, path resolution and bounded walks over the shared
//! index-first directory pages.
use std::{
    collections::{HashSet, VecDeque},
    future::Future,
};

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::CliError;
use crate::{
    commands::filesystem::filesystem_page::FsNodePage,
    core::filesystem::tree::fs_tree::{FsNode, NodeKind},
};

const PAGE_SIZE: usize = 500;

/// Symbolic links one path may follow, as Linux bounds them (ELOOP).
const MAX_LINKS: usize = 40;

/// Whether the last component of a path, when it is a symbolic link, names
/// the link or what it points to. Links before the last component are always
/// followed: such a path has no other reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LastLink {
    /// The link itself, as `files get` reports it with its `linkTarget`.
    Keep,
    /// What the link points to, as reading, hashing, exporting or listing needs.
    Follow,
}

/// A node a walk found, with its absolute image path.
#[derive(Debug, Serialize)]
pub struct Entry {
    pub path: String,
    pub node: FsNode,
}

/// `/` followed by the names of `location`.
pub(super) fn display_path(location: &[(String, String)]) -> String {
    format!(
        "/{}",
        location
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join("/")
    )
}

fn child_path(location: &[(String, String)], name: &str) -> String {
    match display_path(location).as_str() {
        "/" => format!("/{name}"),
        parent => format!("{parent}/{name}"),
    }
}

/// The root directory of a volume: a partition, a logical volume, an APFS
/// volume, a btrfs subvolume or a container's file system.
fn volume_root(id: &str) -> bool {
    let number = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    match id.split(':').collect::<Vec<_>>().as_slice() {
        ["part", partition] => number(partition),
        ["lvm", partition, volume] => number(partition) && number(volume),
        ["apfs", "part", partition, volume, ""] => number(partition) && number(volume),
        // The root directory of every btrfs subvolume is inode 256.
        ["btrfs", "part", _, _, "256"] | ["btrfs", "lvm", _, _, _, "256"] => true,
        ["host-docker", container, ""] => !container.is_empty(),
        ["docker", ""] => true,
        _ => false,
    }
}

/// How many leading entries of `location` an absolute link target keeps: the
/// way down to the volume that holds the link, or for folder and archive
/// evidence to the source's own root. `..` in a target stops there too.
fn volume_depth(location: &[(String, String)]) -> usize {
    location
        .iter()
        .rposition(|(_, id)| volume_root(id))
        .map_or(location.len().min(1), |index| index + 1)
}

/// NTFS and ReFS links may name a drive (`C:\…`), a device (`\??\…`) or a
/// share (`\\server\…`); which volume a drive letter meant is not known here.
fn windows_absolute(target: &str) -> bool {
    match target.as_bytes() {
        [b'\\', ..] => true,
        [drive, b':'] | [drive, b':', b'\\' | b'/', ..] => drive.is_ascii_alphabetic(),
        _ => false,
    }
}

pub(super) async fn node<F, Fut>(
    location: &[(String, String)],
    page: &mut F,
) -> Result<Option<FsNode>, CliError>
where
    F: FnMut(String, Option<String>, usize) -> Fut,
    Fut: Future<Output = Result<FsNodePage, CliError>>,
{
    let Some((_, target)) = location.last() else {
        return Ok(None);
    };
    let parent = location
        .iter()
        .rev()
        .nth(1)
        .map(|(_, id)| id.as_str())
        .unwrap_or("");
    let mut cursor = None;
    loop {
        let result = page(parent.to_owned(), cursor, PAGE_SIZE).await?;
        if let Some(node) = result.items.into_iter().find(|node| node.id == *target) {
            return Ok(Some(node));
        }
        cursor = result.next_cursor;
        if cursor.is_none() {
            return Err(CliError::invalid("目标节点已不在当前目录中"));
        }
    }
}

/// Resolve `path` from `current` to the names and IDs leading to its node.
/// Symbolic links are followed as Linux follows them, within the volume that
/// holds each link: an absolute target starts at that volume's root, and
/// `..` in a target never climbs above it.
pub(super) async fn resolve<F, Fut, C, CFut>(
    current: &[(String, String)],
    path: &str,
    last_link: LastLink,
    page: &mut F,
    mut case_insensitive: C,
) -> Result<Vec<(String, String)>, CliError>
where
    F: FnMut(String, Option<String>, usize) -> Fut,
    Fut: Future<Output = Result<FsNodePage, CliError>>,
    C: FnMut(String) -> CFut,
    CFut: Future<Output = Result<bool, CliError>>,
{
    let mut location = if path.starts_with('/') {
        Vec::new()
    } else {
        current.to_vec()
    };
    // Each component still to resolve, with the depth its `..` stops at: none
    // for the path as given, the holding volume's root for a link's target.
    let mut pending: VecDeque<(String, Option<usize>)> = path
        .split('/')
        .map(|component| (component.to_owned(), None))
        .collect();
    // The link last followed, named when its target cannot be resolved.
    let mut following = None;
    let mut links = 0;
    while let Some((component, floor)) = pending.pop_front() {
        match component.as_str() {
            "" | "." => continue,
            ".." => {
                if floor.is_none_or(|floor| location.len() > floor) {
                    location.pop();
                }
                continue;
            }
            _ => {}
        }
        let child = match child(&location, &component, page, &mut case_insensitive).await {
            Ok(child) => child,
            Err(mut error) => {
                if let Some(link) = &following {
                    error
                        .message
                        .push_str(&format!("（跟随符号链接 {link} 时）"));
                }
                return Err(error);
            }
        };
        let last = pending
            .iter()
            .all(|(component, _)| matches!(component.as_str(), "" | "."));
        if child.kind != NodeKind::Symlink || (last && last_link == LastLink::Keep) {
            location.push((child.name, child.id));
            continue;
        }
        let link = child_path(&location, &child.name);
        links += 1;
        if links > MAX_LINKS {
            return Err(CliError::invalid(format!(
                "解析 {path} 时跟随了超过 {MAX_LINKS} 个符号链接，链接可能成环"
            )));
        }
        let target = child
            .link_target
            .as_deref()
            .filter(|target| !target.is_empty())
            .ok_or_else(|| {
                CliError::invalid(format!(
                    "无法跟随符号链接 {link}：卷中没有读到链接目标；用 files get 查看该节点"
                ))
            })?;
        if windows_absolute(target) {
            return Err(CliError::invalid(format!(
                "不跟随指向 Windows 路径的符号链接 {link} → {target}；请在盘符对应的卷中定位目标"
            )));
        }
        let target = if child.id.starts_with("ntfs:") || child.id.starts_with("refs:") {
            target.replace('\\', "/")
        } else {
            target.to_owned()
        };
        let depth = volume_depth(&location);
        if target.starts_with('/') {
            location.truncate(depth);
        }
        for component in target.split('/').rev() {
            pending.push_front((component.to_owned(), Some(depth)));
        }
        following = Some(format!("{link} → {target}"));
    }
    Ok(location)
}

/// The entry named `component` in the directory `location` leads to: an
/// exact name (at the synthetic root also a root ID), else a unique
/// case-insensitive match on a volume that folds case.
async fn child<F, Fut, C, CFut>(
    location: &[(String, String)],
    component: &str,
    page: &mut F,
    case_insensitive: &mut C,
) -> Result<FsNode, CliError>
where
    F: FnMut(String, Option<String>, usize) -> Fut,
    Fut: Future<Output = Result<FsNodePage, CliError>>,
    C: FnMut(String) -> CFut,
    CFut: Future<Output = Result<bool, CliError>>,
{
    let parent = location.last().map(|(_, id)| id.as_str()).unwrap_or("");
    let mut cursor = None;
    let mut child: Option<FsNode> = None;
    let mut folded_child = None;
    let mut folded_ambiguous = false;
    let folded_component = component.to_lowercase();
    loop {
        let result = page(parent.to_owned(), cursor, PAGE_SIZE).await?;
        if result.truncated {
            return Err(CliError::invalid("目录列表不完整，无法可靠解析路径"));
        }
        for node in result.items {
            // Root IDs provide script-friendly volume paths without changing
            // evidence names. Match both in one pass so a name/ID collision
            // reports ambiguity instead of silently selecting another volume.
            if node.name == component || (location.is_empty() && node.id == component) {
                if child.is_some() {
                    return Err(CliError::invalid(
                        "同名节点不唯一，请改用节点 ID 指定（files list 输出的 items[].id）",
                    ));
                }
                child = Some(node);
            } else if node.name.to_lowercase() == folded_component {
                folded_ambiguous |= folded_child.is_some();
                folded_child = Some(node);
            }
        }
        cursor = result.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    if child.is_none() && folded_child.is_some() && case_insensitive(parent.to_owned()).await? {
        if folded_ambiguous {
            return Err(CliError::invalid(
                "忽略大小写后节点不唯一，请改用节点 ID 指定（files list 输出的 items[].id）",
            ));
        }
        child = folded_child;
    }
    child.ok_or_else(|| {
        let parent = display_path(location);
        CliError::new(
            "notFound",
            format!("{parent} 下找不到「{component}」；用 files list --path \"{parent}\" 查看名称"),
        )
    })
}

/// Locate an unindexed ID through directory pages, preserving duplicate names.
/// Partition-qualified IDs only search their own partition. Bounds and read
/// failures are reported, rather than returning a partial ancestor chain.
pub(super) async fn locate_id<F, Fut>(
    target: &str,
    cancellation: &CancellationToken,
    page: &mut F,
) -> Result<Vec<(String, String)>, CliError>
where
    F: FnMut(String, Option<String>, usize) -> Fut,
    Fut: Future<Output = Result<FsNodePage, CliError>>,
{
    if target.is_empty() || target == "root" {
        return Ok(Vec::new());
    }
    let parts = target.split(':').collect::<Vec<_>>();
    let partition = parts
        .windows(2)
        .find(|pair| pair[0] == "part")
        .map(|pair| format!("part:{}", pair[1]));
    let mut queue = VecDeque::from([Vec::<(String, String)>::new()]);
    let mut visited = HashSet::new();
    let mut examined = 0usize;
    let mut failure = None;
    while let Some(location) = queue.pop_front() {
        let parent = location.last().map_or("", |(_, id)| id.as_str());
        if !visited.insert(parent.to_owned()) {
            continue;
        }
        let mut cursor = None;
        loop {
            if cancellation.is_cancelled() {
                return Err(CliError::new("cancelled", "节点查找已取消"));
            }
            if examined >= 1_000_000 || location.len() >= 128 {
                return Err(CliError::invalid(
                    "按节点 ID 查找达到目录遍历上限；请等待卷索引完成后重试",
                ));
            }
            let result = match page(parent.to_owned(), cursor, PAGE_SIZE).await {
                Ok(result) => result,
                Err(error) => {
                    failure.get_or_insert(error);
                    break;
                }
            };
            if result.truncated {
                return Err(CliError::invalid("目录列表不完整，无法可靠定位节点 ID"));
            }
            for node in result.items {
                examined += 1;
                let found = node.id == target;
                if found || node.kind == NodeKind::Dir {
                    if !found
                        && location.is_empty()
                        && partition.as_ref().is_some_and(|id| *id != node.id)
                    {
                        continue;
                    }
                    let mut child = location.clone();
                    child.push((node.name, node.id));
                    if found {
                        return Ok(child);
                    }
                    queue.push_back(child);
                }
            }
            cursor = result.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
    }
    Err(failure.unwrap_or_else(|| CliError::invalid(format!("找不到节点 ID：{target}"))))
}

pub(super) async fn walk<F, Fut>(
    location: &[(String, String)],
    max_depth: u32,
    limit: usize,
    filter: Option<&str>,
    cancellation: &CancellationToken,
    page: &mut F,
) -> Result<(Vec<Entry>, bool), CliError>
where
    F: FnMut(String, Option<String>, usize) -> Fut,
    Fut: Future<Output = Result<FsNodePage, CliError>>,
{
    let mut items = Vec::new();
    let mut visited = HashSet::new();
    let root = location.last().map_or("", |(_, id)| id.as_str());
    let prefix = location
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join("/");
    let prefix = if prefix.is_empty() {
        String::new()
    } else {
        format!("/{prefix}")
    };
    let mut queue = vec![(root.to_owned(), prefix, 0u32)];
    let mut truncated = false;
    let mut examined = 0usize;
    while let Some((parent, prefix, depth)) = queue.pop() {
        if !visited.insert(parent.clone()) {
            continue;
        }
        let mut cursor = None;
        let mut directories = Vec::new();
        loop {
            if cancellation.is_cancelled() {
                return Err(CliError::invalid("目录查询已取消"));
            }
            if examined >= limit {
                return Ok((items, true));
            }
            let result = page(parent.clone(), cursor, PAGE_SIZE.min(limit - examined)).await?;
            truncated |= result.truncated;
            for node in result.items {
                examined += 1;
                let path = format!("{prefix}/{}", node.name);
                if node.kind == NodeKind::Dir {
                    if depth + 1 < max_depth {
                        directories.push((node.id.clone(), path.clone(), depth + 1));
                    } else if max_depth > 1 {
                        truncated = true;
                    }
                }
                if filter.is_none_or(|filter| node.name.to_lowercase().contains(filter)) {
                    items.push(Entry { path, node });
                }
            }
            cursor = result.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        queue.extend(directories.into_iter().rev());
    }
    Ok((items, truncated))
}

#[cfg(test)]
#[path = "browsing_tests.rs"]
mod tests;
