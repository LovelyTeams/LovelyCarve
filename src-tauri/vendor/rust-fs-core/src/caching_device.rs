//! Small LRU read-cache decorator. Caches only block-aligned, block-sized
//! reads; everything else passes through. Writes invalidate any overlapping
//! cached entries.

use crate::block::{BlockDevice, BlockRead};
use crate::error::Result;
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::ThreadId;

/// The largest block size a cache will accept.
///
/// `block()` allocates a whole block eagerly, and the `.min(size)` clamp
/// that keeps a short device working bounds that allocation by the device
/// rather than by anything sane — so a declared block size of 2^40 over a
/// 1 TB image is a request for a terabyte. A ceiling is the only thing
/// standing between a number off a disk and that allocation.
///
/// 64 MiB is far above anything a real filesystem declares — SquashFS tops
/// out at 1 MiB, and the block sizes every other driver here uses are
/// measured in kilobytes — and far below a size that could exhaust a host.
pub const MAX_BLOCK_SIZE: u64 = 64 * 1024 * 1024;

/// LRU read-cache wrapper.
///
/// # It caches a READ device, and writes through one only if it has one
///
/// This took an `Arc<dyn BlockDevice>` — the read *write* trait — and
/// every driver in this family mounts a volume through an
/// `Arc<dyn BlockRead>`. So a read-only mount could not wrap it at all,
/// and four of the six drivers used no cache: not by choice, but
/// because it was not expressible.
///
/// The read path never needed to write. It holds the read half now, and
/// the writable half only when the caller had one to give:
/// [`CachingDevice::new`] for a device that can be written,
/// [`CachingDevice::read_only`] for one that cannot. A write to a cache
/// built the second way is [`crate::Error::ReadOnly`], which is what the
/// underlying device would have said.
pub struct CachingDevice {
    inner: Arc<dyn BlockRead>,
    /// The same device again, present only when it can be written. Held
    /// separately rather than as one handle so that "can this be
    /// written" is a property of the type rather than a flag someone
    /// has to remember to check.
    writable: Option<Arc<dyn BlockDevice>>,
    block_size: u64,
    /// NOT IN `CacheState`, BECAUSE IT IS NOT STATE.
    ///
    /// It is a construction parameter, written once and never mutated,
    /// and it used to live behind the mutex. Every read therefore took
    /// the lock to compare `spanned` against it — including the reads
    /// that were about to be passed straight through and never touch
    /// the cache at all. On a hit the lock was taken twice, once here
    /// and once inside `block()`, to serve bytes already in memory.
    ///
    /// A plain field beside `block_size` makes the bypass test lock-free
    /// and says what the value is: fixed at construction, like the block
    /// size next to it.
    capacity: usize,
    state: Mutex<CacheState>,
    /// Signalled every time a block leaves [`CacheState::in_flight`],
    /// whether its fetch succeeded or failed.
    ///
    /// One condition variable for all blocks rather than one per block.
    /// A waiter wakes on any fetch completing and re-checks its own
    /// block, so a wake it did not want costs it a lock and a scan of a
    /// list bounded by the number of threads fetching. A map of
    /// per-block condvars would avoid those wakes and has to be built,
    /// looked up and torn down on every miss — the ordinary path — to
    /// save work on the rare one.
    fetched: Condvar,
}

/// One cached block, and its neighbours in recency order.
///
/// `newer`/`older` are indices into [`Lru::slots`], not pointers, so
/// the list is intrusive without being unsafe. A slot's index is stable
/// for as long as the node lives, which is what lets the index map
/// point at it.
struct Node {
    block_start: u64,
    data: Arc<Vec<u8>>,
    /// Toward the head: more recently used. `None` at the head.
    newer: Option<usize>,
    /// Toward the tail: less recently used. `None` at the tail.
    older: Option<usize>,
}

/// The cached blocks, in recency order, with O(1) lookup AND O(1)
/// promotion.
///
/// # Why not a `VecDeque` any more
///
/// It was one, and a hit cost `iter().position(...)` -- a linear scan
/// comparing offsets -- followed by `VecDeque::remove(pos)`, which
/// shifts every element on the shorter side of `pos` to close the gap.
/// Both halves are linear in the number of entries, so the cache got
/// slower the larger it was asked to be. Measured on this crate under
/// `--release`, working set equal to capacity so the steady state is
/// ~100% hits, with a `CountingDevice` underneath proving no device
/// traffic at all (`device_reads = 0` at every capacity, so the whole
/// difference is the cache's own bookkeeping):
///
/// ```text
/// capacity   us/read   vs capacity 8
///        8    0.0157             1x
///       64    0.0319           2.0x
///      512    0.1924          12.3x
///     4096    1.3306            85x
/// ```
///
/// A cached read at capacity 4096 cost 85 times what the same cached
/// read cost at capacity 8, and 8x more capacity from 512 to 4096 cost
/// 6.9x more per read -- linear, which is what `capacity/2` comparisons
/// plus `capacity/2` tuple moves predicts.
///
/// The capacities that matter are already past the knee, which is what
/// ruled out the other candidate fix of documenting a ceiling:
/// `am-fs-erofs` defaults to 512 metadata blocks, and one 3 MiB file
/// read there touches 768 blocks -- about a millisecond of pure
/// scanning to deliver bytes already in memory.
///
/// # Why not a `HashMap` alone
///
/// A map fixes the lookup and leaves the promotion: the entry still has
/// to move to the head of a recency order, and in a `VecDeque` that is
/// still `remove` plus `push_front`, still O(n), and it invalidates
/// every index the map is holding. The recency order has to be a linked
/// list for the promotion to be O(1), and then the map indexes into it.
struct Lru {
    /// Slab. `None` is a free slot, kept rather than compacted so live
    /// indices stay valid.
    slots: Vec<Option<Node>>,
    /// Slots to reuse before growing `slots`.
    free: Vec<usize>,
    index: HashMap<u64, usize>,
    /// Most recently used.
    newest: Option<usize>,
    /// Least recently used: the eviction end.
    oldest: Option<usize>,
}

impl Lru {
    fn with_capacity(capacity: usize) -> Self {
        Lru {
            slots: Vec::with_capacity(capacity),
            free: Vec::new(),
            index: HashMap::with_capacity(capacity),
            newest: None,
            oldest: None,
        }
    }

    fn len(&self) -> usize {
        self.index.len()
    }

    /// Take `i` out of the recency order, leaving the node in its slot.
    fn unlink(&mut self, i: usize) {
        let (newer, older) = {
            let n = self.slots[i].as_ref().expect("unlink of a free slot");
            (n.newer, n.older)
        };
        match newer {
            Some(j) => self.slots[j].as_mut().expect("newer is live").older = older,
            None => self.newest = older,
        }
        match older {
            Some(j) => self.slots[j].as_mut().expect("older is live").newer = newer,
            None => self.oldest = newer,
        }
        let n = self.slots[i].as_mut().expect("unlink of a free slot");
        n.newer = None;
        n.older = None;
    }

    /// Put `i` at the head of the recency order. It must not be linked.
    fn link_newest(&mut self, i: usize) {
        let old_head = self.newest;
        {
            let n = self.slots[i].as_mut().expect("link of a free slot");
            n.newer = None;
            n.older = old_head;
        }
        if let Some(j) = old_head {
            self.slots[j].as_mut().expect("head is live").newer = Some(i);
        } else {
            self.oldest = Some(i);
        }
        self.newest = Some(i);
    }

