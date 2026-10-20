//! Shell navigation and bounded walks over the shared index-first directory pages.
use std::{collections::HashSet, future::Future};

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::CliError;
use crate::{
    commands::filesystem::filesystem_page::FsNodePage,
    core::filesystem::tree::fs_tree::{FsNode, NodeKind},
};

const PAGE_SIZE: usize = 500;

/// A node a walk found, with its path below the walk's root.
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

pub(super) async fn resolve<F, Fut>(
    current: &[(String, String)],
    path: &str,
    page: &mut F,
) -> Result<Vec<(String, String)>, CliError>
where
    F: FnMut(String, Option<String>, usize) -> Fut,
    Fut: Future<Output = Result<FsNodePage, CliError>>,
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
        loop {
            let result = page(parent.to_owned(), cursor, PAGE_SIZE).await?;
            for node in result
                .items
                .into_iter()
                .filter(|node| node.name == component)
            {
                if child.is_some() {
                    return Err(CliError::invalid(
                        "同名节点不唯一，请改用节点 ID 指定（files list 输出的 items[].id）",
                    ));
                }
                child = Some(node);
            }
            cursor = result.next_cursor;
            if cursor.is_none() {
                break;
            }
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

pub(super) async fn walk<F, Fut>(
    root: &str,
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
    let mut queue = vec![(root.to_owned(), String::new(), 0u32)];
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
mod tests {
    use super::*;
    use crate::{
        commands::filesystem::filesystem_page::fs_node_page_from_filedb,
        core::catalog::filedb::{
            filedb_query,
            filedb_store::test_support::{build_test_index, entry},
        },
    };

    #[tokio::test]
    async fn index_pages_cover_large_directories_and_preserve_walk_limits() {
        let directory = tempfile::tempdir().unwrap();
        let mut entries = (0..502)
            .map(|number| {
                entry(
                    "part:0",
                    &format!("file-{number}"),
                    &format!("file{number}.txt"),
                    NodeKind::File,
                    &format!("/file{number}.txt"),
                )
            })
            .collect::<Vec<_>>();
        entries.push(entry(
            "part:0",
            "nested",
            "nested",
            NodeKind::Dir,
            "/nested",
        ));
        entries.push(entry(
            "nested",
            "report",
            "REPORT.txt",
            NodeKind::File,
            "/nested/REPORT.txt",
        ));
        let index = build_test_index(directory.path(), "part:0", &entries);
        let mut page = |parent: String, cursor: Option<String>, limit| {
            let result =
                filedb_query::children_page(&index, &parent, cursor.as_deref(), Some(limit))
                    .unwrap()
                    .unwrap();
            std::future::ready(Ok(fs_node_page_from_filedb(result)))
        };
        let current = vec![("volume".into(), "part:0".into())];
        assert_eq!(
            node(
                &resolve(&current, "file501.txt", &mut page).await.unwrap(),
                &mut page
            )
            .await
            .unwrap()
            .unwrap()
            .name,
            "file501.txt"
        );
        let cancellation = CancellationToken::new();
        assert_eq!(
            resolve(&current, "nested/../file501.txt", &mut page)
                .await
                .unwrap(),
            vec![
                ("volume".into(), "part:0".into()),
                ("file501.txt".into(), "file-501".into())
            ]
        );
        let (items, truncated) = walk(
            "part:0",
            32,
            10000,
            Some("report"),
            &cancellation,
            &mut page,
        )
        .await
        .unwrap();
        assert_eq!(
            (items.len(), items[0].path.as_str(), truncated),
            (1, "/nested/REPORT.txt", false)
        );
        let (items, truncated) = walk("part:0", 1, 503, None, &cancellation, &mut page)
            .await
            .unwrap();
        assert_eq!((items.len(), truncated), (503, false));
        let (items, truncated) = walk("part:0", 1, 501, None, &cancellation, &mut page)
            .await
            .unwrap();
        assert_eq!((items.len(), truncated), (501, true));
        assert!(resolve(&current, "missing", &mut page).await.is_err());
        cancellation.cancel();
        assert!(walk("part:0", 32, 10000, None, &cancellation, &mut page)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn duplicate_names_on_separate_index_pages_remain_ambiguous() {
        let directory = tempfile::tempdir().unwrap();
        let mut entries = (0..499)
            .map(|number| {
                entry(
                    "part:0",
                    &format!("id-{number:03}"),
                    &format!("file{number}.txt"),
                    NodeKind::File,
                    &format!("/file{number}.txt"),
                )
            })
            .collect::<Vec<_>>();
        entries.push(entry(
            "part:0",
            "duplicate-1",
            "z-duplicate",
            NodeKind::File,
            "/z-duplicate",
        ));
        entries.push(entry(
            "part:0",
            "duplicate-2",
            "z-duplicate",
            NodeKind::File,
            "/z-duplicate",
        ));
        let index = build_test_index(directory.path(), "part:0", &entries);
        let mut page = |parent: String, cursor: Option<String>, limit| {
            std::future::ready(Ok(fs_node_page_from_filedb(
                filedb_query::children_page(&index, &parent, cursor.as_deref(), Some(limit))
                    .unwrap()
                    .unwrap(),
            )))
        };
        let error = resolve(
            &[("volume".into(), "part:0".into())],
            "z-duplicate",
            &mut page,
        )
        .await
        .unwrap_err();
        assert!(error.message.contains("同名节点不唯一"));
    }
}
