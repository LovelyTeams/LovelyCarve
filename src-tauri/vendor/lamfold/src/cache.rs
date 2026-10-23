//! Immutable decompressed-block cache.
//!
//! Read-only media never changes, so a cache needs no invalidation logic — the
//! standard squashfs/erofs optimization is to decompress a block once and reuse
//! it. This is a byte-capacity LRU keyed by a frontend-chosen block id (a disc
//! offset, an erofs cluster index, …). Set `cap_bytes = 0` for the tightest
//! bootloader builds (every block re-read/re-decompressed, zero retained).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

struct Entry {
    data: Vec<u8>,
    last: u64,
}

/// A bounded LRU over decompressed blocks. Eviction is O(n) in the entry count,
/// which is fine for the small working sets a bootloader/embedded reader holds.
pub struct BlockCache {
    entries: BTreeMap<u64, Entry>,
    tick: u64,
    cap_bytes: usize,
    cur_bytes: usize,
}

impl BlockCache {
    /// Create a cache holding at most `cap_bytes` of decompressed data. `0`
    /// disables caching (every `insert` is dropped immediately).
    pub fn new(cap_bytes: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            tick: 0,
            cap_bytes,
            cur_bytes: 0,
        }
    }

    /// Look up a cached block, marking it most-recently-used.
    pub fn get(&mut self, key: u64) -> Option<&[u8]> {
        self.tick = self.tick.wrapping_add(1);
        let tick = self.tick;
        let entry = self.entries.get_mut(&key)?;
        entry.last = tick;
        Some(&entry.data)
    }

    /// Insert a decompressed block, evicting least-recently-used entries until it
    /// fits. A block larger than the whole cache is simply not retained (returned
    /// to the caller already; the cache just declines to keep it).
    pub fn insert(&mut self, key: u64, data: Vec<u8>) {
        if self.cap_bytes == 0 || data.len() > self.cap_bytes {
            return;
        }
        if let Some(old) = self.entries.remove(&key) {
            self.cur_bytes -= old.data.len();
        }
        while self.cur_bytes + data.len() > self.cap_bytes {
            match self.evict_one() {
                Some(freed) => self.cur_bytes -= freed,
                None => break,
            }
        }
        self.tick = self.tick.wrapping_add(1);
        self.cur_bytes += data.len();
        self.entries.insert(
            key,
            Entry {
                data,
                last: self.tick,
            },
        );
    }

    /// Current retained bytes (test/diagnostic).
    pub fn len_bytes(&self) -> usize {
        self.cur_bytes
    }

    fn evict_one(&mut self) -> Option<usize> {
        let victim = self
            .entries
            .iter()
            .min_by_key(|(_, e)| e.last)
            .map(|(k, _)| *k)?;
        self.entries.remove(&victim).map(|e| e.data.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn hit_and_miss() {
        let mut c = BlockCache::new(1024);
        assert!(c.get(1).is_none());
        c.insert(1, vec![0xAA; 100]);
        assert_eq!(c.get(1).unwrap().len(), 100);
        assert_eq!(c.get(1).unwrap()[0], 0xAA);
    }

    #[test]
    fn evicts_least_recently_used() {
        let mut c = BlockCache::new(250);
        c.insert(1, vec![1u8; 100]);
        c.insert(2, vec![2u8; 100]);
        // touch 1 so 2 becomes LRU
        assert!(c.get(1).is_some());
        c.insert(3, vec![3u8; 100]); // must evict to fit (100*3 > 250)
        assert!(c.get(2).is_none(), "2 was LRU and should be evicted");
        assert!(c.get(1).is_some());
        assert!(c.get(3).is_some());
        assert!(c.len_bytes() <= 250);
    }

    #[test]
    fn zero_cap_disables() {
        let mut c = BlockCache::new(0);
        c.insert(1, vec![0u8; 10]);
        assert!(c.get(1).is_none());
        assert_eq!(c.len_bytes(), 0);
    }

    #[test]
    fn reinsert_same_key_replaces() {
        let mut c = BlockCache::new(1024);
        c.insert(1, vec![1u8; 100]);
        c.insert(1, vec![2u8; 50]);
        assert_eq!(c.get(1).unwrap().len(), 50);
        assert_eq!(c.len_bytes(), 50);
    }
}
