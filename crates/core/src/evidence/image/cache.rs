//! cache implementation.
use super::*;

/// Block size for the located read cache (see [`BlockCache`]). 64 KiB matches
/// the unit EWF/VMDK decompress per chunk, so one miss warms a span that
/// many small MFT / partition / hex reads then reuse.
pub(super) const CACHE_BLOCK: u64 = 64 * 1024;

/// `read_into` requests at least this large skip the located cache.
pub(super) const BULK_READ_BYPASS_BYTES: usize = 1024 * 1024;

/// Default located-cache capacity for each FFI-backed image.
pub(super) const DEFAULT_CACHE_CAPACITY_MIB: usize = 2;
pub(super) const MIN_CACHE_CAPACITY_MIB: usize = 1;
pub(super) const MAX_CACHE_CAPACITY_MIB: usize = 256;
pub(super) const CACHE_CAPACITY_ENV: &str = "LOVELYCARVE_FFI_BLOCK_CACHE_MIB";

pub(super) fn configured_block_cache_slot_count() -> usize {
    let raw = std::env::var(CACHE_CAPACITY_ENV).ok();
    let capacity_mib = normalize_block_cache_capacity_mib(raw.as_deref());
    if let Some(raw) = raw.as_deref() {
        let trimmed = raw.trim();
        match trimmed.parse::<usize>() {
            Ok(parsed) if parsed == capacity_mib => {}
            Ok(_) => tracing::warn!(
                "{CACHE_CAPACITY_ENV}={trimmed:?} is outside the supported {MIN_CACHE_CAPACITY_MIB}..={MAX_CACHE_CAPACITY_MIB} MiB range; using {capacity_mib} MiB"
            ),
            Err(_) => tracing::warn!(
                "{CACHE_CAPACITY_ENV}={trimmed:?} is not a valid MiB value; using {capacity_mib} MiB"
            ),
        }
    }
    block_cache_slot_count_from_mib(capacity_mib)
}

pub(super) fn normalize_block_cache_capacity_mib(raw: Option<&str>) -> usize {
    let Some(raw) = raw else {
        return DEFAULT_CACHE_CAPACITY_MIB;
    };
    raw.trim()
        .parse::<usize>()
        .map(|value| value.clamp(MIN_CACHE_CAPACITY_MIB, MAX_CACHE_CAPACITY_MIB))
        .unwrap_or(DEFAULT_CACHE_CAPACITY_MIB)
}

pub(super) fn block_cache_slot_count_from_mib(capacity_mib: usize) -> usize {
    let bytes = capacity_mib.saturating_mul(1024 * 1024);
    bytes.div_ceil(CACHE_BLOCK as usize).max(1)
}

/// Thread-safe located block cache for FFI-backed images. The inner cache is
/// protected by a small mutex and tracks in-flight misses by block id, so a
/// burst of concurrent reads for the same 64 KiB chunk performs one backend
/// read / decompression and wakes the remaining waiters when the result is
/// cached.
pub(super) struct SharedBlockCache {
    pub(super) state: FairMutex<BlockCache>,
    pub(super) ready: Condvar,
}

impl fmt::Debug for SharedBlockCache {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SharedBlockCache")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl SharedBlockCache {
    pub(super) fn new(slot_count: usize) -> Self {
        Self {
            state: FairMutex::new(BlockCache::new(slot_count)),
            ready: Condvar::new(),
        }
    }

    pub(super) fn get_or_insert_with<F>(
        &self,
        block: u64,
        fetch: F,
    ) -> anyhow::Result<(Arc<[u8]>, Vec<BadRegion>)>
    where
        F: FnOnce() -> anyhow::Result<(Arc<[u8]>, Vec<BadRegion>)>,
    {
        let mut state = self.state.lock();
        loop {
            if let Some(data) = state.get(block) {
                return Ok((data, Vec::new()));
            }
            if state.inflight.insert(block) {
                break;
            }
            self.ready.wait(&mut state);
        }
        drop(state);

        let fetched = fetch();
        let mut state = self.state.lock();
        state.inflight.remove(&block);
        match fetched {
            Ok((data, faults)) => {
                state.insert(block, Arc::clone(&data));
                self.ready.notify_all();
                Ok((data, faults))
            }
            Err(error) => {
                self.ready.notify_all();
                Err(error)
            }
        }
    }
}

/// Located block read cache for FFI-backed images (EWF / VMDK / VHDX / PMFX),
/// where every miss costs a decompress / sparse-map lookup. Plain mmap-backed
/// sources (Raw / Dmg / Docker) skip the cache — they are already zero-copy
/// page-cache reads. Keyed by 64 KiB-aligned block index with a clock (second-
/// chance) eviction policy. Guarded by [`SharedBlockCache`], kept separate from
/// the FairMutex serialising the underlying reader so a cache hit never waits
/// on an unrelated in-flight decompress.
#[derive(Debug)]
pub(super) struct BlockCache {
    pub(super) slots: Vec<Option<CacheSlot>>,
    pub(super) hand: usize,
    pub(super) inflight: HashSet<u64>,
}

#[derive(Debug)]
pub(super) struct CacheSlot {
    pub(super) block: u64,
    pub(super) data: Arc<[u8]>,
    pub(super) referenced: bool,
}

impl BlockCache {
    pub(super) fn new(slot_count: usize) -> Self {
        Self {
            slots: (0..slot_count.max(1)).map(|_| None).collect(),
            hand: 0,
            inflight: HashSet::new(),
        }
    }

    pub(super) fn get(&mut self, block: u64) -> Option<Arc<[u8]>> {
        for slot in self.slots.iter_mut().flatten() {
            if slot.block == block {
                slot.referenced = true;
                return Some(Arc::clone(&slot.data));
            }
        }
        None
    }

    pub(super) fn insert(&mut self, block: u64, data: Arc<[u8]>) {
        // Clock eviction: advance the hand, giving each referenced slot one
        // second chance before reuse; fill empties immediately.
        loop {
            match &mut self.slots[self.hand] {
                None => {
                    self.slots[self.hand] = Some(CacheSlot {
                        block,
                        data,
                        referenced: false,
                    });
                    self.hand = (self.hand + 1) % self.slots.len();
                    return;
                }
                Some(slot) if slot.referenced => {
                    slot.referenced = false;
                    self.hand = (self.hand + 1) % self.slots.len();
                }
                Some(_) => {
                    self.slots[self.hand] = Some(CacheSlot {
                        block,
                        data,
                        referenced: false,
                    });
                    self.hand = (self.hand + 1) % self.slots.len();
                    return;
                }
            }
        }
    }
}
