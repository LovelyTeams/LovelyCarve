//! Directory filtering, sorting and stable keyset pages, shared by SQL and live sources.
use anyhow::Context;
use rusqlite::{params, params_from_iter, types::Value};
use serde::Deserialize;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::Path;
use {
    crate::core::catalog::filedb::filedb_file,
    crate::core::catalog::filedb::filedb_flat::{metadata_filter_sql, page_join_sql},
    crate::core::catalog::filedb::filedb_query::{self, NodePage},
    crate::core::catalog::filedb::filedb_subtree,
    crate::core::catalog::flat_query::{node_filter_keeps, FlatFileFilter, FlatSort, FlatSortKey},
    crate::core::filesystem::docker::host_docker::{parse_host_docker_id, HostDockerNodeId},
    crate::core::filesystem::tree::fs_tree::{FsNode, FsWalkEntry, NodeKind},
};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DirectoryQuery {
    pub filter: FlatFileFilter,
    pub sort: Option<FlatSortKey>,
    pub descending: bool,
    /// The list's "hide empty directories" toggle: leave out child directories
    /// whose whole subtree holds no file, in both the page and its total.
    /// Directories the source cannot judge stay visible.
    pub hide_empty_dirs: bool,
}

impl DirectoryQuery {
    /// Whether a page under `parent` drops empty directories. The image root
    /// lists partitions and is never filtered.
    pub fn hides_empty_dirs_under(&self, parent: &str) -> bool {
        self.hide_empty_dirs && filedb_query::parent_key(parent) != "root"
    }
    fn order(&self) -> FlatSort {
        FlatSort {
            key: Some(self.sort.unwrap_or(FlatSortKey::Name)),
            desc: self.descending,
        }
    }
    fn compare(&self, left: &FsWalkEntry, right: &FsWalkEntry) -> Ordering {
        (left.node.kind != NodeKind::Dir)
            .cmp(&(right.node.kind != NodeKind::Dir))
            .then_with(|| self.order().compare(left, right))
    }
    fn keeps(&self, node: &FsNode) -> bool {
        let mut filter = self.filter.clone();
        if node.kind != NodeKind::File {
            filter.extensions.clear();
            filter.other_category_exts.clear();
        }
        if node.kind == NodeKind::Dir
            && (filter.size_ge.is_some() || filter.size_le.is_some() || filter.size_mod.is_some())
        {
            return false;
        }
        node_filter_keeps(&walk(node.clone()), &filter)
    }
}
fn walk(node: FsNode) -> FsWalkEntry {
    FsWalkEntry {
        path: format!("/{}", node.name),
        node,
    }
}

/// A Docker container root stays visible as a navigation and mount entry,
/// even before its layers are composed.
fn is_docker_container_root(id: &str) -> bool {
    matches!(
        parse_host_docker_id(id),
        Some(HostDockerNodeId::Container { path, .. }) if path.is_empty()
    )
}

/// The child directories of a memory page whose emptiness is worth judging:
/// deleted directories are never walked by a snapshot, so no source can call
/// them empty, and Docker container roots always stay visible.
pub fn empty_dir_candidates(nodes: &[FsNode]) -> Vec<String> {
    nodes
        .iter()
        .filter(|node| {
            node.kind == NodeKind::Dir && !node.deleted && !is_docker_container_root(&node.id)
        })
        .map(|node| node.id.clone())
        .collect()
}
fn decode(cursor: Option<&str>) -> anyhow::Result<Option<FsNode>> {
    cursor
        .map(|cursor| {
            anyhow::ensure!(cursor.len() <= 65536, "目录分页游标过长");
            Ok(serde_json::from_str(cursor)?)
        })
        .transpose()
}
fn finish(mut items: Vec<FsNode>, total: usize, limit: usize) -> anyhow::Result<NodePage> {
    let more = items.len() > limit;
    items.truncate(limit);
    let next_cursor = if more {
        items.last().map(serde_json::to_string).transpose()?
    } else {
        None
    };
    Ok(NodePage {
        items,
        next_cursor,
        total: Some(total),
        truncated: false,
    })
}

