//! 卷索引的查询路径(读侧)。每个函数针对一个已发布的卷索引文件
//! ([`crate::catalog::filedb::filedb_file`]),只读打开:
//!
//! * 目录浏览 —— `WHERE pid = ?`,`name COLLATE natsort` 排序 + keyset 分页;
//! * 大小写不敏感的名称 / 路径查询 —— 一律走预存的小写副本列(`name_low`、
//!   `full_path_low`),从不 `LOWER()`;
//! * 扩展名 / 大小 / 类型过滤;
//! * 子项计数 —— 构建时预计算的计数缓存点查;
//! * MetaAddr 解析 —— `(meta_addr, meta_addr_high)` 回到适配器节点 id。
//!
//! 文件全显(allowAllFiles)的子树分页在同级的 `filedb_flat`。未被走树覆盖的
//! 目录(`filedb_walked_dirs` 无行)返回 `None`,命令层回退实时遍历。

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Context;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::catalog::filedb::filedb_file;
use crate::filesystem::meta_addr::IS_DELETE_DELETED;
use crate::filesystem::tree::fs_tree::FsNode;

const DEFAULT_PAGE_SIZE: usize = 500;
pub(crate) const MAX_PAGE_SIZE: usize = 5_000;
const SUBTREE_SCAN_CAP: usize = 200_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodePage {
    pub items: Vec<FsNode>,
    pub next_cursor: Option<String>,
    pub total: Option<usize>,
    pub truncated: bool,
}

/// One entry of the child-count cache.
/// `inexact` marks counts computed from a truncated snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildCounts {
    pub count: u64,
    pub deleted: u64,
    pub inexact: bool,
}

/// Filter for the generic query path. Text needles are lowercased by
/// [`FileFilter::normalized`] so the generated SQL compares directly against
/// the `_low` columns.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct FileFilter {
    pub name_prefix: Option<String>,
    pub name_contains: Option<String>,
    /// Case-insensitive whole-name SQLite GLOB: `*`, `?`, `[abc]`, `[^abc]`.
    pub name_glob: Option<String>,
    pub path_prefix: Option<String>,
    pub path_contains: Option<String>,
    pub extensions: Vec<String>,
    pub kinds: Vec<String>,
    pub min_size: Option<i64>,
    pub max_size: Option<i64>,
    pub is_delete: Option<i64>,
    pub limit: usize,
    pub offset: usize,
}

impl FileFilter {
    pub fn normalized(mut self) -> Self {
        self.name_prefix = self.name_prefix.take().map(|v| v.to_lowercase());
        self.name_contains = self.name_contains.take().map(|v| v.to_lowercase());
        self.name_glob = self.name_glob.take().map(|v| v.to_lowercase());
        self.path_prefix = self.path_prefix.take().map(|v| v.to_lowercase());
        self.path_contains = self.path_contains.take().map(|v| v.to_lowercase());
        self.extensions = self
            .extensions
            .iter()
            .map(|v| v.trim_start_matches('.').to_lowercase())
            .collect();
        self
    }
}

/// One flattened result row of [`query_files`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHit {
    pub nid: String,
    pub name: String,
    pub full_path: String,
    pub kind: String,
    pub size: i64,
    pub is_delete: i64,
}

/// One indexed node's place in the tree (locate / scope resolution).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRow {
    pub nid: String,
    pub pid: String,
    pub name: String,
    pub kind: String,
    pub full_path: String,
}

