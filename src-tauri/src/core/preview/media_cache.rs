//! Bounded preview scratch storage. Leases pin files used by WebViews; only
//! unused entries can be evicted. Incomplete writes never enter the cache.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use crate::core::common::ids::NodeRef;

pub const MEDIA_BUDGET: u64 = 4 * 1024 * 1024 * 1024;

/// One materialised preview: the evidence node and the extension it was
/// written with.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MediaKey {
    pub node: NodeRef,
    pub ext: String,
}

struct Entry {
    file: tempfile::TempPath,
    size: u64,
    leases: HashMap<String, String>,
    accessed: u64,
}

#[derive(Default)]
pub struct MediaTempCache {
    entries: HashMap<MediaKey, Entry>,
    locks: HashMap<MediaKey, Weak<tokio::sync::Mutex<()>>>,
    clock: u64,
}

impl MediaTempCache {
    pub fn key_lock(&mut self, key: &MediaKey) -> Arc<tokio::sync::Mutex<()>> {
        self.locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = self.locks.get(key).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        self.locks.insert(key.clone(), Arc::downgrade(&lock));
        lock
    }

    pub fn acquire(&mut self, key: &MediaKey, lease: &str, owner: &str) -> Option<PathBuf> {
        self.clock += 1;
        let entry = self.entries.get_mut(key)?;
        if !entry.file.exists() {
            return None;
        }
        entry.accessed = self.clock;
        entry.leases.insert(lease.into(), owner.into());
        Some(entry.file.to_path_buf())
    }

    pub fn release(&mut self, lease: &str) {
        for entry in self.entries.values_mut() {
            entry.leases.remove(lease);
        }
    }

    pub fn release_owner(&mut self, owner: &str) {
        for entry in self.entries.values_mut() {
            entry.leases.retain(|_, label| label != owner);
        }
    }

    pub fn make_room(&mut self, size: u64) -> anyhow::Result<()> {
        anyhow::ensure!(size <= MEDIA_BUDGET, "文件超过预览缓存容量，请导出后查看");
        while self.entries.values().map(|entry| entry.size).sum::<u64>() + size > MEDIA_BUDGET
            || self.entries.len() >= 6
        {
            let oldest = self
                .entries
                .iter()
                .filter(|(_, entry)| entry.leases.is_empty())
                .min_by_key(|(_, entry)| entry.accessed)
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else {
                anyhow::bail!("预览缓存已被打开的窗口占用，请关闭部分预览后重试");
            };
            self.entries.remove(&oldest);
        }
        Ok(())
    }

    pub fn insert(
        &mut self,
        key: MediaKey,
        file: tempfile::TempPath,
        size: u64,
        lease: String,
        owner: String,
    ) -> PathBuf {
        self.clock += 1;
        let path = file.to_path_buf();
        self.entries.insert(
            key,
            Entry {
                file,
                size,
                leases: HashMap::from([(lease, owner)]),
                accessed: self.clock,
            },
        );
        path
    }

    pub fn purge_image(&mut self, image_id: &str) {
        self.entries.retain(|key, _| key.node.image != image_id);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// A process holds a lock outside its scratch directory. Recovery acquires the
/// same lock before deleting abandoned files, so other running instances survive.
pub struct MediaScratch {
    directory: PathBuf,
    _lock: std::fs::File,
}

impl MediaScratch {
    pub fn open(root: &Path) -> std::io::Result<Self> {
        use fs2::FileExt;
        std::fs::create_dir_all(root)?;
        for entry in std::fs::read_dir(root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name
                .to_str()
                .filter(|name| uuid::Uuid::parse_str(name).is_ok())
            else {
                continue;
            };
            let lock = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(root.join(format!("{name}.lock")))?;
            if lock.try_lock_exclusive().is_ok() {
                if let Err(error) = std::fs::remove_dir_all(entry.path()) {
                    tracing::warn!("清理旧预览缓存失败：{error}");
                }
            }
        }
        let name = uuid::Uuid::new_v4().to_string();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(root.join(format!("{name}.lock")))?;
        lock.try_lock_exclusive()?;
        let directory = root.join(name);
        std::fs::create_dir(&directory)?;
        Ok(Self {
            directory,
            _lock: lock,
        })
    }
    pub fn path(&self) -> &Path {
        &self.directory
    }
    pub fn cleanup(&self) {
        if let Err(error) = std::fs::remove_dir_all(&self.directory) {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("清理预览缓存失败：{error}");
            }
        }
    }
}

impl Drop for MediaScratch {
    fn drop(&mut self) {
        self.cleanup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(image: &str, node: &str) -> MediaKey {
        MediaKey {
            node: NodeRef::new(image, node),
            ext: "mp4".to_string(),
        }
    }
    /// Adds `node` of `image`, leased under the node name.
    fn add_in(cache: &mut MediaTempCache, image: &str, node: &str, size: u64) -> PathBuf {
        cache.make_room(size).expect("space");
        cache.insert(
            key(image, node),
            tempfile::NamedTempFile::new()
                .expect("file")
                .into_temp_path(),
            size,
            node.into(),
            "main".into(),
        )
    }
    fn add(cache: &mut MediaTempCache, node: &str, size: u64) -> PathBuf {
        add_in(cache, "image", node, size)
    }
    #[test]
    fn purging_an_image_keeps_other_images_entries() {
        let mut cache = MediaTempCache::default();
        let purged = add_in(&mut cache, "a", "node", 1);
        // A separator-joined key `a\u{1f}…` would also have matched this one.
        let kept = add_in(&mut cache, "a\u{1f}b", "node", 1);
        cache.purge_image("a");
        assert!(!purged.exists());
        assert!(kept.exists());
        assert!(cache
            .acquire(&key("a\u{1f}b", "node"), "lease", "main")
            .is_some());
    }
    #[test]
    fn active_files_are_pinned_and_released_files_are_evicted_by_bytes() {
        let mut cache = MediaTempCache::default();
        let path = add(&mut cache, "first", MEDIA_BUDGET);
        assert!(cache.make_room(1).is_err());
        assert!(path.exists());
        cache.release("first");
        add(&mut cache, "second", 1);
        assert!(!path.exists());
    }
    #[test]
    fn leases_and_key_locks_are_independent() {
        let mut cache = MediaTempCache::default();
        add(&mut cache, "node", MEDIA_BUDGET);
        cache.acquire(&key("image", "node"), "second", "preview");
        cache.release("node");
        assert!(cache.make_room(1).is_err());
        assert!(Arc::ptr_eq(
            &cache.key_lock(&key("image", "same")),
            &cache.key_lock(&key("image", "same"))
        ));
        cache.release("second");
        assert!(cache.make_room(1).is_ok());
    }
    #[test]
    fn recovery_preserves_live_instances_and_removes_abandoned_directories() {
        let root = tempfile::tempdir().expect("root");
        let live = MediaScratch::open(root.path()).expect("live");
        let abandoned = root.path().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&abandoned).expect("old directory");
        let other = MediaScratch::open(root.path()).expect("other");
        assert!(live.path().exists());
        assert!(!abandoned.exists());
        drop(other);
        assert!(live.path().exists());
    }

    #[test]
    fn destroying_one_window_keeps_other_window_leases() {
        let mut cache = MediaTempCache::default();
        add(&mut cache, "shared", MEDIA_BUDGET);
        cache.acquire(&key("image", "shared"), "preview-lease", "preview");
        cache.release_owner("main");
        assert!(cache.make_room(1).is_err());
        cache.release_owner("preview");
        assert!(cache.make_room(1).is_ok());
    }
}
