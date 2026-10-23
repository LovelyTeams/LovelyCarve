//! Size trees built for the file list's treemap view.
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::OnceCell;

use crate::core::catalog::size_tree::SizeTree;

/// One volume's tree, built once by whichever request comes first; requests
/// arriving meanwhile wait for that build instead of starting their own.
pub type SizeTreeSlot = Arc<OnceCell<Arc<SizeTree>>>;

/// Size trees of volume indexes, keyed by `(image_id, index path)`. A tree
/// holds a whole index in memory (about 50 bytes per row), so only the two
/// most recently used stay. A rebuilt snapshot publishes a new index file and
/// so gets a new entry. Purged per image on unload.
#[derive(Default)]
pub struct SizeTreeCache {
    entries: VecDeque<(String, PathBuf, SizeTreeSlot)>,
}

impl SizeTreeCache {
    const MAX_ENTRIES: usize = 2;

    /// The slot for `index` of `image_id`, made when new, and now the most
    /// recently used.
    pub fn slot(&mut self, image_id: &str, index: &Path) -> SizeTreeSlot {
        let cached = self
            .entries
            .iter()
            .position(|(image, path, _)| image == image_id && path == index)
            .and_then(|position| self.entries.remove(position));
        let entry = cached.unwrap_or_else(|| {
            (
                image_id.to_string(),
                index.to_path_buf(),
                SizeTreeSlot::default(),
            )
        });
        let slot = Arc::clone(&entry.2);
        self.entries.push_back(entry);
        while self.entries.len() > Self::MAX_ENTRIES {
            self.entries.pop_front();
        }
        slot
    }

    /// Drop every tree of `image_id`.
    pub fn purge_image(&mut self, image_id: &str) {
        self.entries.retain(|(image, _, _)| image != image_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_two_most_recently_used_trees() {
        let mut cache = SizeTreeCache::default();
        let first = cache.slot("a", Path::new("one.db"));
        let _second = cache.slot("a", Path::new("two.db"));
        assert!(Arc::ptr_eq(&first, &cache.slot("a", Path::new("one.db"))));
        // two.db is now the least recently used and makes way.
        let _third = cache.slot("b", Path::new("three.db"));
        assert!(Arc::ptr_eq(&first, &cache.slot("a", Path::new("one.db"))));
        let images: Vec<&str> = cache
            .entries
            .iter()
            .map(|(image, ..)| image.as_str())
            .collect();
        assert_eq!(images, vec!["b", "a"]);
        cache.purge_image("a");
        assert_eq!(cache.entries.len(), 1);
    }
}