pub fn children_page(
    index: &Path,
    parent_id: &str,
    cursor: Option<&str>,
    page_size: Option<usize>,
) -> anyhow::Result<Option<NodePage>> {
    let conn = filedb_file::open_reader(index)?;
    let parent_key = parent_key(parent_id);
    if !walked_dir_exists(&conn, parent_key)? {
        return Ok(None);
    }
    let page_size = page_size
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE);
    let total = child_count_total(&conn, parent_key)?;
    let (cursor_name, cursor_id) = match cursor {
        Some(cursor) => {
            let (name, id) = decode_cursor(cursor)?;
            (Some(name), Some(id))
        }
        None => (None, None),
    };
    let query_limit =
        i64::try_from(page_size.saturating_add(1)).context("filedb page size exceeds i64")?;
    // Keyset pagination on the natural-order `pid` index: strictly after the
    // cursor's (name, nid) under the same collation the ORDER BY uses.
    let mut stmt = conn.prepare(
        r#"
        SELECT node_json
        FROM filedb
        WHERE pid = ?1
          AND (
              ?2 IS NULL
              OR name COLLATE natsort > ?2
              OR (name COLLATE natsort = ?2 AND nid > ?3)
          )
        ORDER BY name COLLATE natsort ASC, nid ASC
        LIMIT ?4
        "#,
    )?;
    let rows = stmt.query_map(
        params![parent_key, cursor_name, cursor_id, query_limit],
        |row| deserialize_node(row, 0),
    )?;
    let mut items = Vec::new();
    for row in rows {
        items.push(row?);
    }
    let has_more = items.len() > page_size;
    if has_more {
        items.truncate(page_size);
    }
    let next_cursor = if has_more {
        items.last().map(cursor_for_node)
    } else {
        None
    };
    Ok(Some(NodePage {
        items,
        next_cursor,
        total: Some(total),
        truncated: false,
    }))
}

pub fn children(index: &Path, parent_id: &str) -> anyhow::Result<Option<Vec<FsNode>>> {
    let conn = filedb_file::open_reader(index)?;
    let parent_key = parent_key(parent_id);
    if !walked_dir_exists(&conn, parent_key)? {
        return Ok(None);
    }
    flat_children(&conn, parent_key).map(Some)
}

