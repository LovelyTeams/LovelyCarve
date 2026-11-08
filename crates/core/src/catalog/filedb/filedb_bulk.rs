//! Directory coverage and child counts tallied while rows stream into one
//! volume's index file, published as `filedb_walked_dirs` and
//! `filedb_child_counts` inside the build transaction.

use std::collections::HashMap;

use rusqlite::{params, Connection};

use crate::filesystem::meta_addr::IS_DELETE_DELETED;

/// Directory coverage and child counts of one volume.
///
/// Counting while the rows stream in replaces a publish-time
/// `LEFT JOIN … GROUP BY` over the fresh `pid` index, which read every
/// child's table row back at random for `is_delete`. Keys are owned node ids,
/// so memory grows with the number of distinct parents and directories.
pub(crate) struct DirectoryTally {
    directories: HashMap<String, DirectoryCounts>,
}

#[derive(Default)]
struct DirectoryCounts {
    children: i64,
    deleted: i64,
    /// Listed by the walk (the volume root or a live directory). Only walked
    /// directories get coverage and count rows; children of an unwalked
    /// parent (a deleted directory) are counted but never published.
    walked: bool,
}

impl DirectoryTally {
    pub(crate) fn new(root_key: &str) -> Self {
        let root = DirectoryCounts {
            walked: true,
            ..DirectoryCounts::default()
        };
        Self {
            directories: HashMap::from([(root_key.to_string(), root)]),
        }
    }

    /// Count one row under its parent; `is_delete` is the filedb column value.
    pub(crate) fn record_child(&mut self, parent_id: String, is_delete: i64) {
        let parent = self.directories.entry(parent_id).or_default();
        parent.children += 1;
        if is_delete == IS_DELETE_DELETED {
            parent.deleted += 1;
        }
    }

    /// Mark a live directory row as walked (its listing is in the snapshot).
    pub(crate) fn record_walked_dir(&mut self, node_id: String) {
        self.directories.entry(node_id).or_default().walked = true;
    }

    /// Insert the coverage and counter rows. A truncated snapshot marks every
    /// count inexact.
    pub(crate) fn publish(&self, conn: &Connection, truncated: bool) -> anyhow::Result<()> {
        let mut walked: Vec<(&String, &DirectoryCounts)> = self
            .directories
            .iter()
            .filter(|(_, counts)| counts.walked)
            .collect();
        // Primary-key order turns both inserts into b-tree appends.
        walked.sort_unstable_by_key(|(node_id, _)| *node_id);
        let mut walked_stmt = conn.prepare("INSERT INTO filedb_walked_dirs (pid) VALUES (?1)")?;
        let mut counts_stmt = conn.prepare(
            "INSERT INTO filedb_child_counts
                (pid, child_count, child_delete_count, child_count_inexact)
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        for (node_id, counts) in walked {
            walked_stmt.execute(params![node_id])?;
            counts_stmt.execute(params![node_id, counts.children, counts.deleted, truncated])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tally_counts_only_walked_directories() {
        let mut tally = DirectoryTally::new("part:0");
        tally.record_child("part:0".to_string(), 2);
        tally.record_child("part:0".to_string(), IS_DELETE_DELETED);
        tally.record_walked_dir("dir-a".to_string());
        tally.record_child("dir-a".to_string(), 2);
        tally.record_child("gone".to_string(), 2);
        let mut published: Vec<(String, i64, i64)> = tally
            .directories
            .iter()
            .filter(|(_, counts)| counts.walked)
            .map(|(id, counts)| (id.clone(), counts.children, counts.deleted))
            .collect();
        published.sort();
        assert_eq!(
            published,
            vec![("dir-a".to_string(), 1, 0), ("part:0".to_string(), 2, 1)]
        );
    }
}