/// One page of an in-memory listing. `hidden_dirs` holds the directories the
/// caller judged empty for [`DirectoryQuery::hide_empty_dirs`] (empty when the
/// toggle is off); they leave both the page and the total.
pub fn memory_page(
    mut nodes: Vec<FsNode>,
    query: &DirectoryQuery,
    hidden_dirs: &HashSet<String>,
    cursor: Option<&str>,
    limit: usize,
) -> anyhow::Result<NodePage> {
    nodes.retain(|node| {
        query.keeps(node) && !(node.kind == NodeKind::Dir && hidden_dirs.contains(&node.id))
    });
    let mut nodes: Vec<_> = nodes.into_iter().map(walk).collect();
    nodes.sort_by(|left, right| query.compare(left, right));
    let total = nodes.len();
    let cursor = decode(cursor)?.map(walk);
    let start = cursor.map_or_default(|cursor| {
        nodes.partition_point(|node| query.compare(node, &cursor) != Ordering::Greater)
    });
    finish(
        nodes
            .into_iter()
            .skip(start)
            .take(limit + 1)
            .map(|entry| entry.node)
            .collect(),
        total,
        limit,
    )
}

/// 一个卷索引文件里的目录分页(过滤、排序与 keyset 游标)。
pub fn indexed_page(
    index: &Path,
    parent: &str,
    query: &DirectoryQuery,
    cursor: Option<&str>,
    limit: usize,
) -> anyhow::Result<Option<NodePage>> {
    let connection = filedb_file::open_reader(index)?;
    let parent = filedb_query::parent_key(parent);
    if !filedb_query::walked_dir_exists(&connection, parent)? {
        return Ok(None);
    }
    let mut base_filter = query.filter.clone();
    base_filter.extensions.clear();
    base_filter.other_category_exts.clear();
    let (base_sql, mut values) = metadata_filter_sql(&base_filter);
    let category_filter = FlatFileFilter {
        extensions: query.filter.extensions.clone(),
        other_category_exts: query.filter.other_category_exts.clone(),
        ..Default::default()
    };
    let (category_sql, category_values) = metadata_filter_sql(&category_filter);
    values.extend(category_values);
    let mut predicate =
        format!("({base_sql}) AND (kind != 'file' OR ({category_sql})) AND pid = ?");
    values.push(Value::Text(parent.into()));
    if base_filter.size_ge.is_some()
        || base_filter.size_le.is_some()
        || base_filter.size_mod.is_some()
    {
        predicate.push_str(" AND kind != 'dir'");
    }
    if query.hides_empty_dirs_under(parent) {
        let hidden: Vec<String> = filedb_subtree::empty_child_dirs(&connection, parent)?
            .into_iter()
            .filter(|nid| !is_docker_container_root(nid))
            .collect();
        if !hidden.is_empty() {
            // One JSON array binds any number of ids for both the count and the page.
            predicate.push_str(" AND nid NOT IN (SELECT value FROM json_each(?))");
            values.push(Value::Text(serde_json::to_string(&hidden)?));
        }
    }
    let total: i64 = connection.query_row(
        &format!("SELECT count(*) FROM filedb WHERE {predicate}"),
        params_from_iter(values.iter()),
        |row| row.get(0),
    )?;
    let total = usize::try_from(total).context("目录条目数超出平台限制")?;
    let expression = query.sort.unwrap_or(FlatSortKey::Name).order_expression();
    let directory = "(CASE WHEN kind = 'dir' THEN 0 ELSE 1 END)";
    if let Some(cursor) = decode(cursor)? {
        let key: Value = connection
            .query_row(
                &format!("SELECT {expression} FROM filedb WHERE pid = ? AND nid = ?"),
                params![parent, cursor.id],
                |row| row.get(0),
            )
            .context("目录已变化，请刷新后重试")?;
        let group = Value::Integer(i64::from(cursor.kind != NodeKind::Dir));
        let comparison = if query.descending { "<" } else { ">" };
        predicate.push_str(&format!(" AND ({directory} > ? OR ({directory} = ? AND ({expression} {comparison} ? OR ({expression} = ? AND nid > ?))))"));
        values.extend([
            group.clone(),
            group,
            key.clone(),
            key,
            Value::Text(cursor.id),
        ]);
    }
    let direction = if query.descending { "DESC" } else { "ASC" };
    let ordering = format!("{directory} ASC, {expression} {direction}, nid ASC");
    values.push(Value::Integer((limit + 1) as i64));
    let sql = page_join_sql(
        "f.node_json",
        &format!("SELECT ordinal FROM filedb WHERE {predicate} ORDER BY {ordering} LIMIT ?"),
        &ordering,
    );
    let mut statement = connection.prepare(&sql)?;
    let items = statement
        .query_map(params_from_iter(values.iter()), |row| {
            filedb_query::deserialize_node(row, 0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    finish(items, total, limit).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        catalog::filedb::filedb_store, catalog::snapshot::volume_snapshot::SnapshotEntry,
        catalog::snapshot::volume_snapshot::VolumeSnapshot, filesystem::tree::fs_tree::Mactb,
    };

    /// Every item of the SQL pages for `query`, read one row per page.
    fn all_indexed(index: &Path, query: &DirectoryQuery) -> (Vec<FsNode>, Option<usize>) {
        let mut cursor = None;
        let mut items = Vec::new();
        loop {
            let page = indexed_page(index, "part:0", query, cursor.as_deref(), 1)
                .expect("sql")
                .expect("covered");
            items.extend(page.items);
            cursor = page.next_cursor;
            if cursor.is_none() {
                return (items, page.total);
            }
        }
    }

    #[test]
    fn sql_and_memory_pages_hide_the_same_empty_directories() {
        use filedb_store::test_support::{entry, header};
        let temporary = tempfile::tempdir().expect("fixture");
        let mut gone = entry("part:0", "gone", "gone", NodeKind::Dir, "/gone");
        gone.node.deleted = true;
        let entries = vec![
            entry("part:0", "full", "full", NodeKind::Dir, "/full"),
            entry("full", "full-file", "f.txt", NodeKind::File, "/full/f.txt"),
            entry("part:0", "nested", "nested", NodeKind::Dir, "/nested"),
            entry("nested", "inner", "inner", NodeKind::Dir, "/nested/inner"),
            entry("part:0", "deep", "deep", NodeKind::Dir, "/deep"),
            entry("deep", "deep-inner", "inner", NodeKind::Dir, "/deep/inner"),
            entry(
                "deep-inner",
                "deep-file",
                "x.bin",
                NodeKind::File,
                "/deep/inner/x.bin",
            ),
            entry("part:0", "bare", "bare", NodeKind::Dir, "/bare"),
            entry("part:0", "top-file", "a.txt", NodeKind::File, "/a.txt"),
            gone,
        ];
        let index =
            filedb_store::test_support::build_test_index(temporary.path(), "part:0", &entries);
        let snapshot = VolumeSnapshot::new(header(), entries.clone());
        let nodes: Vec<FsNode> = entries
            .iter()
            .filter(|entry| entry.parent == "part:0")
            .map(|entry| entry.node.clone())
            .collect();
        // The memory path judges candidates from the snapshot body, as the
        // command layer does for case and session snapshots.
        let hidden: HashSet<String> = empty_dir_candidates(&nodes)
            .into_iter()
            .filter(|id| snapshot.subtree_has_file(id) == Some(false))
            .collect();
        assert_eq!(
            hidden,
            HashSet::from(["bare".to_string(), "nested".to_string()])
        );
        for (sort, descending) in [(None, false), (Some(FlatSortKey::Size), true)] {
            let query = DirectoryQuery {
                sort,
                descending,
                hide_empty_dirs: true,
                ..Default::default()
            };
            let expected = memory_page(nodes.clone(), &query, &hidden, None, 100).expect("memory");
            let (actual, total) = all_indexed(&index, &query);
            assert_eq!(total, expected.total);
            assert_eq!(
                serde_json::to_value(&actual).expect("actual"),
                serde_json::to_value(&expected.items).expect("expected"),
                "{query:?}"
            );
            let mut ids: Vec<&str> = actual.iter().map(|node| node.id.as_str()).collect();
            ids.sort_unstable();
            assert_eq!(ids, vec!["deep", "full", "gone", "top-file"]);
        }
        let (all, total) = all_indexed(&index, &DirectoryQuery::default());
        assert_eq!((all.len(), total), (6, Some(6)));
    }

    #[test]
    fn empty_directory_rules_spare_the_root_and_docker_containers() {
        let query = DirectoryQuery {
            hide_empty_dirs: true,
            ..Default::default()
        };
        assert!(query.hides_empty_dirs_under("part:0"));
        assert!(!query.hides_empty_dirs_under("root"));
        assert!(!query.hides_empty_dirs_under(""));
        assert!(!DirectoryQuery::default().hides_empty_dirs_under("part:0"));
        let dir = |id: &str, deleted: bool| FsNode {
            id: id.into(),
            name: id.into(),
            parent_id: None,
            kind: NodeKind::Dir,
            size: 0,
            deleted,
            inode: None,
            timestamps: Mactb::default(),
            link_target: None,
            has_snapshot: false,
        };
        let nodes = [
            dir("plain", false),
            dir("removed", true),
            dir("host-docker:abc:", false),
            dir("host-docker:abc:2f657463", false),
        ];
        assert_eq!(
            empty_dir_candidates(&nodes),
            vec!["plain".to_string(), "host-docker:abc:2f657463".to_string()]
        );
    }

    #[test]
    fn sql_and_memory_pages_agree_for_filters_ties_directories_and_sort_directions() {
        let temporary = tempfile::tempdir().expect("fixture");
        let nodes: Vec<_> = [
            ("dir", "folder", NodeKind::Dir, 0),
            ("a", "file-2.txt", NodeKind::File, 2),
            ("b", "file-10.pdf", NodeKind::File, 10),
            ("c", "file-2.txt", NodeKind::File, 2),
        ]
        .into_iter()
        .map(|(id, name, kind, size)| FsNode {
            id: id.into(),
            name: name.into(),
            parent_id: Some("part:0".into()),
            kind,
            size,
            deleted: false,
            inode: None,
            timestamps: Mactb::default(),
            link_target: None,
            has_snapshot: false,
        })
        .collect();
        let entries: Vec<_> = nodes
            .iter()
            .map(|node| SnapshotEntry {
                parent: "part:0".into(),
                path: format!("/{}", node.name),
                node: node.clone(),
            })
            .collect();
        let index =
            filedb_store::test_support::build_test_index(temporary.path(), "part:0", &entries);
        for filter in [
            FlatFileFilter::default(),
            FlatFileFilter {
                name_query: Some("file-?.txt".into()),
                ..Default::default()
            },
            FlatFileFilter {
                name_query: Some("file* ".into()),
                ..Default::default()
            },
            FlatFileFilter {
                name_query: Some("file".into()),
                ..Default::default()
            },
            FlatFileFilter {
                extensions: vec!["pdf".into()],
                ..Default::default()
            },
            FlatFileFilter {
                size_ge: Some(2),
                ..Default::default()
            },
        ] {
            for sort in [
                None,
                Some(FlatSortKey::Size),
                Some(FlatSortKey::Name),
                Some(FlatSortKey::Deleted),
                Some(FlatSortKey::Inode),
            ] {
                for descending in [false, true] {
                    let query = DirectoryQuery {
                        filter: filter.clone(),
                        sort,
                        descending,
                        hide_empty_dirs: false,
                    };
                    let expected = memory_page(nodes.clone(), &query, &HashSet::new(), None, 100)
                        .expect("memory");
                    let mut cursor = None;
                    let mut actual = Vec::new();
                    loop {
                        let page = indexed_page(&index, "part:0", &query, cursor.as_deref(), 1)
                            .expect("sql")
                            .expect("covered");
                        assert_eq!(page.total, expected.total);
                        actual.extend(page.items);
                        cursor = page.next_cursor;
                        if cursor.is_none() {
                            break;
                        }
                        assert!(actual.len() <= nodes.len());
                    }
                    assert_eq!(
                        serde_json::to_value(actual).expect("actual"),
                        serde_json::to_value(expected.items).expect("expected"),
                        "{query:?}"
                    );
                }
            }
        }
    }
}