/// Point lookups on the counter cache. Walked dirs with no children have a 0
/// row; uncovered dirs have no row.
pub fn child_counts(
    index: &Path,
    parent_ids: &[String],
) -> anyhow::Result<HashMap<String, ChildCounts>> {
    if parent_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let conn = filedb_file::open_reader(index)?;
    let mut unique_parent_ids: Vec<String> = Vec::with_capacity(parent_ids.len());
    for parent_id in parent_ids {
        let key = parent_key(parent_id);
        if !unique_parent_ids.iter().any(|known| known == key) {
            unique_parent_ids.push(key.to_string());
        }
    }
    let placeholders = std::iter::repeat_n("?", unique_parent_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT pid, child_count, child_delete_count, child_count_inexact
         FROM filedb_child_counts
         WHERE pid IN ({placeholders})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(unique_parent_ids.iter()), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    let mut counts = HashMap::with_capacity(unique_parent_ids.len());
    for row in rows {
        let (parent_id, count, deleted, inexact) = row?;
        counts.insert(
            parent_id,
            ChildCounts {
                count: u64::try_from(count).context("filedb child count is negative")?,
                deleted: u64::try_from(deleted).context("filedb delete count is negative")?,
                inexact: inexact != 0,
            },
        );
    }
    Ok(counts)
}

pub fn subtree_has_file(index: &Path, parent_id: &str) -> anyhow::Result<Option<bool>> {
    let conn = filedb_file::open_reader(index)?;
    let start_key = parent_key(parent_id);
    if !walked_dir_exists(&conn, start_key)? {
        return Ok(None);
    }

    let mut stack = vec![start_key.to_string()];
    let mut visited = HashSet::from([start_key.to_string()]);
    let mut budget = SUBTREE_SCAN_CAP;
    while let Some(parent) = stack.pop() {
        let children = child_edges(&conn, &parent)?;
        for child in children {
            if budget == 0 {
                return Ok(None);
            }
            budget -= 1;
            if child.is_deleted {
                continue;
            }
            if child.kind != "dir" {
                return Ok(Some(true));
            }
            if !walked_dir_exists(&conn, &child.nid)? {
                return Ok(None);
            }
            if visited.insert(child.nid.clone()) {
                stack.push(child.nid);
            }
        }
    }
    Ok(Some(false))
}

/// The generic filter path (name / path / extension / size / kind / delete),
/// paged with `LIMIT ? OFFSET ?` and naturally ordered.
pub fn query_files(index: &Path, filter: &FileFilter) -> anyhow::Result<Vec<FileHit>> {
    let conn = filedb_file::open_reader(index)?;
    let mut sql = String::from(
        "SELECT nid, name, full_path, kind, size, is_delete
         FROM filedb
         WHERE 1 = 1",
    );
    let mut bind: Vec<String> = Vec::new();
    if let Some((lower, upper)) = filter.name_prefix.as_deref().and_then(prefix_bounds) {
        sql.push_str(" AND name_low >= ? AND name_low < ?");
        bind.push(lower);
        bind.push(upper);
    }
    if let Some(needle) = &filter.name_contains {
        sql.push_str(" AND name_low LIKE ? ESCAPE '\\'");
        bind.push(like_contains_escaped(needle));
    }
    if let Some(pattern) = &filter.name_glob {
        sql.push_str(" AND name_low GLOB ?");
        bind.push(pattern.clone());
    }
    if let Some((lower, upper)) = filter.path_prefix.as_deref().and_then(prefix_bounds) {
        sql.push_str(" AND full_path_low >= ? AND full_path_low < ?");
        bind.push(lower);
        bind.push(upper);
    }
    if let Some(needle) = &filter.path_contains {
        sql.push_str(" AND full_path_low LIKE ? ESCAPE '\\'");
        bind.push(like_contains_escaped(needle));
    }
    if !filter.extensions.is_empty() {
        let placeholders = std::iter::repeat_n("?", filter.extensions.len())
            .collect::<Vec<_>>()
            .join(",");
        sql.push_str(&format!(" AND extension IN ({placeholders})"));
        bind.extend(filter.extensions.iter().cloned());
    }
    if !filter.kinds.is_empty() {
        let placeholders = std::iter::repeat_n("?", filter.kinds.len())
            .collect::<Vec<_>>()
            .join(",");
        sql.push_str(&format!(" AND kind IN ({placeholders})"));
        bind.extend(filter.kinds.iter().cloned());
    }
    if let Some(min_size) = filter.min_size {
        sql.push_str(" AND size >= ?");
        bind.push(min_size.to_string());
    }
    if let Some(max_size) = filter.max_size {
        sql.push_str(" AND size <= ?");
        bind.push(max_size.to_string());
    }
    if let Some(is_delete) = filter.is_delete {
        sql.push_str(" AND is_delete = ?");
        bind.push(is_delete.to_string());
    }
    sql.push_str(" ORDER BY name COLLATE natsort ASC, nid ASC LIMIT ? OFFSET ?");
    let limit = filter.limit.clamp(1, MAX_PAGE_SIZE);
    bind.push(limit.to_string());
    bind.push(filter.offset.to_string());

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(bind), |row| {
        Ok(FileHit {
            nid: row.get(0)?,
            name: row.get(1)?,
            full_path: row.get(2)?,
            kind: row.get(3)?,
            size: row.get(4)?,
            is_delete: row.get(5)?,
        })
    })?;
    let mut hits = Vec::new();
    for row in rows {
        hits.push(row?);
    }
    Ok(hits)
}

/// Resolve a MetaAddr pair back to the adapter node id. The index is
/// authoritative (deleted files, path-addressed filesystems, named streams);
/// path-addressed entries (`meta_addr = 0`) are discriminated by their
/// lowercase full path.
pub fn node_id_for_meta_addr(
    index: &Path,
    meta_addr: i64,
    meta_addr_high: i64,
    full_path: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let conn = filedb_file::open_reader(index)?;
    let lookup = if meta_addr == 0 && meta_addr_high == 0 {
        let Some(full_path) = full_path else {
            return Ok(None);
        };
        conn.query_row(
            "SELECT nid FROM filedb WHERE full_path_low = ?1 LIMIT 1",
            params![full_path.to_lowercase()],
            |row| row.get::<_, String>(0),
        )
    } else {
        conn.query_row(
            "SELECT nid FROM filedb WHERE meta_addr = ?1 AND meta_addr_high = ?2 LIMIT 1",
            params![meta_addr, meta_addr_high],
            |row| row.get::<_, String>(0),
        )
    };
    lookup.optional().map_err(Into::into)
}