    /// The block's data, promoted to most-recently-used. Two hash
    /// lookups and a constant number of pointer writes, whatever the
    /// capacity.
    fn get(&mut self, block_start: u64) -> Option<Arc<Vec<u8>>> {
        let i = *self.index.get(&block_start)?;
        let data = self.slots[i]
            .as_ref()
            .expect("indexed slot is live")
            .data
            .clone();
        if self.newest != Some(i) {
            self.unlink(i);
            self.link_newest(i);
        }
        Some(data)
    }

    /// Drop one block if it is held. Returns whether it was.
    fn remove(&mut self, block_start: u64) -> bool {
        let Some(i) = self.index.remove(&block_start) else {
            return false;
        };
        self.unlink(i);
        self.slots[i] = None;
        self.free.push(i);
        true
    }

    /// Insert at the head, evicting the least recently used first if
    /// the cache is already at `capacity`.
    ///
    /// EVICT-THEN-INSERT UNCONDITIONALLY, which is the sequence the
    /// `VecDeque` version used (`pop_back` under `len() >= capacity`,
    /// then `push_front`). It matters at `capacity == 0`, where both
    /// spellings leave exactly one entry held rather than none: a
    /// behaviour worth preserving deliberately rather than changing
    /// while moving house.
    fn insert(&mut self, block_start: u64, data: Arc<Vec<u8>>, capacity: usize) {
        // A RE-INSERT REPLACES RATHER THAN DUPLICATING. No caller does
        // this today -- `block()` consults `get` under the same lock
        // immediately before -- but a second slot for one block would
        // leave the first linked in the recency list and unreachable
        // through the index: a leak that also makes the list longer
        // than the index, which is the kind of drift that shows up
        // later as an eviction of something still held.
        self.remove(block_start);
        if self.len() >= capacity {
            if let Some(oldest) = self.oldest {
                let victim = self.slots[oldest]
                    .as_ref()
                    .expect("oldest is live")
                    .block_start;
                self.remove(victim);
            }
        }
        let node = Node {
            block_start,
            data,
            newer: None,
            older: None,
        };
        let i = match self.free.pop() {
            Some(i) => {
                self.slots[i] = Some(node);
                i
            }
            None => {
                self.slots.push(Some(node));
                self.slots.len() - 1
            }
        };
        self.index.insert(block_start, i);
        self.link_newest(i);
    }

    fn clear(&mut self) {
        self.slots.clear();
        self.free.clear();
        self.index.clear();
        self.newest = None;
        self.oldest = None;
    }

    /// Drop every block for which `keep` is false.
    ///
    /// Linear in the number of entries, like the `retain` it replaces,
    /// and deliberately so: this runs per WRITE, not per read, and the
    /// blocks to drop have to be found by looking at all of them.
    fn retain_blocks(&mut self, keep: impl Fn(u64) -> bool) {
        let doomed: Vec<u64> = self.index.keys().copied().filter(|b| !keep(*b)).collect();
        for b in doomed {
            self.remove(b);
        }
    }

    /// Most-recently-used first. Tests only: the recency ORDER is the
    /// thing a linked list can get wrong in ways a hit rate cannot see.
    #[cfg(test)]
    fn recency_order(&self) -> Vec<u64> {
        let mut out = Vec::with_capacity(self.len());
        let mut cur = self.newest;
        while let Some(i) = cur {
            let n = self.slots[i].as_ref().expect("live");
            out.push(n.block_start);
            cur = n.older;
        }
        out
    }

    /// Least-recently-used first, walked the other way. Tests only, and
    /// the reason it exists is that a singly-consistent list passes
    /// every forward walk while being broken backwards -- which is the
    /// half eviction uses.
    #[cfg(test)]
    fn recency_order_reversed(&self) -> Vec<u64> {
        let mut out = Vec::with_capacity(self.len());
        let mut cur = self.oldest;
        while let Some(i) = cur {
            let n = self.slots[i].as_ref().expect("live");
            out.push(n.block_start);
            cur = n.newer;
        }
        out
    }
}

struct CacheState {
    /// Fixed-capacity LRU; head is most-recently used. The capacity
    /// itself is [`CachingDevice::capacity`] — it never changes, so it
    /// is not kept under the lock.
    ///
    /// An index plus an intrusive recency list rather than a
    /// `VecDeque`: see [`Lru`] for the measurement that decided it.
    entries: Lru,
    hits: u64,
    misses: u64,
    /// Bumped by every invalidation. A miss records it before it lets go
    /// of the lock to read the device, and the insert on the way back in
    /// is refused if it has moved — see `CachingDevice::block`.
    generation: u64,
    /// The blocks some thread is reading from the device right now.
    ///
    /// `generation` fences a miss against an *invalidation*; this fences
    /// it against another *miss*. Nothing used to: two threads missing
    /// the same block both read the device and both inserted, so a
    /// capacity-4 cache could end up holding four copies of one block
    /// having evicted the other three to make room. The more contention,
    /// the worse it got, which is the opposite of what a cache is for.
    ///
    /// A `Vec` scanned linearly, like `entries` beside it. Its length is
    /// the number of blocks being fetched concurrently, not the number
    /// cached, so it is small in exactly the cases the scan would matter.
    ///
    /// EACH FETCH CARRIES THE THREAD DOING IT, so that a thread can ask
    /// whether it is holding a fetch of its own before it waits for
    /// anybody else's. Waiting for another thread is the point; waiting
    /// while holding a fetch is how a cycle forms, whether the block
    /// waited on is this thread's own or two links away round a ring of
    /// re-entrant threads. See `block`.
    in_flight: Vec<(u64, ThreadId)>,
}

