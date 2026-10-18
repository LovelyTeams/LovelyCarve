//! read implementation.
use super::*;

impl DiskImage {
    pub fn len(&self) -> u64 {
        self.source.len()
    }

    pub fn is_empty(&self) -> bool {
        self.source.len() == 0
    }

    /// Handle for one of several threads reading this image at once. The
    /// EWF / VMDK / VHD(X) / PMFX decoders serialize every read behind one
    /// lock, so each caller gets a privately reopened handle; every other
    /// source is shared. A failed reopen falls back to sharing (same bytes,
    /// serialized reads).
    pub fn concurrent_reader(self: &Arc<Self>) -> Arc<DiskImage> {
        let serialized = matches!(
            self.source,
            ImageSource::Ewf { .. }
                | ImageSource::Vmdk { .. }
                | ImageSource::VhdFamily { .. }
                | ImageSource::Pmfx { .. }
        );
        if !serialized {
            return Arc::clone(self);
        }
        match DiskImage::open(&self.path) {
            Ok(image) => Arc::new(image),
            Err(error) => {
                tracing::warn!(
                    "reopen {} for a concurrent reader failed, sharing the handle: {error:#}",
                    self.path.display()
                );
                Arc::clone(self)
            }
        }
    }

    pub fn read_range(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        self.require_source()?;
        let available = self.source.read_length(offset, len);
        if available == 0 {
            return Ok(Vec::new());
        }
        let mut out = vec![0u8; available];
        let filled = self.read_into(offset, &mut out)?;
        out.truncate(filled);
        Ok(out)
    }

    /// Fill a caller-owned buffer from the image byte stream, returning the
    /// number of bytes read. This is the allocation-light companion to
    /// [`Self::read_range`] for hot sequential I/O paths such as hashing,
    /// exporting and in-place BitLocker reads.
    pub fn read_into(&self, offset: u64, dst: &mut [u8]) -> anyhow::Result<usize> {
        let (start, filled) = self.fill_from_source(offset, dst)?;
        self.collect_known_faults_in_range(start, filled as u64);
        Ok(filled)
    }

    /// [`Self::read_range`] for a composite device that reads this image as
    /// one of its members: the recorded synthesized spans intersecting the
    /// bytes read come back as absolute image offsets instead of going to the
    /// current thread's read-fault collection scope, whose offsets belong to
    /// the device the operation reads. They are still recorded on this image.
    pub fn read_range_traced(
        &self,
        offset: u64,
        len: usize,
    ) -> anyhow::Result<(Vec<u8>, Vec<BadRegion>)> {
        self.require_source()?;
        let available = self.source.read_length(offset, len);
        if available == 0 {
            return Ok((Vec::new(), Vec::new()));
        }
        let mut out = vec![0u8; available];
        let (start, filled) = self.fill_from_source(offset, &mut out)?;
        out.truncate(filled);
        Ok((out, self.known_faults_in_range(start, filled as u64)))
    }

