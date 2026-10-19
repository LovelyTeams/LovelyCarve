//! Per-view directory cache for path-based plugin access. The cache ends
//! with its run's view; node and directory budgets bound memory.
use crate::core::filesystem::tree::fs_tree::FsNode;
use lru::LruCache;
use std::sync::Arc;

const MAX_DIRECTORIES: usize = 4096;
const MAX_NODES: usize = 65_536;

pub(super) struct DirectoryCache {
    directories: LruCache<String, Arc<Vec<FsNode>>>,
    nodes: usize,
}

impl Default for DirectoryCache {
    fn default() -> Self {
        Self {
            directories: LruCache::unbounded(),
            nodes: 0,
        }
    }
}

impl DirectoryCache {
    pub fn get(&mut self, directory: &str) -> Option<Arc<Vec<FsNode>>> {
        self.directories.get(directory).map(Arc::clone)
    }

    pub fn insert(&mut self, directory: String, nodes: Arc<Vec<FsNode>>) {
        if nodes.len() > MAX_NODES {
            return;
        }
        if let Some(previous) = self.directories.pop(&directory) {
            self.nodes -= previous.len();
        }
        while self.nodes + nodes.len() > MAX_NODES || self.directories.len() >= MAX_DIRECTORIES {
            if let Some((_, previous)) = self.directories.pop_lru() {
                self.nodes -= previous.len();
            } else {
                break;
            }
        }
        self.nodes += nodes.len();
        self.directories.put(directory, nodes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn directory_budget_evicts_the_least_recently_used_entry() {
        let mut cache = DirectoryCache::default();
        for index in 0..MAX_DIRECTORIES {
            cache.insert(index.to_string(), Arc::new(Vec::new()));
        }
        assert!(cache.get("0").is_some());
        cache.insert("new".into(), Arc::new(Vec::new()));
        assert!(cache.get("1").is_none());
        assert!(cache.get("0").is_some());
        assert_eq!(cache.directories.len(), MAX_DIRECTORIES);
    }

    #[test]
    fn node_budget_and_replacement_keep_accounting_bounded() {
        let node = FsNode {
            id: "file".into(),
            name: "file".into(),
            parent_id: None,
            kind: crate::core::filesystem::tree::fs_tree::NodeKind::File,
            size: 0,
            deleted: false,
            inode: None,
            timestamps: Default::default(),
            link_target: None,
            has_snapshot: false,
        };
        let mut cache = DirectoryCache::default();
        cache.insert("large".into(), Arc::new(vec![node.clone(); MAX_NODES]));
        cache.insert("small".into(), Arc::new(vec![node.clone()]));
        assert!(cache.get("large").is_none());
        assert_eq!(cache.nodes, 1);
        cache.insert("small".into(), Arc::new(vec![node.clone(); 2]));
        assert_eq!(cache.nodes, 2);
        cache.insert("oversize".into(), Arc::new(vec![node; MAX_NODES + 1]));
        assert!(cache.get("oversize").is_none());
        assert_eq!(cache.nodes, 2);
    }
}