impl CachingDevice {
    /// Cache a device that can be written. Writes invalidate the
    /// entries they overlap and go through to `inner`.
    ///
    /// The invalidation holds against concurrent readers as well as
    /// sequential ones: once `write_at` has returned, no later read can
    /// be served pre-write bytes from this cache, including a read that
    /// was already in flight when the write began. What such a read
    /// *returns* is still either side of the write — that is what racing
    /// means — but it is not remembered.
    ///
    /// `block_size` must be non-zero and no larger than
    /// [`MAX_BLOCK_SIZE`]; see [`CachingDevice::read_only`] for why that
    /// is enforced at first use rather than here.
    ///
    /// `capacity` is documented on [`CachingDevice::read_only`]; it means
    /// the same here, including that `0` still caches one block.
    pub fn new(inner: Arc<dyn BlockDevice>, block_size: u64, capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: inner.clone(),
            writable: Some(inner),
            block_size,
            capacity,
            state: Mutex::new(CacheState {
                entries: Lru::with_capacity(capacity),
                hits: 0,
                misses: 0,
                generation: 0,
                in_flight: Vec::new(),
            }),
            fetched: Condvar::new(),
        })
    }

    /// Cache a device that is only ever read.
    ///
    /// The case every driver here actually has: a volume mounted for
    /// reading, behind a `BlockRead` that was never a `BlockDevice`.
    ///
    /// `block_size` must be non-zero and no larger than
    /// [`MAX_BLOCK_SIZE`]. Construction cannot refuse — it returns
    /// `Arc<Self>`, not `Result` — so a block size outside that range is
    /// refused by every read and every write instead, with an error
    /// naming the offending size.
    ///
    /// `capacity` is a count of **blocks**, not bytes: at most that many
    /// entries of up to `block_size` bytes each are held, so the memory
    /// bound is their product and is the caller's to choose. There is no
    /// ceiling; promotion and eviction are O(1) at any capacity.
    ///
    /// **`capacity = 0` does not disable the cache.** It holds one entry,
    /// so a repeated single-block read is still served as a hit, and two
    /// alternating blocks thrash it (#124). The behaviour is deliberate
    /// and pinned, not an oversight. A caller that wants no cache at zero
    /// should not construct one, and this constructor cannot do that for
    /// it because it returns `Arc<Self>`:
    ///
    /// ```text
    /// let dev: Arc<dyn BlockRead> = if blocks == 0 {
    ///     dev
    /// } else {
    ///     CachingDevice::read_only(dev, block_size, blocks)
    /// };
    /// ```
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use fs_core::{BlockRead, CachingDevice, CountingDevice};
    /// # struct Mem(Vec<u8>);
    /// # impl BlockRead for Mem {
    /// #     fn read_at(&self, offset: u64, buf: &mut [u8]) -> fs_core::Result<()> {
    /// #         let o = offset as usize;
    /// #         buf.copy_from_slice(&self.0[o..o + buf.len()]);
    /// #         Ok(())
    /// #     }
    /// #     fn size_bytes(&self) -> u64 { self.0.len() as u64 }
    /// # }
    /// let device = Arc::new(CountingDevice::new(Arc::new(Mem(vec![7; 512 * 8]))));
    /// let cache = CachingDevice::read_only(device.clone(), 512, 0);
    /// let mut buf = [0u8; 16];
    /// cache.read_at(0, &mut buf).unwrap();
    /// cache.read_at(8, &mut buf).unwrap();
    /// // A capacity of zero served the second read from the cache.
    /// assert_eq!(device.reads(), 1);
    /// assert_eq!(cache.stats(), (1, 1));
    /// ```
    pub fn read_only(inner: Arc<dyn BlockRead>, block_size: u64, capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            inner,
            writable: None,
            block_size,
            capacity,
            state: Mutex::new(CacheState {
                entries: Lru::with_capacity(capacity),
                hits: 0,
                misses: 0,
                generation: 0,
                in_flight: Vec::new(),
            }),
            fetched: Condvar::new(),
        })
    }

    /// `(hits, misses)`, in that order.
    ///
    /// Both count **block lookups inside the cache**, not reads of this
    /// device and not reads of `inner` (#123):
    ///
    /// - a hit is a block served from the cache, including one that was
    ///   waiting on another thread's fetch of the same block;
    /// - a miss is a block this cache fetched from `inner`, so `misses` is
    ///   the number of fetches the cache itself made.
    ///
    /// A read the cache declines to serve moves **neither** counter. A
    /// read reaching past the end of `inner`, and a read spanning enough
    /// blocks that caching it would sweep the cache (more than one block,
    /// and more than half of `capacity`), go straight to `inner`; an empty
    /// read, or one refused for its block size, touches nothing. So `hits + misses` is not the number of `read_at` calls,
    /// `misses` is a lower bound on the reads `inner` saw, and
    /// `hits / (hits + misses)` is a rate over the reads the cache served,
    /// which leaves out every read it chose not to. To count what reached
    /// the device, put a [`CountingDevice`](crate::CountingDevice) under
    /// the cache.
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use fs_core::{BlockRead, CachingDevice, CountingDevice};
    /// # struct Mem(Vec<u8>);
    /// # impl BlockRead for Mem {
    /// #     fn read_at(&self, offset: u64, buf: &mut [u8]) -> fs_core::Result<()> {
    /// #         let o = offset as usize;
    /// #         buf.copy_from_slice(&self.0[o..o + buf.len()]);
    /// #         Ok(())
    /// #     }
    /// #     fn size_bytes(&self) -> u64 { self.0.len() as u64 }
    /// # }
    /// let device = Arc::new(CountingDevice::new(Arc::new(Mem(vec![7; 512 * 32]))));
    /// let cache = CachingDevice::read_only(device.clone(), 512, 8);
    /// let mut one = [0u8; 16];
    /// cache.read_at(0, &mut one).unwrap(); // miss: fetched
    /// cache.read_at(8, &mut one).unwrap(); // hit
    /// let mut big = vec![0u8; 512 * 6];
    /// cache.read_at(0, &mut big).unwrap(); // 6 blocks > capacity / 2: bypassed
    /// assert_eq!(cache.stats(), (1, 1));
    /// assert_eq!(device.reads(), 2); // the bypassed read is not a miss
    /// ```
    pub fn stats(&self) -> (u64, u64) {
        let s = self.state.lock().unwrap();
        (s.hits, s.misses)
    }

    pub fn invalidate_all(&self) {
        let mut s = self.state.lock().unwrap();
        s.entries.clear();
        s.generation = s.generation.wrapping_add(1);
    }

    fn invalidate_range(state: &mut CacheState, start: u64, end: u64, block_size: u64) {
        state.entries.retain_blocks(|off| {
            let block_end = off.saturating_add(block_size);
            off >= end || block_end <= start
        });
        // BUMPED WHETHER OR NOT ANYTHING WAS DROPPED. The counter is not a
        // record of what this sweep removed; it is a fence a concurrent
        // miss can compare itself against, and a miss that is mid-flight
        // over this range holds no entry for the sweep to find.
        state.generation = state.generation.wrapping_add(1);
    }

    /// One invalidation sweep, taking and releasing the lock.
    fn invalidate_for_write(&self, start: u64, end: u64) {
        let mut s = self.state.lock().unwrap();
        let bs = self.block_size;
        Self::invalidate_range(&mut s, start, end, bs);
    }

    /// Refuse a block size the cache cannot work with.
    ///
    /// # Why this is checked here and not in the constructors
    ///
    /// It belongs in the constructors, and they cannot express it: both
    /// return `Arc<Self>` rather than `Result`, and that signature is
    /// published API in eleven sibling crates. Making them fallible to
    /// catch a case no correct caller hits would be a breaking change to
    /// all of them. So the refusal happens at first use instead, which
    /// costs two comparisons against an immutable field per call and
    /// turns both failures into an error the caller can handle.
    ///
    /// # Why a block size is worth checking at all
    ///
    /// Zero divides by zero on the first read, and integer division by
    /// zero panics unconditionally — it is not governed by
    /// `overflow-checks`, so a release build dies too. An absurd value
    /// reaches `vec![0u8; len]` in `block()` with `len` bounded only by
    /// the device. Both numbers come off a disk: a driver reads its block
    /// size from a superblock and passes it through, so a truncated,
    /// fuzzed or hostile image reaches this.
    fn check_block_size(&self) -> Result<()> {
        if self.block_size == 0 {
            return Err(crate::error::Error::Custom(
                "cache block size is zero".to_string(),
            ));
        }
        if self.block_size > MAX_BLOCK_SIZE {
            return Err(crate::error::Error::Custom(format!(
                "cache block size {} exceeds the {MAX_BLOCK_SIZE}-byte ceiling",
                self.block_size
            )));
        }
        Ok(())
    }
}