    /// Fill `dst` from the source and record the spans the backend had to
    /// fabricate. Returns the requested start offset and the bytes filled.
    fn fill_from_source(&self, offset: u64, dst: &mut [u8]) -> anyhow::Result<(u64, usize)> {
        self.require_source()?;
        let image_len = self.source.len();
        let start = offset;
        // Bulk sequential reads (hash / export / search, typically 1–16 MiB)
        // bypass the small located cache: they would evict every hot MFT /
        // hex block and split one backend call into hundreds of 64 KiB ones.
        let cache = self
            .block_cache
            .as_ref()
            .filter(|_| dst.len() < BULK_READ_BYPASS_BYTES);
        let Some(cache) = cache else {
            // Non-cached sources (and bulk reads) go straight through; still
            // record any fabricated bad-sector spans so they stay auditable.
            let (filled, faults) = self.source.read_into_range_traced(offset, dst)?;
            self.record_faults(faults);
            return Ok((start, filled));
        };

        let available = image_len.saturating_sub(start).min(dst.len() as u64) as usize;
        if available == 0 {
            return Ok((start, 0));
        }
        let end = start + available as u64;

        // Serve the request from 64 KiB-aligned blocks: a hit copies the slice
        // out, a miss reads exactly one aligned block from the FFI backend and
        // caches it so neighbouring small reads (MFT walk, hex scroll) reuse
        // the same decompress. The cache lock is held only for the slice
        // copy / insert, never across the backend read.
        let mut filled = 0usize;
        let mut block = start - (start % CACHE_BLOCK);
        while block < end {
            let block_len = CACHE_BLOCK.min(image_len - block) as usize;
            // Record fabricated zero spans on the miss owner only — they become
            // part of the cached block, so a later waiter/cache hit does not
            // re-observe (or double-record) them.
            let (data, faults) = cache.get_or_insert_with(block, || {
                let (data, faults) = self.source.read_range_traced(block, block_len)?;
                let data: Arc<[u8]> = data.into();
                Ok((data, faults))
            })?;
            self.record_faults(faults);
            // Intersect [start,end) with this block [block, block+data.len()).
            let block_end = block + data.len() as u64;
            let copy_from = start.max(block);
            let copy_to = end.min(block_end);
            if copy_to > copy_from {
                let lo = (copy_from - block) as usize;
                let hi = (copy_to - block) as usize;
                let dst_start = (copy_from - start) as usize;
                let dst_end = dst_start + (hi - lo);
                dst[dst_start..dst_end].copy_from_slice(&data[lo..hi]);
                filled = filled.max(dst_end);
            }
            // A short block (EOF / bad-sector truncation) ends the loop.
            if (data.len() as u64) < CACHE_BLOCK {
                break;
            }
            block += CACHE_BLOCK;
        }
        Ok((start, filled))
    }

    /// Runs an operation while collecting source-read faults intersecting the
    /// bytes actually requested through [`Self::read_range`] on the current
    /// thread. This gives hash/export commands an operation-level audit report
    /// without changing every filesystem adapter to return physical extents.
    ///
    /// The collector is thread-local on purpose: all filesystem export loops
    /// run inside one blocking worker thread, and nested scopes stay isolated.
    pub fn collect_read_faults<T>(&self, operation: impl FnOnce() -> T) -> (T, ReadFaultReport) {
        let scope = ReadFaultCollectionScope::new();
        let output = operation();
        let report = scope.finish();
        (output, report)
    }

    /// Merges newly-discovered bad-sector spans into the auditable registry,
    /// coalescing with an existing adjacent span. Kept cheap: faults are rare
    /// and the list is consulted only when a report enumerates them.
    pub(super) fn record_faults(&self, faults: Vec<BadRegion>) {
        if faults.is_empty() {
            return;
        }
        let Ok(mut regions) = self.bad_regions.lock() else {
            return;
        };
        regions.extend(faults);
        *regions = normalize_bad_regions(std::mem::take(&mut *regions));
    }

    /// Absolute-offset spans that were zero-filled because the backend could
    /// not read them. Empty when every read so far has succeeded. Lets a report
    /// or the UI flag which bytes of an extracted artifact are fabricated zeros
    /// rather than genuine disk content (forensic chain-of-custody).
    pub fn bad_regions(&self) -> Vec<BadRegion> {
        self.bad_regions
            .lock()
            .map(|regions| regions.clone())
            .unwrap_or_default()
    }

    /// Whether a previously recorded synthesized span intersects
    /// `[offset, offset + len)`. Used by strict hash/export operations after
    /// each read, including cache hits whose zero-fill fault was recorded by an
    /// earlier request.
    pub fn has_bad_region_in_range(&self, offset: u64, len: u64) -> bool {
        let end = offset.saturating_add(len);
        self.bad_regions.lock().is_ok_and(|regions| {
            regions.iter().any(|region| {
                let region_end = region.offset.saturating_add(region.len);
                region.offset < end && region_end > offset
            })
        })
    }

    pub(super) fn collect_known_faults_in_range(&self, offset: u64, len: u64) {
        record_collected_read_faults(self.known_faults_in_range(offset, len));
    }

    /// Recorded synthesized spans intersecting `[offset, offset + len)`.
    fn known_faults_in_range(&self, offset: u64, len: u64) -> Vec<BadRegion> {
        if len == 0 {
            return Vec::new();
        }
        self.bad_regions
            .lock()
            .map(|regions| collect_read_fault_intersections(&regions, offset, len))
            .unwrap_or_default()
    }
}
