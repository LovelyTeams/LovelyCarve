//! Shared filesystem-node pagination helpers.
//!
//! Directory pages and snapshot-index pages expose
//! the same stable `(name, id)` cursor model through this module.

use ::hex::{decode as hex_decode, encode as hex_encode};
use serde::{Deserialize, Serialize};

use crate::app::error::AppError;
use crate::core::catalog::filedb::filedb_query;
use crate::core::filesystem::tree::fs_tree::FsNode;

const FS_NODE_PAGE_DEFAULT_SIZE: usize = 500;
const FS_NODE_PAGE_MAX_SIZE: usize = 5_000;

pub(crate) fn normalize_fs_node_page_size(page_size: Option<usize>) -> usize {
    page_size
        .unwrap_or(FS_NODE_PAGE_DEFAULT_SIZE)
        .clamp(1, FS_NODE_PAGE_MAX_SIZE)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsNodePage {
    pub items: Vec<FsNode>,
    pub next_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<usize>,
    pub truncated: bool,
}

pub fn fs_node_page_from_filedb(page: filedb_query::NodePage) -> FsNodePage {
    FsNodePage {
        items: page.items,
        next_cursor: page.next_cursor,
        total: page.total,
        truncated: page.truncated,
    }
}

pub fn fs_node_page_from_sorted_nodes(
    mut nodes: Vec<FsNode>,
    cursor: Option<&str>,
    page_size: Option<usize>,
) -> Result<FsNodePage, AppError> {
    sort_fs_nodes_for_cursor(&mut nodes);
    let page_size = normalize_fs_node_page_size(page_size);
    let start = match cursor {
        Some(cursor) => {
            let (cursor_name, cursor_id) = decode_fs_node_cursor(cursor)?;
            nodes
                .iter()
                .position(|node| {
                    node.name.as_str() > cursor_name.as_str()
                        || (node.name.as_str() == cursor_name.as_str()
                            && node.id.as_str() > cursor_id.as_str())
                })
                .unwrap_or(nodes.len())
        }
        None => 0,
    };
    let total = nodes.len();
    let end = start.saturating_add(page_size).min(total);
    let items = nodes[start..end].to_vec();
    let next_cursor = if end < total {
        items.last().map(fs_node_cursor_for_node)
    } else {
        None
    };
    Ok(FsNodePage {
        items,
        next_cursor,
        total: Some(total),
        truncated: false,
    })
}

fn sort_fs_nodes_for_cursor(nodes: &mut [FsNode]) {
    nodes.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn fs_node_cursor_for_node(node: &FsNode) -> String {
    format!(
        "{}:{}",
        hex_encode(node.name.as_bytes()),
        hex_encode(node.id.as_bytes())
    )
}

fn decode_fs_node_cursor(cursor: &str) -> Result<(String, String), AppError> {
    let (name_hex, id_hex) = cursor
        .split_once(':')
        .ok_or_else(|| AppError::invalid_input("invalid FsNode page cursor"))?;
    let name = String::from_utf8(hex_decode(name_hex).map_err(|err| {
        AppError::invalid_input(format!("invalid FsNode page cursor name: {err}"))
    })?)
    .map_err(|err| {
        AppError::invalid_input(format!("invalid FsNode page cursor name utf8: {err}"))
    })?;
    let id =
        String::from_utf8(hex_decode(id_hex).map_err(|err| {
            AppError::invalid_input(format!("invalid FsNode page cursor id: {err}"))
        })?)
        .map_err(|err| {
            AppError::invalid_input(format!("invalid FsNode page cursor id utf8: {err}"))
        })?;
    Ok((name, id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::filesystem::tree::fs_tree::{Mactb, NodeKind};

    fn test_node(name: &str, id: &str) -> FsNode {
        FsNode {
            id: id.to_string(),
            name: name.to_string(),
            parent_id: Some("part:1".to_string()),
            kind: NodeKind::File,
            size: 0,
            deleted: true,
            inode: None,
            timestamps: Mactb::default(),
            link_target: None,
            has_snapshot: false,
        }
    }

    #[test]
    fn fs_node_page_cursor_resumes_after_last_sort_key() {
        let nodes = vec![
            test_node("beta", "ntfs:part:1:2"),
            test_node("alpha", "ntfs:part:1:1"),
            test_node("beta", "ntfs:part:1:3"),
        ];

        let first = fs_node_page_from_sorted_nodes(nodes.clone(), None, Some(2)).unwrap();
        assert_eq!(
            first
                .items
                .iter()
                .map(|node| node.id.as_str())
                .collect::<Vec<_>>(),
            vec!["ntfs:part:1:1", "ntfs:part:1:2"]
        );
        let second =
            fs_node_page_from_sorted_nodes(nodes, first.next_cursor.as_deref(), Some(2)).unwrap();
        assert_eq!(
            second
                .items
                .iter()
                .map(|node| node.id.as_str())
                .collect::<Vec<_>>(),
            vec!["ntfs:part:1:3"]
        );
        assert!(second.next_cursor.is_none());
    }
}