impl CachingDevice {
    /// The cached block at `block_start`, fetching it if it is not held.
    ///
    /// # A MISS THAT OVERLAPPED AN INVALIDATION IS NOT CACHED
    ///
    /// The device read below runs with the lock released — holding a mutex
    /// across I/O would serialise every reader, which is the whole reason
    /// the lock is dropped. That leaves a window: a write can sweep the
    /// cache and land on the device while this read is in flight, and the
    /// bytes in hand are then the ones the device held *before* the write.
    /// Inserting them puts a stale entry in a cache the sweep has already
    /// gone past, and nothing would ever invalidate it again.
    ///
    /// So the miss records the generation counter before it lets go, and
    /// declines to insert if any invalidation has happened since. The
    /// caller still gets the bytes that were read — a read racing a write
    /// may legitimately see either side of it — but the cache does not keep
    /// them.
    ///
    /// The check is deliberately coarse: the counter is global rather than
    /// per range, so a write to an unrelated block also costs this miss its
    /// insert. Writes are far rarer than reads in these drivers, and the
    /// price of being conservative is one extra device read.
    fn block(&self, block_start: u64) -> Result<Arc<Vec<u8>>> {
        // ONE FETCH PER BLOCK, NOT ONE PER MISSING THREAD.
        //
        // The lock has to be released for the device read -- holding a
        // mutex across I/O would serialise every reader, which is what
        // `perf: reads are positioned, and no longer take a lock`
        // deliberately stopped doing. But releasing it used to mean N
        // threads missing the same block all read the device and all
        // inserted, so the cache held N copies of one block and had
        // evicted up to N-1 others to store bytes it already had.
        //
        // So a thread that is going to fetch says so first, under the
        // lock. A thread that finds someone else already fetching its
        // block waits for them and then looks again, rather than making
        // a second trip to the device for bytes already on their way.
        //
        // The loop is not a spin: `wait` blocks until a fetch finishes,
        // and re-checking from the top afterwards is what makes it
        // correct against a spurious wake, an invalidation that landed
        // in the meantime, and a fetch that failed and left nothing.
        //
        // A THREAD THAT IS ALREADY FETCHING SOMETHING NEVER WAITS,
        // WHOEVER OWNS THE BLOCK IT WANTS. WHICH IS WHY THE MARKER
        // CARRIES AN OWNER.
        //
        // A device whose `read_at` reads back through the cache that
        // wraps it re-enters this method while its own fetch is still
        // outstanding. The narrow case is that it asks for the very
        // block it is fetching, and waiting there is waiting for a
        // fetch this thread is holding up: a deadlock. That shape is
        // visible from the owner alone.
        //
        // THE GENERAL CASE IS NOT, and asking only "is this fetch
        // mine" cannot see it. Thread A fetches X and re-enters for Y;
        // thread B fetches Y and re-enters for X. Neither finds its own
        // id against the block it wants, so both wait -- each for a
        // fetch the other is holding up, on a condition variable only a
        // completed fetch can signal. Nothing completes. It is the same
        // deadlock one link longer, and there is no length at which
        // comparing the owner to the caller starts to notice.
        //
        // So the question is not "is this fetch mine" but "am I holding
        // one at all". A cycle needs every thread in it to be both
        // holding a fetch and waiting for another one; a thread holding
        // nothing cannot be waited on, so it cannot be in a cycle. A
        // thread therefore waits only when it holds nothing, and that
        // removes every cycle rather than the shortest one. It needs no
        // wait-for graph and no new state to do it -- only a wider
        // question asked of the list already being scanned.
        //
        // WHAT IT COSTS is a redundant device read in one narrow case:
        // a re-entrant thread that wants a block another thread is
        // fetching, where waiting would in fact have been safe. That is
        // the same trade the self case already made, and only a device
        // that re-enters can reach it. For every other caller nothing
        // changes at all, because a thread holds a fetch here only
        // while it is inside `read_at` on the device, and a device that
        // cannot call back in leaves this false on every ordinary miss.
        //
        // THE GUARD IS HERE BECAUSE OF WHAT A HANG COSTS, not because
        // re-entrancy is expected. It is not: no device in this crate
        // can do it today, since none holds a handle to what wraps it,
        // and stacked caches are unaffected because each has its own
        // lock and its own list. But `CachingDevice` accepts any
        // `BlockRead`, so the constraint is a property of the current
        // devices rather than of this API -- a device with a slower
        // tier behind it that consults a cache on miss would violate it
        // the day it is written. And the drivers built on this crate
        // run as filesystem extensions, where a deadlock inside a
        // fetch is not a visible failure but a spinning cursor on a
        // volume that cannot be unmounted, with no error and no log
        // line to say which mount is stuck. Four lines and a thread id
        // turn that into a redundant read.
        let generation_at_miss = {
            let mut s = self.state.lock().unwrap();
            loop {
                if let Some(data) = s.entries.get(block_start) {
                    s.hits += 1;
                    return Ok(data);
                }
                let mine = std::thread::current().id();
                // Any fetch of this thread's, not just one of this
                // block: holding any of them is what makes waiting
                // unsafe. See the cycle argument above.
                let holding_a_fetch = s.in_flight.iter().any(|(_, owner)| *owner == mine);
                let being_fetched = s.in_flight.iter().any(|(o, _)| *o == block_start);
                if being_fetched && !holding_a_fetch {
                    // Counted as neither yet. It becomes a hit when the
                    // fetch it is waiting for lands, and a miss if that
                    // fetch fails and this thread has to do it instead
                    // -- so `hits + misses` stays the number of calls,
                    // and `misses` stays the number of device fetches.
                    s = self.fetched.wait(s).unwrap();
                    continue;
                }
                s.misses += 1;
                s.in_flight.push((block_start, mine));
                break s.generation;
            }
        };

        // FROM HERE EVERY EXIT MUST CLEAR THE MARKER, including the `?`
        // below and a panic inside the device. A fetch that vanished
        // without clearing it would leave every later reader of that
        // block waiting for a thread that is gone.
        let _fetch = FetchGuard {
            device: self,
            block_start,
            owner: std::thread::current().id(),
        };

        // THE LAST BLOCK OF A DEVICE IS OFTEN SHORT, and asking the
        // device for a whole one past its end is an error rather than a
        // short read. A SquashFS image is 4 KiB and its declared block
        // size 128 KiB; without this clamp, caching such an image failed
        // on the first read it ever made.
        let size = self.inner.size_bytes();
        let end = block_start.saturating_add(self.block_size).min(size);
        let len = end.saturating_sub(block_start) as usize;
        let mut block = vec![0u8; len];
        self.inner.read_at(block_start, &mut block)?;
        let data = Arc::new(block);

        // Inserted BEFORE `_fetch` clears the marker and wakes the
        // waiters, so a thread woken by this fetch finds the entry
        // rather than an empty cache and a free marker.
        {
            let mut s = self.state.lock().unwrap();
            // ALREADY HELD? Then return that copy and drop this one
            // rather than holding the same block twice. Only a
            // re-entrant fetch reaches this -- one fetch per block per
            // thread is the rule for every other caller -- and without
            // it a re-entrant read puts two entries in for one block,
            // which is the defect this commit exists to remove, in a
            // new place.
            if let Some(held) = s.entries.get(block_start) {
                return Ok(held);
            }
            if s.generation == generation_at_miss {
                s.entries.insert(block_start, data.clone(), self.capacity);
            }
        }
        Ok(data)
    }
}