/// One indexed node by id.
pub fn node_row(conn: &Connection, nid: &str) -> anyhow::Result<Option<NodeRow>> {
    conn.query_row(
        "SELECT nid, pid, name, kind, full_path FROM filedb WHERE nid = ?1 LIMIT 1",
        params![nid],
        node_row_from,
    )
    .optional()
    .map_err(Into::into)
}

/// One indexed node by case-insensitive full path.
pub fn node_row_by_path(conn: &Connection, full_path: &str) -> anyhow::Result<Option<NodeRow>> {
    conn.query_row(
        "SELECT nid, pid, name, kind, full_path FROM filedb WHERE full_path_low = ?1 LIMIT 1",
        params![full_path.to_lowercase()],
        node_row_from,
    )
    .optional()
    .map_err(Into::into)
}

fn node_row_from(row: &rusqlite::Row<'_>) -> rusqlite::Result<NodeRow> {
    Ok(NodeRow {
        nid: row.get(0)?,
        pid: row.get(1)?,
        name: row.get(2)?,
        kind: row.get(3)?,
        full_path: row.get(4)?,
    })
}

/// Category aggregation for one directory.
pub fn category_summary(
    index: &Path,
    parent_id: &str,
) -> anyhow::Result<Option<Vec<(String, u64)>>> {
    let conn = filedb_file::open_reader(index)?;
    let parent_key = parent_key(parent_id);
    let mut stmt = conn.prepare(
        "SELECT kind, COUNT(*) FROM filedb
         WHERE pid = ?1
         GROUP BY kind
         ORDER BY kind",
    )?;
    let rows = stmt.query_map(params![parent_key], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut summary = Vec::new();
    for row in rows {
        let (kind, count) = row?;
        summary.push((
            kind,
            u64::try_from(count).context("filedb category count is negative")?,
        ));
    }
    Ok(Some(summary))
}

pub fn parent_key(parent_id: &str) -> &str {
    if parent_id.trim().is_empty() {
        "root"
    } else {
        parent_id
    }
}

struct ChildEdge {
    nid: String,
    kind: String,
    is_deleted: bool,
}

fn child_edges(conn: &Connection, parent_id: &str) -> anyhow::Result<Vec<ChildEdge>> {
    let mut stmt = conn.prepare_cached(
        "SELECT nid, kind, is_delete FROM filedb
         WHERE pid = ?1
         ORDER BY name COLLATE natsort ASC, nid ASC",
    )?;
    let rows = stmt.query_map(params![parent_id], |row| {
        Ok(ChildEdge {
            nid: row.get(0)?,
            kind: row.get(1)?,
            is_deleted: row.get::<_, i64>(2)? == IS_DELETE_DELETED,
        })
    })?;
    let mut edges = Vec::new();
    for row in rows {
        edges.push(row?);
    }
    Ok(edges)
}

fn flat_children(conn: &Connection, parent_id: &str) -> anyhow::Result<Vec<FsNode>> {
    let mut stmt = conn.prepare(
        "SELECT node_json FROM filedb
         WHERE pid = ?1
         ORDER BY name COLLATE natsort ASC, nid ASC",
    )?;
    let rows = stmt.query_map(params![parent_id], |row| deserialize_node(row, 0))?;
    let mut children = Vec::new();
    for row in rows {
        children.push(row?);
    }
    Ok(children)
}

pub(crate) fn walked_dir_exists(conn: &Connection, parent_id: &str) -> anyhow::Result<bool> {
    let exists: u8 = conn
        .prepare_cached("SELECT EXISTS(SELECT 1 FROM filedb_walked_dirs WHERE pid = ?1)")?
        .query_row(params![parent_id], |row| row.get(0))?;
    Ok(exists != 0)
}

pub(crate) fn deserialize_node(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<FsNode> {
    let node_json: String = row.get(idx)?;
    serde_json::from_str::<FsNode>(&node_json).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, Box::new(err))
    })
}

fn child_count_total(conn: &Connection, parent_id: &str) -> anyhow::Result<usize> {
    let count: i64 = conn.query_row(
        "SELECT child_count FROM filedb_child_counts WHERE pid = ?1",
        params![parent_id],
        |row| row.get(0),
    )?;
    let count = u64::try_from(count).context("filedb child count is negative")?;
    usize::try_from(count).context("filedb child count exceeds platform usize")
}

