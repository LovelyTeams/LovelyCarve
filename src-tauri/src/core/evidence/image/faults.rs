//! faults implementation.
use super::*;

/// A span of the image that could not be read and was zero-filled instead of
/// aborting the operation. Surfaced for diagnostics; offsets are absolute
/// image bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BadRegion {
    pub offset: u64,
    pub len: u64,
}

/// Behaviour requested by an evidentiary operation when a source read must be
/// synthesized. Interactive browsing remains tolerant; hash and export IPCs
/// accept this policy explicitly so a caller can require fail-closed output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReadFaultPolicy {
    Strict,
    #[default]
    ZeroFillAndReport,
}

/// Structured audit payload returned by hash/export commands and the
/// `image_bad_regions` query. Regions are normalized, sorted, and coalesced.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadFaultReport {
    pub bad_regions: Vec<BadRegion>,
    pub synthetic_bytes: u64,
    pub read_faults_detected: bool,
}

impl ReadFaultReport {
    pub fn from_regions(regions: Vec<BadRegion>) -> Self {
        let bad_regions = normalize_bad_regions(regions);
        let synthetic_bytes = bad_regions
            .iter()
            .fold(0u64, |total, region| total.saturating_add(region.len));
        Self {
            read_faults_detected: !bad_regions.is_empty(),
            bad_regions,
            synthetic_bytes,
        }
    }
}

thread_local! {
    pub(super) static READ_FAULT_COLLECTIONS: RefCell<Vec<Vec<BadRegion>>> = const { RefCell::new(Vec::new()) };
}

pub(super) struct ReadFaultCollectionScope {
    pub(super) active: bool,
}

impl ReadFaultCollectionScope {
    pub(super) fn new() -> Self {
        READ_FAULT_COLLECTIONS.with(|collections| {
            collections.borrow_mut().push(Vec::new());
        });
        Self { active: true }
    }

    pub(super) fn finish(mut self) -> ReadFaultReport {
        self.active = false;
        let regions = READ_FAULT_COLLECTIONS
            .with(|collections| collections.borrow_mut().pop().unwrap_or_default());
        ReadFaultReport::from_regions(regions)
    }
}

impl Drop for ReadFaultCollectionScope {
    fn drop(&mut self) {
        if self.active {
            READ_FAULT_COLLECTIONS.with(|collections| {
                let _ = collections.borrow_mut().pop();
            });
        }
    }
}

pub(super) fn collect_read_fault_intersections(
    regions: &[BadRegion],
    offset: u64,
    len: u64,
) -> Vec<BadRegion> {
    if len == 0 {
        return Vec::new();
    }
    let end = offset.saturating_add(len);
    regions
        .iter()
        .filter_map(|region| {
            let region_end = region.offset.saturating_add(region.len);
            let intersection_start = region.offset.max(offset);
            let intersection_end = region_end.min(end);
            (intersection_end > intersection_start).then_some(BadRegion {
                offset: intersection_start,
                len: intersection_end.saturating_sub(intersection_start),
            })
        })
        .collect()
}

/// `regions` of a read of `len` bytes at `from`, clipped to that read and moved
/// to the address space in which the same bytes start at `to`. Composite
/// devices report a member's synthesized spans at their own offsets this way.
pub fn rebase_bad_regions(regions: &[BadRegion], from: u64, len: u64, to: u64) -> Vec<BadRegion> {
    collect_read_fault_intersections(regions, from, len)
        .into_iter()
        .map(|region| BadRegion {
            offset: to.saturating_add(region.offset - from),
            len: region.len,
        })
        .collect()
}

pub(super) fn record_collected_read_faults(regions: Vec<BadRegion>) {
    if regions.is_empty() {
        return;
    }
    READ_FAULT_COLLECTIONS.with(|collections| {
        if let Some(current) = collections.borrow_mut().last_mut() {
            current.extend(regions);
        }
    });
}

pub(super) fn normalize_bad_regions(mut regions: Vec<BadRegion>) -> Vec<BadRegion> {
    regions.retain(|region| region.len > 0);
    regions.sort_by_key(|region| region.offset);
    let mut normalized: Vec<BadRegion> = Vec::with_capacity(regions.len());
    for region in regions {
        if let Some(last) = normalized.last_mut() {
            let last_end = last.offset.saturating_add(last.len);
            let region_end = region.offset.saturating_add(region.len);
            if region.offset <= last_end {
                last.len = last_end.max(region_end).saturating_sub(last.offset);
                continue;
            }
        }
        normalized.push(region);
    }
    normalized
}

/// Sector-sized granularity for skipping past an unreadable span. A single bad
/// 512-byte sector shouldn't poison the surrounding kilobytes.
pub(super) const FAULT_GRANULARITY: usize = 512;

/// Fills `buf` from `read_at`, tolerating bad sectors. On a read error the
/// faulting span (one [`FAULT_GRANULARITY`] sector) is zero-filled, logged,
/// and the fill continues past it — so one unreadable sector no longer aborts
/// an entire filesystem walk, search, or carve. Mirrors the short-read /
/// zero-fill behaviour of TSK's image layer. Gaps are zeroed explicitly
/// because bulk [`DiskImage::read_into`] calls pass the caller's reused
/// hash / export buffer straight through; skipping would leave stale bytes
/// from the previous chunk where the report claims zeros.
/// Returns the number of bytes filled (including zero-filled gaps; a trailing
/// real EOF stops the fill early so the caller can truncate) together with the
/// absolute-offset spans that were fabricated as zeros. The caller records
/// those spans so the fabrication stays forensically auditable rather than
/// living only in a transient log line — a re-read served from the block cache
/// would otherwise return the synthetic zeros indistinguishably from real data.
pub(super) fn tolerant_fill<F>(
    start: u64,
    buf: &mut [u8],
    mut read_at: F,
) -> (usize, Vec<BadRegion>)
where
    F: FnMut(u64, &mut [u8]) -> anyhow::Result<usize>,
{
    let mut filled = 0usize;
    let mut faults: Vec<BadRegion> = Vec::new();
    while filled < buf.len() {
        match read_at(start + filled as u64, &mut buf[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(err) => {
                let skip = FAULT_GRANULARITY.min(buf.len() - filled);
                let offset = start + filled as u64;
                tracing::warn!(
                    "image read error at offset {offset} (+{skip} bytes): {err}; zero-filling gap",
                );
                // Coalesce a run of adjacent bad sectors into one span.
                match faults.last_mut() {
                    Some(last) if last.offset + last.len == offset => last.len += skip as u64,
                    _ => faults.push(BadRegion {
                        offset,
                        len: skip as u64,
                    }),
                }
                buf[filled..filled + skip].fill(0);
                filled += skip;
            }
        }
    }
    (filled, faults)
}