impl BlockRead for CachingDevice {
    /// # A read is served from the blocks it falls in, whatever its size
    ///
    /// This used to serve a read only when it was **exactly one aligned
    /// block**, and pass everything else through untouched — including
    /// reads of bytes it was already holding.
    ///
    /// The drivers almost never read a whole block. Measured on
    /// `am-fs-xfs` against a fixture with a 4096-byte block size, the
    /// average read during a directory walk was **1040 bytes**: inodes
    /// are read at inode size and group headers at sector size, so
    /// roughly three quarters of reads missed by construction.
    ///
    /// # What it costs
    ///
    /// A 512-byte read of an uncached block now fetches 4096. That is a
    /// trade of bytes for calls, and it is the right way round for these
    /// drivers: the block being fetched is the one holding the inode,
    /// and the next inode read is very often in it.
    ///
    /// # Where it still passes through
    ///
    /// A read larger than the cache's own capacity would evict
    /// everything to hold one answer, so anything spanning more blocks
    /// than a useful fraction of the cache goes straight to the device.
    /// File data is read in large pieces and would otherwise push out
    /// the metadata this exists to keep.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        self.check_block_size()?;
        if buf.is_empty() {
            return Ok(());
        }

        // THE END OF THE READ IS COMPUTED ONCE, CHECKED, AND BEFORE ANY
        // DIVISION.
        //
        // This used to be `offset + buf.len()`, twice, unchecked, above
        // the only bounds check the function has — which then made the
        // same sum a third time with `saturating_add`, so the line that
        // knew the sum could overflow sat below the two that did not.
        //
        // An offset here is computed by a driver from an on-disk field:
        // an extent pointer, an inode block number, a directory offset.
        // A wild one is an ordinary thing to be handed off a corrupt or
        // hostile image rather than a mistake in the caller, which is
        // the same argument `slice.rs` was fixed on.
        //
        // A sum that does not fit in a `u64` cannot name a byte on any
        // device, so it is refused here rather than forwarded. `got: 0`
        // because nothing was transferred — the convention the slice
        // adapters already use for a read refused before it starts.
        let Some(end) = offset.checked_add(buf.len() as u64) else {
            return Err(crate::error::Error::ShortRead {
                offset,
                want: buf.len(),
                got: 0,
            });
        };

        // A READ RUNNING PAST THE END OF THE DEVICE IS THE DEVICE'S TO
        // REFUSE. Serving it from clamped blocks would hand back a short
        // answer with no error, which is worse than the failure the
        // caller would otherwise have seen.
        if end > self.inner.size_bytes() {
            return self.inner.read_at(offset, buf);
        }

        let bs = self.block_size;
        let first = offset / bs;
        let last = (end - 1) / bs;
        let spanned = (last - first + 1) as usize;

        // A read big enough to sweep the cache is not worth caching.
        //
        // A SINGLE BLOCK IS NEVER "BIG ENOUGH", however small the cache.
        // Without that clause a cache of one block bypasses every read
        // it is ever given -- one block is more than half of one block --
        // so the smallest cache anybody can ask for is the one that
        // silently does nothing.
        //
        // AND THE TEST TAKES NO LOCK. `capacity` is a plain field on the
        // device, so a read that is about to bypass the cache decides
        // that without ever contending for the mutex -- see the field's
        // own comment.
        if spanned > 1 && spanned.saturating_mul(2) > self.capacity {
            return self.inner.read_at(offset, buf);
        }

        let mut done = 0usize;
        for index in first..=last {
            let block_start = index * bs;
            let block = self.block(block_start)?;
            // Where this block overlaps what was asked for.
            let from = (offset.max(block_start) - block_start) as usize;
            // Bounded by what the BLOCK holds rather than by the block
            // size, since the last one may be short.
            let take = (block.len().saturating_sub(from)).min(buf.len() - done);
            if take == 0 {
                break;
            }
            buf[done..done + take].copy_from_slice(&block[from..from + take]);
            done += take;
        }

        // A BUFFER THIS DID NOT FILL IS A FAILURE, NOT A SUCCESS.
        //
        // The loop can run out of bytes before the buffer is full: a
        // cached block is clamped to the device's size when it is
        // fetched, so a block held from when the device measured smaller
        // is short, and the guard above — which asks the device its size
        // a second time — lets the read through as in range. Reporting
        // `Ok` there hands back exactly the short answer with no error
        // that the guard's own comment refuses.
        //
        // It cannot fire while `size_bytes` is stable, which is the
        // contract `BlockRead` now states: the guard bounds the read by
        // the device, and the blocks between them cover everything up to
        // that bound. So this is the device breaking its promise being
        // caught rather than believed.
        //
        // `BlockDevice::set_len` IS THE ONE SANCTIONED WAY THAT NUMBER
        // MOVES, and this is still the right answer when a read races
        // one. `set_len` sweeps the blocks its new length makes short,
        // either side of the device call, so a read that is not
        // concurrent with one can never land here; a read that IS
        // concurrent with one may, and an error naming what it got beats
        // a short buffer reported as success. The defect this catches is
        // a `set_len` that moved the length and swept nothing, which is
        // exactly how a growth API re-creates #70 -- see this type's
        // `set_len`.
        if done != buf.len() {
            return Err(crate::error::Error::ShortRead {
                offset,
                want: buf.len(),
                got: done,
            });
        }
        Ok(())
    }

    fn size_bytes(&self) -> u64 {
        self.inner.size_bytes()
    }
}

impl BlockDevice for CachingDevice {
    /// # THE CACHE IS INVALIDATED EVEN IF THE WRITE THEN FAILS
    ///
    /// Deliberately: dropping entries the write would have made stale
    /// costs a re-read, while keeping them past a write that half
    /// succeeded serves bytes the device no longer holds.
    ///
    /// That applies to a write the device refused, not to one this type
    /// refused on the device's behalf. A write rejected because there is
    /// no writable half, or because the block size is unusable, never
    /// reaches the device and so cannot have staled anything — those
    /// return above both sweeps and leave the cache exactly as it was.
    ///
    /// # AND IT IS INVALIDATED TWICE, ONCE EITHER SIDE OF THE DEVICE
    ///
    /// One sweep before the write is not enough. Between it and the
    /// device write landing, a concurrent [`CachingDevice::read_at`] can
    /// miss, fetch pre-write bytes, and insert them behind the sweep —
    /// an entry the sweep has already gone past and nothing else would
    /// ever drop. The second sweep is what closes that window, together
    /// with the generation check on the miss path that refuses such an
    /// insert outright.
    ///
    /// Both are needed, and each covers what the other cannot. A read
    /// that began BEFORE this write is caught by the counter, because it
    /// recorded the generation before the first sweep bumped it. A read
    /// that begins AFTER that sweep records the bumped value, so the
    /// counter agrees with it and only the second sweep drops what it
    /// inserted.
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        // NO WRITABLE HALF IS ANSWERED FIRST, ABOVE EVERY OTHER REFUSAL.
        //
        // This used to sit below the block-size check and below the first
        // sweep, and both positions were wrong for their own reason.
        //
        // Below the block-size check, a read-only cache whose block size
        // came off a damaged disk answered a write with `Error::Custom`
        // rather than the `Error::ReadOnly` this type's own documentation
        // promises. That is not a cosmetic difference in the variant name:
        // `stream.rs` maps `ReadOnly` to `PermissionDenied` and `Custom` to
        // `io::Error::other`, so a caller branching on `PermissionDenied`
        // to say "this volume is read-only" reported an uncategorised
        // failure instead — on a read-only mount of a damaged image, which
        // is exactly the configuration where a bad block size turns up.
        //
        // Below the first sweep, every refused write emptied the region it
        // was refused for, so a read-only cache paid a re-read for a write
        // that could never have staled anything.
        //
        // The block-size check does NOT move down to meet it. See below.
        let Some(writable) = self.writable.as_ref() else {
            return Err(crate::error::Error::ReadOnly);
        };