fn cursor_for_node(node: &FsNode) -> String {
    format!(
        "{}:{}",
        hex::encode(node.name.as_bytes()),
        hex::encode(node.id.as_bytes())
    )
}

fn decode_cursor(cursor: &str) -> anyhow::Result<(String, String)> {
    let (name_hex, id_hex) = cursor
        .split_once(':')
        .context("invalid filedb node page cursor")?;
    let name = String::from_utf8(hex::decode(name_hex).context("decode cursor name")?)
        .context("cursor name is not utf8")?;
    let id = String::from_utf8(hex::decode(id_hex).context("decode cursor id")?)
        .context("cursor id is not utf8")?;
    Ok((name, id))
}

/// Half-open range `[prefix, prefix + U+10FFFF)` covering every string that
/// starts with `prefix` under BINARY (memcmp) collation. A range comparison is
/// served straight from the `_low` index, whereas `LIKE … ESCAPE` disables
/// SQLite's LIKE optimization and forces a scan.
pub(crate) fn prefix_bounds(prefix: &str) -> Option<(String, String)> {
    if prefix.is_empty() {
        return None;
    }
    let mut upper = String::with_capacity(prefix.len() + 4);
    upper.push_str(prefix);
    upper.push(char::MAX);
    Some((prefix.to_string(), upper))
}

pub(crate) fn like_contains_escaped(needle: &str) -> String {
    format!("%{}%", escape_like(needle))
}

