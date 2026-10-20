//! Shell navigation and bounded walks over the shared index-first directory pages.
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

/// A node a walk found, with its absolute image path.
#[derive(Debug, Serialize)]
pub struct Entry {
    pub path: String,
    pub node: FsNode,
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

pub(super) async fn resolve<F, Fut, C, CFut>(
    current: &[(String, String)],
    path: &str,
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
    for component in path.split('/') {
        match component {
            "" | "." => continue,
            ".." => {
                location.pop();
                continue;
            }
            _ => {}
        }
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
                if node.name == component {
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
        let child = child.ok_or_else(|| {
            CliError::invalid(format!(
                "找不到路径分量：{component}；使用 files list 查看名称"
            ))
        })?;
        location.push((child.name, child.id));
    }
    Ok(location)
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