        // BEFORE EITHER SWEEP, AND THAT ORDERING IS LOAD-BEARING.
        //
        // A sweep cannot be done correctly with a block size of zero:
        // `invalidate_range` computes `block_end = off + block_size`, so a
        // zero block size makes `block_end == off` and turns the retain
        // predicate into `*off >= end || *off <= start`, which KEEPS
        // entries the write has made stale. Sweeping first and refusing
        // afterwards would therefore do the one thing this type must never
        // do, on the way to reporting an error.
        //
        // Refusing here is also a position that cannot break the two-sweep
        // guarantee above. The invariant that guarantee rests on is "if the
        // device was written, both sweeps ran" — and returning at this point
        // means the device is never reached, so nothing was written and
        // nothing needs sweeping. An early return anywhere BELOW would skip
        // the second sweep after a write that may have landed, which is
        // exactly the window that fix closed. The read-only refusal above
        // is safe for the same reason and no other: it too returns before
        // either sweep and touches neither the cache nor the device.
        self.check_block_size()?;
        let end = offset.saturating_add(buf.len() as u64);
        self.invalidate_for_write(offset, end);
        let result = writable.write_at(offset, buf);
        // Unconditionally, for the same reason the first sweep is
        // unconditional: a write that failed may still have landed.
        self.invalidate_for_write(offset, end);
        result
    }

    fn flush(&self) -> Result<()> {
        match self.writable.as_ref() {
            Some(writable) => writable.flush(),
            // Nothing was written, so there is nothing to flush. An
            // error here would make a caller that flushes defensively
            // fail on a read-only volume.
            None => Ok(()),
        }
    }

    fn is_writable(&self) -> bool {
        self.writable.as_ref().is_some_and(|w| w.is_writable())
    }

    /// # THE CACHE'S VIEW HAS TO MOVE WITH THE DEVICE, OR THIS IS #70
    ///
    /// `size_bytes` here forwards to the device, so the NUMBER follows a
    /// grow for free. The entries do not. `block()` clamps every fetch to
    /// `size_bytes()` at the moment it runs — "the last block of a device
    /// is often short", as its own comment says — so an entry fetched
    /// before the grow ends where the device used to. Leave it in place
    /// and a later read across the old end is served from it, runs out of
    /// bytes, and comes back `ShortRead` for a region the device now
    /// holds perfectly well.
    ///
    /// Measured, with this method forwarding to the device and sweeping
    /// nothing: a 6000-byte file behind a 4096-byte cache, the short
    /// block warmed, `set_len(8192)`, then one read across the old end:
    ///
    /// ```text
    /// ShortRead { offset: 5000, want: 3000, got: 1000 }
    /// ```
    ///
    /// That is rust-fs-core#70's signature exactly — the grow succeeded,
    /// the file and the reported size agreed, and only a LATER CACHED
    /// READ found the hole. It is why the growth API is a change to this
    /// file as much as to `block.rs`.
    ///
    /// # FROM `min(old, new)` UPWARDS, WHICHEVER WAY THE LENGTH WENT
    ///
    /// A grow only makes the block STRADDLING the old end wrong, and that
    /// block starts below the old end — so the sweep has to begin at the
    /// old length, not at the first block boundary above it, and
    /// `invalidate_range` drops any block whose end passes `start`.
    ///
    /// A shrink makes everything from the new length up wrong instead.
    /// Taking the smaller of the two covers both without asking which
    /// happened, and the upper bound is `u64::MAX` because "the rest of
    /// the device" is what changed in either case.
    ///
    /// # `min` RATHER THAN `old`, AND NO TEST HERE CAN TELL THEM APART
    ///
    /// Said plainly because the alternative is a comment claiming a
    /// guarantee nobody measured. Sweeping from the OLD length alone
    /// leaves the blocks between the two lengths cached after a shrink,
    /// and that suite is EXIT=0 -- every arm in `tests/device_growth.rs`
    /// passes with `min` removed.
    ///
    /// It passes because nothing can read those entries. `read_at`
    /// forwards any read whose end passes `size_bytes()` straight to the
    /// device rather than serving it from blocks, so while the device is
    /// short they are unreachable; and a later grow sweeps from the
    /// smaller of ITS two lengths, which is the shrunk one, so they are
    /// dropped before they become reachable again.
    ///
    /// `min` ships anyway, and not for symmetry. The argument above rests
    /// on a bound in a DIFFERENT METHOD holding forever -- an entry that
    /// is stale but currently unreadable is one guard away from being
    /// stale and readable. This is the cheaper half of the invariant to
    /// state correctly, so it is stated correctly here rather than
    /// derived from somewhere else on every future read of this file.
    ///
    /// # AND IT IS SWEPT TWICE, ONCE EITHER SIDE, FOR `write_at`'S REASON
    ///
    /// One sweep before is not enough. Between it and the device call
    /// landing, a concurrent [`CachingDevice::read_at`] can miss, fetch a
    /// block clamped to the OLD length, and insert it behind the sweep.
    /// The second sweep closes that window, together with the generation
    /// check on the miss path. Each covers what the other cannot, in the
    /// same way and for the same reason `write_at` documents at length.
    ///
    /// Both run whether or not the device call succeeded, also for
    /// `write_at`'s reason: a `set_len` that failed may still have moved
    /// the file, and dropping entries needlessly costs a re-read while
    /// keeping stale ones serves bytes the device no longer has.
    ///
    /// # THE TWO REFUSALS ABOVE THE SWEEPS
    ///
    /// No writable half, and an unusable block size — the same pair
    /// `write_at` refuses on, in the same order, and above both sweeps
    /// for the same two reasons. `Error::ReadOnly` rather than
    /// `Error::Custom` when there is nothing to write, because
    /// [`crate::stream`] maps only the first to `PermissionDenied`; and
    /// the block-size check above the sweeps because `invalidate_range`
    /// cannot sweep correctly with a block size of zero, so sweeping
    /// first and refusing after would do the one thing this type must
    /// never do on the way to reporting an error. Neither refusal reaches
    /// the device, so neither can have staled anything.
    fn set_len(&self, new_len: u64) -> Result<()> {
        let Some(writable) = self.writable.as_ref() else {
            return Err(crate::error::Error::ReadOnly);
        };
        self.check_block_size()?;

        // The lower of the two lengths: below it nothing changed, at or
        // above it everything may have.
        let from = self.inner.size_bytes().min(new_len);
        self.invalidate_for_write(from, u64::MAX);
        let result = writable.set_len(new_len);
        self.invalidate_for_write(from, u64::MAX);
        result
    }

    /// The writable half's answer, or `false` when there is no writable
    /// half — a cache cannot grow a device it can only read.
    fn can_grow(&self) -> bool {
        self.writable.as_ref().is_some_and(|w| w.can_grow())
    }
}