pub(crate) fn escape_like(needle: &str) -> String {
    let mut escaped = String::with_capacity(needle.len());
    for character in needle.chars() {
        if matches!(character, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

#[cfg(test)]
#[path = "filedb_query/glob_tests.rs"]
mod glob_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::filedb::filedb_store::test_support::*;
    use crate::filesystem::tree::fs_tree::NodeKind;

    #[test]
    fn pages_children_in_natural_order_with_stable_cursor() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = build_test_index(
            dir.path(),
            "part:0",
            &[
                entry("part:0", "file-2", "file-2", NodeKind::File, "/file-2"),
                entry("part:0", "file-10", "file-10", NodeKind::File, "/file-10"),
                entry("part:0", "file-1", "file-1", NodeKind::File, "/file-1"),
                entry("part:0", "dir-b", "b", NodeKind::Dir, "/b"),
            ],
        );

        let first = children_page(&index, "part:0", None, Some(2))
            .expect("page")
            .expect("covered");
        assert_eq!(first.total, Some(4));
        let names: Vec<&str> = first.items.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, vec!["b", "file-1"]);
        assert!(first.next_cursor.is_some());

        let second = children_page(&index, "part:0", first.next_cursor.as_deref(), Some(10))
            .expect("page")
            .expect("covered");
        let names: Vec<&str> = second.items.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["file-2", "file-10"],
            "natsort: file-2 before file-10"
        );
        assert!(second.next_cursor.is_none());
    }

    #[test]
    fn uncovered_directories_fall_back_with_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = build_test_index(dir.path(), "part:0", &entries());
        assert!(
            children_page(&index, "ntfs:part:0:not-walked", None, Some(10))
                .expect("page")
                .is_none()
        );
        let covered_empty = children(&index, "empty")
            .expect("children")
            .expect("walked empty dir is covered");
        assert!(covered_empty.is_empty());
    }

    #[test]
    fn child_counts_reports_cache_counters() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut deleted = entry("part:0", "gone", "gone", NodeKind::File, "/gone");
        deleted.node.deleted = true;
        let mut all = entries();
        all.push(deleted);
        all.push(entry("part:0", "keep", "keep", NodeKind::File, "/keep"));
        let index = build_test_index(dir.path(), "part:0", &all);

        let counts = child_counts(
            &index,
            &[
                "part:0".to_string(),
                "missing".to_string(),
                "empty".to_string(),
            ],
        )
        .expect("counts");
        assert_eq!(
            counts.get("part:0"),
            Some(&ChildCounts {
                count: 4,
                deleted: 1,
                inexact: false
            })
        );
        assert_eq!(
            counts.get("empty"),
            Some(&ChildCounts {
                count: 0,
                deleted: 0,
                inexact: false
            })
        );
        assert!(!counts.contains_key("missing"));
    }

    #[test]
    fn query_files_uses_lowercase_columns_and_escapes_like() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = build_test_index(
            dir.path(),
            "part:0",
            &[
                entry(
                    "part:0",
                    "ntfs:part:0:1",
                    "Report.PDF",
                    NodeKind::File,
                    "/Docs/Report.PDF",
                ),
                entry(
                    "part:0",
                    "ntfs:part:0:2",
                    "notes.txt",
                    NodeKind::File,
                    "/Docs/notes.txt",
                ),
                entry(
                    "part:0",
                    "ntfs:part:0:3",
                    "100_percent.log",
                    NodeKind::File,
                    "/100_percent.log",
                ),
                entry(
                    "part:0",
                    "ntfs:part:0:4",
                    "100xpercent.log",
                    NodeKind::File,
                    "/100xpercent.log",
                ),
            ],
        );
        let query = |filter: FileFilter| query_files(&index, &filter.normalized()).expect("query");

        let by_ext = query(FileFilter {
            extensions: vec![".PDF".to_string()],
            limit: 10,
            ..FileFilter::default()
        });
        assert_eq!(by_ext.len(), 1);
        assert_eq!(by_ext[0].name, "Report.PDF");

        let by_name = query(FileFilter {
            name_contains: Some("REPORT".to_string()),
            limit: 10,
            ..FileFilter::default()
        });
        assert_eq!(by_name[0].nid, "ntfs:part:0:1");

        let path_prefix = query(FileFilter {
            path_prefix: Some("/DOCS/".to_string()),
            limit: 10,
            ..FileFilter::default()
        });
        assert_eq!(path_prefix.len(), 2);

        let literal_underscore = query(FileFilter {
            name_contains: Some("100_percent".to_string()),
            limit: 10,
            ..FileFilter::default()
        });
        assert_eq!(literal_underscore.len(), 1, "_ is escaped, not a wildcard");

        let by_size = query(FileFilter {
            min_size: Some(200),
            limit: 10,
            ..FileFilter::default()
        });
        assert!(by_size.is_empty(), "every fixture row is 123 bytes");
    }

    #[test]
    fn meta_addr_and_node_lookups_read_the_index() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = build_test_index(
            dir.path(),
            "part:0",
            &[
                entry("part:0", "ntfs:part:0:512", "mft", NodeKind::File, "/mft"),
                entry(
                    "part:0",
                    "fat:part:0:/Docs/x.txt",
                    "x.txt",
                    NodeKind::File,
                    "/Docs/x.txt",
                ),
            ],
        );
        assert_eq!(
            node_id_for_meta_addr(&index, 512, 0, None)
                .expect("lookup")
                .as_deref(),
            Some("ntfs:part:0:512")
        );
        assert_eq!(
            node_id_for_meta_addr(&index, 0, 0, Some("/docs/X.TXT"))
                .expect("lookup")
                .as_deref(),
            Some("fat:part:0:/Docs/x.txt")
        );
        assert!(node_id_for_meta_addr(&index, 999, 0, None)
            .expect("lookup")
            .is_none());

        let conn = filedb_file::open_reader(&index).expect("open");
        let row = node_row(&conn, "ntfs:part:0:512")
            .expect("row")
            .expect("present");
        assert_eq!((row.pid.as_str(), row.kind.as_str()), ("part:0", "file"));
        assert_eq!(
            node_row_by_path(&conn, "/DOCS/x.txt")
                .expect("row")
                .map(|row| row.nid),
            Some("fat:part:0:/Docs/x.txt".to_string())
        );
    }

    #[test]
    fn subtree_probe_and_category_summary_read_the_index() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = build_test_index(dir.path(), "part:0", &entries());
        assert_eq!(
            subtree_has_file(&index, "part:0").expect("subtree"),
            Some(true)
        );
        assert_eq!(
            subtree_has_file(&index, "empty").expect("subtree"),
            Some(false)
        );
        assert_eq!(
            category_summary(&index, "part:0")
                .expect("summary")
                .expect("covered"),
            vec![("dir".to_string(), 1), ("file".to_string(), 1)]
        );
    }
}
