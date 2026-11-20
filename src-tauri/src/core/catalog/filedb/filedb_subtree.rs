//! Subtree emptiness on a volume index: does a directory hold any non-deleted,
//! non-directory entry anywhere beneath it? Mirrors
//! [`VolumeSnapshot::subtree_has_file`](crate::core::catalog::snapshot::volume_snapshot::VolumeSnapshot::subtree_has_file),
//! so index-backed and snapshot-backed directory pages hide the same folders.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::{params, Connection};

use crate::core::catalog::filedb::filedb_file;
use crate::core::catalog::filedb::filedb_query::{parent_key, walked_dir_exists};
use crate::core::filesystem::meta_addr::IS_DELETE_DELETED;

/// Upper bound on nodes one subtree probe visits before it answers `None`.
const SUBTREE_SCAN_CAP: usize = 200_000;

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

/// `Some(true)` as soon as a non-deleted, non-directory entry turns up under
/// `parent_id`, `Some(false)` when only (recursively) empty directories
/// remain, and `None` when the index cannot tell: the start or a descendant
/// directory was never walked, or the scan passed [`SUBTREE_SCAN_CAP`] nodes.
/// Deleted entries are ignored, as the browsable tree never lists them.
pub(crate) fn subtree_has_file_in(
    conn: &Connection,
    parent_id: &str,
) -> anyhow::Result<Option<bool>> {
    let start_key = parent_key(parent_id);
    if !walked_dir_exists(conn, start_key)? {
        return Ok(None);
    }

    let mut stack = vec![start_key.to_string()];
    let mut visited = HashSet::from([start_key.to_string()]);
    let mut budget = SUBTREE_SCAN_CAP;
    while let Some(parent) = stack.pop() {
        let children = child_edges(conn, &parent)?;
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
            if !walked_dir_exists(conn, &child.nid)? {
                return Ok(None);
            }
            if visited.insert(child.nid.clone()) {
                stack.push(child.nid);
            }
        }
    }
    Ok(Some(false))
}

/// [`subtree_has_file_in`] on a freshly opened reader.
pub fn subtree_has_file(index: &Path, parent_id: &str) -> anyhow::Result<Option<bool>> {
    let conn = filedb_file::open_reader(index)?;
    subtree_has_file_in(&conn, parent_id)
}

/// [`subtree_has_file_in`] for many directories over one connection. A
/// directory the index cannot judge is left out of the map.
pub fn subtree_has_files(
    index: &Path,
    parent_ids: &[String],
) -> anyhow::Result<HashMap<String, bool>> {
    let conn = filedb_file::open_reader(index)?;
    let mut decided = HashMap::with_capacity(parent_ids.len());
    for parent_id in parent_ids {
        if let Some(has_file) = subtree_has_file_in(&conn, parent_id)? {
            decided.insert(parent_id.clone(), has_file);
        }
    }
    Ok(decided)
}

/// Non-deleted child directories of `parent_id` whose whole subtree holds no
/// file. One indexed query keeps only the directories without a direct file
/// (usually none), so the recursive probe runs on those alone. Directories
/// the index cannot judge are not returned.
pub(crate) fn empty_child_dirs(conn: &Connection, parent_id: &str) -> anyhow::Result<Vec<String>> {
    // `INDEXED BY pid` pins both lookups to the (pid, name, nid) index: without
    // statistics the planner could otherwise walk the volume-wide `kind` index.
    let mut stmt = conn.prepare_cached(
        "SELECT d.nid FROM filedb d INDEXED BY pid
         WHERE d.pid = ?1 AND d.kind = 'dir' AND d.is_delete != ?2
           AND NOT EXISTS (
               SELECT 1 FROM filedb f INDEXED BY pid
               WHERE f.pid = d.nid AND f.kind != 'dir' AND f.is_delete != ?2
           )",
    )?;
    let candidates = stmt
        .query_map(params![parent_key(parent_id), IS_DELETE_DELETED], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut empty = Vec::new();
    for candidate in candidates {
        if subtree_has_file_in(conn, &candidate)? == Some(false) {
            empty.push(candidate);
        }
    }
    Ok(empty)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::catalog::filedb::filedb_store::test_support::*;
    use crate::core::filesystem::tree::fs_tree::NodeKind;

    #[test]
    fn subtree_probe_reads_the_index() {
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
            subtree_has_files(&index, &["empty".into(), "dir-b".into(), "missing".into()])
                .expect("batch"),
            HashMap::from([("empty".to_string(), false), ("dir-b".to_string(), true)])
        );
    }

    #[test]
    fn empty_child_dirs_follow_nested_empty_folders_and_skip_deleted_ones() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut deleted_empty = entry("part:0", "gone", "gone", NodeKind::Dir, "/gone");
        deleted_empty.node.deleted = true;
        let mut deleted_file = entry(
            "only-deleted",
            "old",
            "old.txt",
            NodeKind::File,
            "/only-deleted/old.txt",
        );
        deleted_file.node.deleted = true;
        let index = build_test_index(
            dir.path(),
            "part:0",
            &[
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
                entry(
                    "part:0",
                    "only-deleted",
                    "only-deleted",
                    NodeKind::Dir,
                    "/only-deleted",
                ),
                deleted_file,
                entry("part:0", "bare", "bare", NodeKind::Dir, "/bare"),
                deleted_empty,
            ],
        );
        let conn = filedb_file::open_reader(&index).expect("open");
        let mut empty = empty_child_dirs(&conn, "part:0").expect("empty dirs");
        empty.sort();
        assert_eq!(empty, vec!["bare", "nested", "only-deleted"]);
    }
}