/// Clears one block from [`CacheState::in_flight`] and wakes whoever is
/// waiting for it.
///
/// A guard rather than a line at the end of `block()`, because the
/// paths out of that method are not all the happy one: the device read
/// uses `?`, and a device is free to panic. Either would step over a
/// manual cleanup and strand every future reader of that block on a
/// fetch that no longer exists.
struct FetchGuard<'a> {
    device: &'a CachingDevice,
    block_start: u64,
    owner: ThreadId,
}

impl Drop for FetchGuard<'_> {
    fn drop(&mut self) {
        // REMOVES ONE ENTRY, NOT EVERY MATCH. `retain` would be wrong:
        // it reads as the same thing and would clear a second fetch of
        // this block that this guard does not own.
        //
        // A poisoned lock is left alone. Another thread panicked holding
        // it, so the cache is already unusable and every waiter will
        // surface the poison from its own `wait`; unwrapping here could
        // panic while a panic is already unwinding, which aborts.
        if let Ok(mut s) = self.device.state.lock() {
            if let Some(pos) = s
                .in_flight
                .iter()
                .position(|(o, owner)| *o == self.block_start && *owner == self.owner)
            {
                s.in_flight.remove(pos);
            }
        }
        // Outside the lock, and after it: waiters re-check the state, so
        // waking them before the marker was cleared would send them
        // straight back to sleep.
        self.device.fetched.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_device::Bytes;

    const BS: u64 = 512;

    fn backing() -> Arc<Bytes> {
        Arc::new(Bytes::new((0..4096u32).map(|i| i as u8).collect()))
    }

    /// THE CASE THAT COULD NOT BE EXPRESSED BEFORE: a device that is
    /// only ever read, wrapped in a cache.
    ///
    /// Every driver in this family mounts through a `BlockRead`, so
    /// this is not an exotic configuration — it is the ordinary one,
    /// and requiring `BlockDevice` is why four of the six drivers used
    /// no cache at all.
    #[test]
    fn a_read_only_device_can_be_cached() {
        let inner = backing();
        let cache = CachingDevice::read_only(inner, BS, 4);

        let mut first = vec![0u8; BS as usize];
        let mut again = vec![0u8; BS as usize];
        cache.read_at(0, &mut first).expect("first read");
        cache.read_at(0, &mut again).expect("second read");

        assert_eq!(first, again, "the cache must serve what the device held");
        assert_eq!(cache.stats(), (1, 1), "one hit after one miss");
    }

    /// THE CASE THE OLD HIT CONDITION MISSED: a read smaller than a
    /// block, of a block already held.
    ///
    /// Serving only exact aligned blocks meant the drivers' ordinary
    /// reads — an inode at inode size, a group header at sector size —
    /// went to the device every time, even when the block containing
    /// them was cached.
    #[test]
    fn a_read_smaller_than_a_block_is_served_from_it() {
        let cache = CachingDevice::read_only(backing(), BS, 8);

        let mut whole = vec![0u8; BS as usize];
        cache.read_at(0, &mut whole).expect("warm the block");
        assert_eq!(cache.stats(), (0, 1), "one miss to fetch it");

        // Four sub-block reads inside the block just fetched.
        for at in [0u64, 8, 100, 504] {
            let mut small = [0u8; 8];
            cache.read_at(at, &mut small).expect("sub-block read");
            assert_eq!(
                &small[..],
                &whole[at as usize..at as usize + 8],
                "the bytes must be the block's own, at the right offset"
            );
        }
        assert_eq!(cache.stats(), (4, 1), "four hits, and no further misses");
    }

    /// A read crossing a block boundary is stitched from both blocks,
    /// and each is cached.
    #[test]
    fn a_read_spanning_two_blocks_is_stitched() {
        let inner = backing();
        let mut direct = vec![0u8; 16];
        inner.read_at(BS - 8, &mut direct).expect("read it plainly");

        let cache = CachingDevice::read_only(backing(), BS, 8);
        let mut across = vec![0u8; 16];
        cache.read_at(BS - 8, &mut across).expect("spanning read");

        assert_eq!(across, direct, "the same bytes the device would give");
        assert_eq!(cache.stats(), (0, 2), "one miss per block touched");

        cache.read_at(BS - 8, &mut across).expect("again");
        assert_eq!(cache.stats(), (2, 2), "and both are held now");
    }

    /// A read big enough to sweep the cache goes straight to the device.
    ///
    /// File data arrives in large pieces, and caching it would evict the
    /// metadata this exists to hold — the opposite of the point.
    #[test]
    fn a_read_that_would_sweep_the_cache_passes_through() {
        let cache = CachingDevice::read_only(backing(), BS, 4);
        let mut big = vec![0u8; (BS * 4) as usize];
        cache.read_at(0, &mut big).expect("a large read");
        assert_eq!(
            cache.stats(),
            (0, 0),
            "neither hit nor miss: it never consulted the cache"
        );
    }

    /// A device smaller than one block still reads.
    ///
    /// THE CASE THAT BROKE. `am-fs-squashfs` declares a block size from
    /// the archive's superblock -- 128 KiB is the usual -- and a small
    /// image is a few kilobytes whole. Fetching "the block at zero"
    /// asked the device for 128 KiB it did not have, which is an error
    /// rather than a short read, so opening such an image with a cache
    /// failed on the very first read.
    #[test]
    fn a_device_shorter_than_a_block_still_reads() {
        let tiny: Arc<Bytes> = Arc::new(Bytes::new((0..100u32).map(|i| i as u8).collect()));
        let cache = CachingDevice::read_only(tiny, BS, 4);

        let mut buf = vec![0u8; 40];
        cache
            .read_at(10, &mut buf)
            .expect("a read inside the device");
        assert_eq!(buf[0], 10, "the wrong bytes came back");
        assert_eq!(buf[39], 49);

        // And the second one is a hit, so the short block was cached
        // rather than merely tolerated.
        cache.read_at(10, &mut buf).expect("again");
        assert_eq!(cache.stats(), (1, 1));
    }

    /// A read running past the end of the device still fails.
    ///
    /// The clamp above must not turn "you asked for bytes that are not
    /// there" into a short answer with no error. That failure is
    /// invisible to the caller, which is the one kind this family of
    /// crates refuses to produce.
    #[test]
    fn a_read_past_the_end_is_still_an_error() {
        let tiny: Arc<Bytes> = Arc::new(Bytes::new(vec![0u8; 100]));
        let cache = CachingDevice::read_only(tiny, BS, 4);

        let mut buf = vec![0u8; 40];
        assert!(
            cache.read_at(80, &mut buf).is_err(),
            "80 + 40 is past the end of a 100-byte device"
        );
    }

    /// A READ THAT BYPASSES THE CACHE DOES NOT WAIT FOR THE CACHE LOCK.
    ///
    /// The state lock is held for the whole of the read, by a thread
    /// that is not doing the read. With the `capacity` comparison under
    /// that mutex the bypass cannot proceed and the receive times out;
    /// with it outside there is nothing to wait for. No timing
    /// threshold and no thread count, so nothing here is flaky on a
    /// loaded machine.
    #[test]
    fn a_bypassed_read_does_not_wait_for_the_cache_lock() {
        use std::sync::mpsc;
        use std::thread;
        use std::time::Duration;

        // Capacity 4 and a read spanning 4 blocks: 4 * 2 > 4, so this
        // read bypasses -- the same read
        // `a_read_that_would_sweep_the_cache_passes_through` makes.
        let cache = CachingDevice::read_only(backing(), BS, 4);
        let held = cache.state.lock().expect("nothing else holds it yet");

        let reader = Arc::clone(&cache);
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut big = vec![0u8; (BS * 4) as usize];
            let outcome = reader.read_at(0, &mut big);
            // Sent whatever it is: the point is that the read RETURNED.
            let _ = tx.send(outcome);
        });

        let outcome = rx.recv_timeout(Duration::from_secs(5)).expect(
            "a read that bypasses the cache must not block on the cache lock; \
             it timed out waiting for a mutex it has no reason to take",
        );
        outcome.expect("and the bypassed read itself must succeed");

        // Released only now, and it has to be explicit: `stats()` takes
        // the same lock, so leaving the guard to the end of scope would
        // deadlock the assertion below.
        drop(held);

        assert_eq!(
            cache.stats(),
            (0, 0),
            "it bypassed, so it neither hit nor missed"
        );
    }

    /// A cache over a read-only device says so, and refuses a write
    /// with the answer the device underneath would have given.
    #[test]
    fn writing_through_a_read_only_cache_is_refused() {
        let cache = CachingDevice::read_only(backing(), BS, 4);
        assert!(!cache.is_writable());
        assert!(matches!(
            cache.write_at(0, &[1u8; 8]),
            Err(crate::error::Error::ReadOnly)
        ));
        // And flushing is not an error: a caller that flushes
        // defensively must not fail on a volume it never wrote.
        assert!(cache.flush().is_ok());
    }
}

