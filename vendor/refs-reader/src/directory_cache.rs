use crate::{corrupt, FileEntry, Result};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const CACHE_BYTES: usize = 16 * 1024 * 1024;
const CACHE_DIRECTORIES: usize = 128;

#[derive(Default)]
pub(crate) struct DirectoryCache(Mutex<VecDeque<CachedDirectory>>);

struct CachedDirectory {
    object_id: u64,
    entries: Arc<Vec<FileEntry>>,
    weight: usize,
}

impl DirectoryCache {
    pub(crate) fn get(&self, object_id: u64) -> Result<Option<Arc<Vec<FileEntry>>>> {
        let mut cache = self
            .0
            .lock()
            .map_err(|_| corrupt("ReFS directory cache poisoned"))?;
        let Some(index) = cache.iter().position(|entry| entry.object_id == object_id) else {
            return Ok(None);
        };
        let entry = cache
            .remove(index)
            .ok_or_else(|| corrupt("ReFS directory cache position missing"))?;
        let entries = Arc::clone(&entry.entries);
        cache.push_back(entry);
        Ok(Some(entries))
    }

    pub(crate) fn insert(&self, object_id: u64, entries: Arc<Vec<FileEntry>>) -> Result<()> {
        let weight = entries.iter().try_fold(0usize, |total, entry| {
            total
                .checked_add(std::mem::size_of::<FileEntry>())
                .and_then(|total| total.checked_add(entry.value.len()))
                .and_then(|total| total.checked_add(entry.name.len()))
                .and_then(|total| {
                    total.checked_add(entry.symlink_target.as_ref().map_or(0, String::len))
                })
                .ok_or_else(|| corrupt("ReFS directory cache size overflow"))
        })?;
        if weight > CACHE_BYTES {
            return Ok(());
        }
        let mut cache = self
            .0
            .lock()
            .map_err(|_| corrupt("ReFS directory cache poisoned"))?;
        cache.retain(|entry| entry.object_id != object_id);
        while cache.len() >= CACHE_DIRECTORIES
            || cache.iter().map(|entry| entry.weight).sum::<usize>() + weight > CACHE_BYTES
        {
            if cache.pop_front().is_none() {
                break;
            }
        }
        cache.push_back(CachedDirectory {
            object_id,
            entries,
            weight,
        });
        Ok(())
    }
}
