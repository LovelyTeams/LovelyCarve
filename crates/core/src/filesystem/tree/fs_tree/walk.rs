//! Filesystem session walk operations.
use super::*;

impl<'a> FsSession<'a> {
    /// Stream-walk the whole tree using this walker's cached `children`,
    /// invoking `visit` per node. Return `Ok(false)` from `visit` to stop
    /// early (how a cancellable search aborts enumeration promptly). Every
    /// directory listing reuses the cached volume handle, so enumerating a
    /// large NTFS/ext4 tree opens each volume once rather than once per
    /// directory. A directory that fails to list is skipped (logged) rather
    /// than aborting the entire walk.
    pub fn walk_visit<F>(&mut self, visit: &mut F) -> anyhow::Result<()>
    where
        F: FnMut(&FsWalkEntry) -> anyhow::Result<bool>,
    {
        let mut visited = HashSet::new();
        for node in self.children("")? {
            if !self.walk_visit_node(node, String::new(), visit, &no_prune, &mut visited)? {
                break;
            }
        }
        Ok(())
    }

    /// Like [`Self::walk_visit`] but rooted at `start_id`'s children rather
    /// than the image root. An empty `start_id` walks the whole image. Used by
    /// the mactime timeline export to scope a body-file to one subtree.
    pub fn walk_visit_from<F>(&mut self, start_id: &str, visit: &mut F) -> anyhow::Result<()>
    where
        F: FnMut(&FsWalkEntry) -> anyhow::Result<bool>,
    {
        self.walk_visit_from_pruned(start_id, visit, &no_prune)
    }

    /// Like [`Self::walk_visit_from`] but starts from a caller-supplied root
    /// node and visits that node before its descendants. Used when the caller
    /// already resolved the logical directory node and wants the root included
    /// in aggregate accounting.
    pub fn walk_visit_subtree<F>(&mut self, root: FsNode, visit: &mut F) -> anyhow::Result<()>
    where
        F: FnMut(&FsWalkEntry) -> anyhow::Result<bool>,
    {
        let mut visited = HashSet::new();
        self.walk_visit_node(root, String::new(), visit, &no_prune, &mut visited)
            .map(|_| ())
    }

    /// Like [`Self::walk_visit_from`] but never descends into a directory for
    /// which `prune` returns `true` — the directory node itself is still
    /// visited, only its subtree is skipped.
    pub fn walk_visit_from_pruned<F>(
        &mut self,
        start_id: &str,
        visit: &mut F,
        prune: &dyn Fn(&FsNode) -> bool,
    ) -> anyhow::Result<()>
    where
        F: FnMut(&FsWalkEntry) -> anyhow::Result<bool>,
    {
        let mut visited = HashSet::new();
        for node in self.children(start_id)? {
            if !self.walk_visit_node(node, String::new(), visit, prune, &mut visited)? {
                break;
            }
        }
        Ok(())
    }

    pub(super) fn walk_visit_node<F>(
        &mut self,
        node: FsNode,
        parent_path: String,
        visit: &mut F,
        prune: &dyn Fn(&FsNode) -> bool,
        visited: &mut HashSet<String>,
    ) -> anyhow::Result<bool>
    where
        F: FnMut(&FsWalkEntry) -> anyhow::Result<bool>,
    {
        let path = if parent_path.is_empty() {
            format!("/{}", node.name)
        } else {
            format!("{parent_path}/{}", node.name)
        };
        let is_dir = node.kind == NodeKind::Dir;
        let node_id = node.id.clone();
        // Match the standalone `walk_node`: directory hardlinks/cycles are
        // neither descended into nor emitted twice, so stats and scans do not
        // double-count a directory reachable through multiple parents. Files
        // remain leaves and are emitted for every directory entry.
        if is_dir && !visited.insert(node_id.clone()) {
            return Ok(true);
        }
        let entry = FsWalkEntry {
            node,
            path: path.clone(),
        };
        if !visit(&entry)? {
            return Ok(false);
        }
        if !is_dir || prune(&entry.node) {
            return Ok(true);
        }
        let children = match self.children(&node_id) {
            Ok(children) => children,
            Err(err) => {
                tracing::debug!("skip unreadable directory {node_id}: {err}");
                return Ok(true);
            }
        };
        for child in children {
            if !self.walk_visit_node(child, path.clone(), visit, prune, visited)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