#[cfg(test)]
mod lru_tests {
    use super::*;

    fn block(n: u8) -> Arc<Vec<u8>> {
        Arc::new(vec![n; 4])
    }

    /// The list has to be walkable BOTH WAYS and agree with itself.
    ///
    /// A singly-consistent list passes every forward walk while being
    /// broken backwards -- and backwards is the half eviction uses, so
    /// the failure would surface as evicting the wrong block rather
    /// than as anything a hit rate could show.
    fn assert_consistent(lru: &Lru) {
        let forward = lru.recency_order();
        let mut backward = lru.recency_order_reversed();
        backward.reverse();
        assert_eq!(
            forward, backward,
            "the recency list disagrees with itself walked the other way"
        );
        assert_eq!(
            forward.len(),
            lru.len(),
            "the list holds {} nodes and the index {} -- they have drifted",
            forward.len(),
            lru.len()
        );
    }

    #[test]
    fn a_hit_promotes_to_most_recently_used() {
        let mut lru = Lru::with_capacity(4);
        for i in 0..4u64 {
            lru.insert(i * 100, block(i as u8), 4);
        }
        assert_eq!(lru.recency_order(), vec![300, 200, 100, 0]);
        assert!(lru.get(100).is_some());
        assert_eq!(lru.recency_order(), vec![100, 300, 200, 0]);
        assert_consistent(&lru);

        // Promoting what is already newest must not corrupt the ends.
        assert!(lru.get(100).is_some());
        assert_eq!(lru.recency_order(), vec![100, 300, 200, 0]);
        assert_consistent(&lru);

        // Nor must promoting the oldest.
        assert!(lru.get(0).is_some());
        assert_eq!(lru.recency_order(), vec![0, 100, 300, 200]);
        assert_consistent(&lru);
    }

    #[test]
    fn the_least_recently_used_is_what_gets_evicted() {
        let mut lru = Lru::with_capacity(3);
        for i in 0..3u64 {
            lru.insert(i * 100, block(i as u8), 3);
        }
        // Touch the oldest so it is no longer the victim.
        assert!(lru.get(0).is_some());
        lru.insert(999, block(9), 3);
        assert_eq!(lru.len(), 3);
        assert_eq!(
            lru.recency_order(),
            vec![999, 0, 200],
            "100 was least recently used and is the one that should be gone"
        );
        assert!(lru.get(100).is_none());
        assert_consistent(&lru);
    }

    #[test]
    fn evicted_slots_are_reused_rather_than_growing_the_slab() {
        let mut lru = Lru::with_capacity(2);
        for i in 0..20u64 {
            lru.insert(i, block(i as u8), 2);
            assert_consistent(&lru);
        }
        assert_eq!(lru.len(), 2);
        assert!(
            lru.slots.len() <= 3,
            "twenty inserts at capacity 2 left {} slots: freed slots are not being \
             reused, so the slab grows without bound",
            lru.slots.len()
        );
    }

    #[test]
    fn a_re_insert_replaces_and_does_not_leave_the_old_node_linked() {
        let mut lru = Lru::with_capacity(4);
        lru.insert(10, block(1), 4);
        lru.insert(20, block(2), 4);
        lru.insert(10, block(3), 4);
        assert_eq!(lru.len(), 1 + 1, "one entry per block, not one per insert");
        assert_eq!(lru.recency_order(), vec![10, 20]);
        assert_eq!(
            lru.get(10).as_deref().map(|v| v[0]),
            Some(3),
            "the newer data wins"
        );
        assert_consistent(&lru);
    }

    #[test]
    fn removing_and_retaining_keep_the_list_consistent() {
        let mut lru = Lru::with_capacity(8);
        for i in 0..8u64 {
            lru.insert(i, block(i as u8), 8);
        }
        assert!(lru.remove(0), "the tail");
        assert_consistent(&lru);
        assert!(lru.remove(7), "the head");
        assert_consistent(&lru);
        assert!(lru.remove(4), "the middle");
        assert_consistent(&lru);
        assert!(!lru.remove(4), "already gone");

        lru.retain_blocks(|b| b % 2 == 0);
        assert_consistent(&lru);
        assert_eq!(lru.recency_order(), vec![6, 2]);

        lru.clear();
        assert_eq!(lru.len(), 0);
        assert!(lru.recency_order().is_empty());
        assert_consistent(&lru);
    }

    /// `capacity == 0` holds exactly one entry, not none.
    ///
    /// That is what the `VecDeque` version did -- `pop_back` on an
    /// empty deque is a no-op, then `push_front` -- and it is
    /// preserved deliberately rather than changed while moving house.
    /// Pinned so the next person to touch `insert` finds out from a
    /// test rather than from a caller.
    #[test]
    fn capacity_zero_behaves_as_it_did_before() {
        let mut lru = Lru::with_capacity(0);
        lru.insert(1, block(1), 0);
        assert_eq!(lru.len(), 1);
        lru.insert(2, block(2), 0);
        assert_eq!(lru.len(), 1);
        assert_eq!(lru.recency_order(), vec![2]);
        assert_consistent(&lru);
    }
}
