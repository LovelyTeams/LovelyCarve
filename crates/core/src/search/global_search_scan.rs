//! 全局搜索的扫描工作池（[`crate::search::global_search`] 的 worker 侧）。
//!
//! 每个 worker 持一个 [`FsSession`]（卷按卷根缓存）与一份复用的扫描缓冲，从
//! 共享游标按原始行序领取工作单元（整源，或大源的一段），流式匹配，并在扫描
//! 窗口里顺手截取命中上下文，把构建好的命中记录交给 drain 线程编号落库。
//!
//! - **上下文随扫随取**：命中上下文（前 64 / 后 128 字节）直接从扫描窗口切出。
//!   carry 窗口至少留「前文 + 最长模式」字节，保证任何保留下来的命中前文都在
//!   窗口内；后文跨块时由下一块补齐。不再对每条命中回读文件——在不可随机读的
//!   文件系统上那是 O(命中偏移) 的重复流式读，命中密集时是全局搜索的头号开销。
//! - **分段直读**：可随机读的大源切段（[`ScanLayout`]，默认 64 MiB、≥ 两段才切）
//!   跨 worker 摊派，各段经 [`FsSession::stream_file_range`] 只读本段窗口。不可
//!   随机读的源整段扫描：每段都要从文件头流式跳读，总读量随段数平方增长。
//! - **每文件上限按文件序**：每块命中按偏移排序后计上限；分段各自保留前 N 条，
//!   末段按偏移归并再截断——整源与分段扫描选出同一批命中，与调度时序无关。
//! - **成批动态领取**：worker 用原子游标按行序整批领取相邻单元（≤64 个、≤4 MiB）。
//!   相邻文件的数据块往往相邻，同一 worker 连续读它们才能命中自己的 64 KiB 块
//!   缓存；逐个领取时相邻小文件落到不同 worker，每个都要整块读一次。大单元（分段、
//!   大文件）单独成批，分段仍摊到多个 worker；慢批次不会让后续单元空等。
//! - 读错误逐源记录、不致命；没有数据流的文件（[`MissingDataStream`]）按空文件
//!   处理，不算读错误。取消、首命中即停与段尾经哨兵错误中止流式读。

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use crate::filesystem::evidence_session::EvidenceSession;
use crate::filesystem::fs_dispatch::MissingDataStream;
use crate::filesystem::tree::fs_tree::FsSession;
use crate::search::global_search::GlobalSearchOptions;
use crate::search::global_search_store::GlobalSearchHit;
use crate::search::keyword::keyword_search::{ByteHit, MatchScratch, MatcherSet, SearchEncoding};

/// 一批最多的相邻单元数与字节数（见 [`WorkPlan`]）。
const BATCH_MAX_ITEMS: usize = 64;
const BATCH_MAX_BYTES: u64 = 4 * 1024 * 1024;
/// 命中上下文窗口与预览上限。
const CONTEXT_BEFORE: usize = 64;
const CONTEXT_AFTER: usize = 128;
const CONTEXT_PREVIEW_CHARS: usize = 180;
const CONTEXT_FULL_CHARS: usize = 2048;
const CONTEXT_FULL_LINES: usize = 24;

/// 扫描分块与大源分段尺寸。生产值见 [`ScanLayout::default`]；测试用小尺寸
/// 覆盖块边界与分段路径。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScanLayout {
    /// 流式读取分块（carry 窗口见 [`scan_carry_len`]）。
    pub chunk_bytes: usize,
    /// 大源分段长度；可随机读的源 ≥ 两段时切段跨 worker 摊派。
    pub segment_bytes: u64,
}

impl Default for ScanLayout {
    fn default() -> Self {
        Self {
            chunk_bytes: 4 * 1024 * 1024,
            segment_bytes: 64 * 1024 * 1024,
        }
    }
}

/// 一个可扫源：普通文件、`unalloc:`/`diskgap:` 虚拟节点。
#[derive(Debug, Clone)]
pub(crate) struct ScanSource {
    /// 报告与落库用的节点 id。
    pub node_id: String,
    /// 实际读取的节点 id（宿主 Docker 文件为其底层文件系统节点）；`None` =
    /// 与 `node_id` 相同。
    pub read_id: Option<String>,
    pub name: String,
    pub size_bytes: u64,
    /// 展示路径：文件为相对路径，虚拟节点为名称。
    pub path: String,
    pub volume_key: String,
    /// `file` | `unallocated` | `diskgap`。
    pub source_kind: &'static str,
}

impl ScanSource {
    pub(crate) fn read_id(&self) -> &str {
        self.read_id.as_deref().unwrap_or(&self.node_id)
    }
}

/// 一个可扫单元：整源，或大源的一段。
struct WorkItem {
    source_idx: usize,
    segment: Option<Segment>,
}

/// 按原始行序排好的工作单元与领取批次。
pub(crate) struct WorkPlan {
    items: Vec<WorkItem>,
    /// 相邻单元的半开区间 `[start, end)`，worker 整批领取。
    batches: Vec<(usize, usize)>,
    /// 零字节源数：不流式，但计入已完成。
    pub zero_byte_sources: u64,
}

impl WorkPlan {
    /// 工作单元数（大源每段一个）。
    pub(crate) fn item_count(&self) -> usize {
        self.items.len()
    }

    /// 领取批次数（worker 数的上限）。
    pub(crate) fn batch_count(&self) -> usize {
        self.batches.len()
    }
}

/// 大源的一段 `[start, start + len)`。
struct Segment {
    start: u64,
    len: u64,
    shared: Arc<SplitSourceState>,
}

/// 分段源的跨段共享状态：各段命中（已带上下文）汇总，末段统一产出记录。
struct SplitSourceState {
    pending_hits: Mutex<Vec<FileHit>>,
    remaining: AtomicUsize,
    total_hits: AtomicU64,
}

/// 一条待构建命中（文件绝对偏移 + 模式/编码 + 扫描时截取的上下文字节）。
struct FileHit {
    absolute: u64,
    len: usize,
    pattern_index: usize,
    encoding: SearchEncoding,
    /// 上下文原始字节，区间见 [`context_span`]（EOF 截断）；`None` = 扫描窗口没
    /// 覆盖到前文（只有长于 carry 窗口的正则命中会这样），收尾回读。
    context: Option<Vec<u8>>,
}

/// 扫描提前终止哨兵：段窗口完成 / 取消 / 首命中即停。
struct SegmentDone;
struct CancelMarker;
struct StopAfterFirstMarker;

macro_rules! sentinel_error {
    ($name:ident, $message:literal) => {
        impl std::fmt::Debug for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(stringify!($name))
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str($message)
            }
        }
        impl std::error::Error for $name {}
    };
}
sentinel_error!(SegmentDone, "segment window scanned");
sentinel_error!(CancelMarker, "global search cancelled");
sentinel_error!(StopAfterFirstMarker, "stop after first hit");

/// 每个 worker 复用的扫描缓冲（避免逐文件重分配 4 MiB 级缓冲）。
#[derive(Default)]
struct ScanScratch {
    carry: Vec<u8>,
    scan_buf: Vec<u8>,
    hits_buf: Vec<ByteHit>,
    matcher: MatchScratch,
}

/// 一个 worker 对一个工作单元的扫描产出（分段源的非末段不带记录）。只有带
/// 命中或读错误的单元才发给 drain 线程。
pub(crate) struct SourceOutcome {
    pub source_idx: usize,
    pub records: Vec<GlobalSearchHit>,
    pub hit_count: u64,
    pub read_error: bool,
}

/// 工作池共享的输入、游标与进度计数（drain 线程读计数上报进度）。
pub(crate) struct ScanPool<'a> {
    pub plan: &'a WorkPlan,
    pub sources: &'a [ScanSource],
    pub matchers: &'a MatcherSet,
    pub options: &'a GlobalSearchOptions,
    /// 合并模式表：关键词在前、正则在后（与 `MatcherSet` 编号一致）。
    pub patterns: &'a [String],
    pub keyword_count: usize,
    pub cancel_flag: &'a CancellationToken,
    /// 全局上限触顶 / worker 启动失败时置位，worker 不再领取新单元。
    pub stop_flag: &'a AtomicBool,
    pub layout: ScanLayout,
    pub next_batch: AtomicUsize,
    pub files_done: AtomicU64,
    pub files_scanned: AtomicU64,
}

/// 把源清单展开成工作单元（原始行序）并切成领取批次。可随机读的大源切段；
/// 是否可随机读用 `session` 对源做一次零长度探测。
pub(crate) fn build_work_plan(
    session: &mut FsSession<'_>,
    sources: &[ScanSource],
    options: &GlobalSearchOptions,
    layout: ScanLayout,
) -> WorkPlan {
    let segment_bytes = layout.segment_bytes.max(1);
    let mut work = Vec::with_capacity(sources.len());
    let mut zero_byte_sources = 0u64;
    for (source_idx, source) in sources.iter().enumerate() {
        if source.size_bytes == 0 {
            zero_byte_sources += 1;
            continue;
        }
        let split = !options.stop_after_first
            && source.size_bytes >= segment_bytes.saturating_mul(2)
            && supports_ranged_reads(session, source);
        if !split {
            work.push(WorkItem {
                source_idx,
                segment: None,
            });
            continue;
        }
        let shared = Arc::new(SplitSourceState {
            pending_hits: Mutex::new(Vec::new()),
            remaining: AtomicUsize::new(
                usize::try_from(source.size_bytes.div_ceil(segment_bytes)).unwrap_or(usize::MAX),
            ),
            total_hits: AtomicU64::new(0),
        });
        let mut start = 0u64;
        while start < source.size_bytes {
            let len = segment_bytes.min(source.size_bytes - start);
            work.push(WorkItem {
                source_idx,
                segment: Some(Segment {
                    start,
                    len,
                    shared: Arc::clone(&shared),
                }),
            });
            start += len;
        }
    }
    let batches = batch_ranges(&work, sources);
    WorkPlan {
        items: work,
        batches,
        zero_byte_sources,
    }
}

/// 把相邻单元切成批：每批至多 [`BATCH_MAX_ITEMS`] 个、合计不超过
/// [`BATCH_MAX_BYTES`]（超过上限的单个大单元单独成批）。
fn batch_ranges(work: &[WorkItem], sources: &[ScanSource]) -> Vec<(usize, usize)> {
    let mut batches = Vec::new();
    let mut start = 0usize;
    let mut bytes = 0u64;
    for (index, item) in work.iter().enumerate() {
        let len = item
            .segment
            .as_ref()
            .map_or(sources[item.source_idx].size_bytes, |segment| segment.len);
        let full = index > start
            && (index - start >= BATCH_MAX_ITEMS || bytes.saturating_add(len) > BATCH_MAX_BYTES);
        if full {
            batches.push((start, index));
            start = index;
            bytes = 0;
        }
        bytes = bytes.saturating_add(len);
    }
    if start < work.len() {
        batches.push((start, work.len()));
    }
    batches
}

/// 源能否随机读（决定大源是否切段）。探测失败按不可随机读处理：整段扫描时
/// 读错误照常逐源记录。
fn supports_ranged_reads(session: &mut FsSession<'_>, source: &ScanSource) -> bool {
    match session.read_range_direct(source.read_id(), 0, 0) {
        Ok(direct) => direct.is_some(),
        Err(err) => {
            tracing::debug!(
                "global_search ranged-read probe failed node={}: {err:#}",
                source.node_id
            );
            false
        }
    }
}

/// 毒锁恢复：worker panic 不应让整个搜索崩溃。
fn recover_poison<T>(result: std::sync::LockResult<T>) -> T {
    result.unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// worker 主循环：从游标整批领取单元扫描，整源直接产出记录；分段源由末段汇总
/// 产出。
pub(crate) fn scan_worker(
    pool: &ScanPool<'_>,
    image: &Arc<EvidenceSession>,
    tx: &SyncSender<SourceOutcome>,
) {
    let mut session = FsSession::new(image);
    let mut scratch = ScanScratch::default();
    loop {
        let batch = pool.next_batch.fetch_add(1, Ordering::Relaxed);
        let Some(&(start, end)) = pool.plan.batches.get(batch) else {
            return;
        };
        for item in &pool.plan.items[start..end] {
            if !scan_work_item(pool, &mut session, &mut scratch, item, tx) {
                return;
            }
        }
    }
}

/// 扫一个工作单元并把有内容的产出交给 drain 线程。返回 `false` 表示 worker 应当
/// 停止（取消、全局上限触顶或 drain 已退出）。
fn scan_work_item(
    pool: &ScanPool<'_>,
    session: &mut FsSession<'_>,
    scratch: &mut ScanScratch,
    item: &WorkItem,
    tx: &SyncSender<SourceOutcome>,
) -> bool {
    if pool.cancel_flag.is_cancelled() || pool.stop_flag.load(Ordering::Relaxed) {
        return false;
    }
    let source = &pool.sources[item.source_idx];
    if item
        .segment
        .as_ref()
        .is_none_or(|segment| segment.start == 0)
    {
        pool.files_done.fetch_add(1, Ordering::Relaxed);
        pool.files_scanned.fetch_add(1, Ordering::Relaxed);
    }
    let scan = scan_item(session, scratch, pool, source, item.segment.as_ref());
    let records = match &item.segment {
        None => build_records(session, pool, source, scan.stored, scan.hit_count),
        Some(segment) => {
            finish_segment(session, pool, source, segment, scan.stored, scan.hit_count)
        }
    };
    // 无命中、无读错误的单元（全盘扫描的绝大多数）不打扰 drain 线程：
    // 进度走原子计数，它们没有别的可汇报。
    if scan.hit_count > 0 || !records.is_empty() || scan.read_error {
        let outcome = SourceOutcome {
            source_idx: item.source_idx,
            records,
            hit_count: scan.hit_count,
            read_error: scan.read_error,
        };
        if tx.send(outcome).is_err() {
            return false;
        }
    }
    !scan.cancelled
}

/// 分段源的一段完成：命中并入共享状态；最后完成的一段按偏移归并、截到每文件
/// 上限并产出全部记录（命中数为各段之和）。
fn finish_segment(
    session: &mut FsSession<'_>,
    pool: &ScanPool<'_>,
    source: &ScanSource,
    segment: &Segment,
    mut stored: Vec<FileHit>,
    hit_count: u64,
) -> Vec<GlobalSearchHit> {
    let shared = &segment.shared;
    shared.total_hits.fetch_add(hit_count, Ordering::SeqCst);
    recover_poison(shared.pending_hits.lock()).append(&mut stored);
    if shared.remaining.fetch_sub(1, Ordering::SeqCst) != 1 {
        return Vec::new();
    }
    let total = shared.total_hits.load(Ordering::SeqCst);
    let mut hits = std::mem::take(&mut *recover_poison(shared.pending_hits.lock()));
    // 段区间互不相交：稳定排序后同偏移的命中保持段内扫描序。
    hits.sort_by_key(|hit| hit.absolute);
    hits.truncate(pool.options.per_file_cap.max(1) as usize);
    build_records(session, pool, source, hits, total)
}

/// 一个工作单元的扫描状态。
#[derive(Default)]
struct ItemScan {
    stored: Vec<FileHit>,
    /// 后文尚未取全的命中：`stored` 下标与上下文终点（文件绝对偏移）。
    pending: Vec<(usize, u64)>,
    hit_count: u64,
    /// 首命中即停已触发：只再补齐上下文，不再匹配。
    stop_requested: bool,
    read_error: bool,
    cancelled: bool,
}

impl ItemScan {
    /// 存一条命中并从当前窗口截取上下文；后文超出窗口末端时登记待补。
    fn store(&mut self, hit: &ByteHit, absolute: u64, window: &[u8], window_start: u64) {
        let (context_start, context_end) = context_span(absolute, hit.len);
        let window_end = window_start + window.len() as u64;
        // 匹配器只报窗口内的区间；`get` 兜底，异常区间退回收尾回读而不是 panic。
        let context = if context_start >= window_start {
            let from = (context_start - window_start) as usize;
            let to = (context_end.min(window_end) - window_start) as usize;
            window.get(from..to).map(<[u8]>::to_vec)
        } else {
            None
        };
        if context.is_some() && context_end > window_end {
            self.pending.push((self.stored.len(), context_end));
        }
        self.stored.push(FileHit {
            absolute,
            len: hit.len,
            pattern_index: hit.keyword_index,
            encoding: hit.encoding,
            context,
        });
    }

    /// 用新窗口补齐待补命中的后文。窗口从上一窗口末端之前（carry）开始，
    /// 已取到的字节末端总落在本窗口内。
    fn complete_pending(&mut self, window: &[u8], window_start: u64) {
        if self.pending.is_empty() {
            return;
        }
        let window_end = window_start + window.len() as u64;
        let stored = &mut self.stored;
        self.pending.retain(|&(index, context_end)| {
            let Some(hit) = stored.get_mut(index) else {
                return false;
            };
            let (context_start, _) = context_span(hit.absolute, hit.len);
            let Some(context) = hit.context.as_mut() else {
                return false;
            };
            let have_end = context_start + context.len() as u64;
            if have_end >= window_start && have_end < window_end {
                let from = (have_end - window_start) as usize;
                let to = (context_end.min(window_end) - window_start) as usize;
                if let Some(bytes) = window.get(from..to) {
                    context.extend_from_slice(bytes);
                }
            }
            context_start + (context.len() as u64) < context_end
        });
    }
}

/// 命中上下文的文件区间 `[start, end)`：前文 [`CONTEXT_BEFORE`] 字节（文件头截断），
/// 总长 `len + CONTEXT_BEFORE + CONTEXT_AFTER`（EOF 由读取方截断）。
fn context_span(absolute: u64, len: usize) -> (u64, u64) {
    let start = absolute.saturating_sub(CONTEXT_BEFORE as u64);
    (start, start + (len + CONTEXT_BEFORE + CONTEXT_AFTER) as u64)
}

/// 扫描 carry 窗口：匹配器自身的 carry（跨块命中不漏），且不小于「前文 + 最长
/// 模式」——完全落在 carry 内的命中已在上一窗口报告并被丢弃，保留下来的命中
/// 起点因此至少在窗口的第 [`CONTEXT_BEFORE`] 字节之后，前文必在窗口内。
fn scan_carry_len(matchers: &MatcherSet) -> usize {
    matchers
        .carry_len()
        .max(CONTEXT_BEFORE + matchers.max_pattern_len())
}

/// 扫一个工作单元（整源或一段）。分段从段前 carry 处开读：段前字节只充当
/// carry（去重与前文），命中起点须落在段内；读过段尾 carry 且上下文取全即停。
fn scan_item(
    session: &mut FsSession<'_>,
    scratch: &mut ScanScratch,
    pool: &ScanPool<'_>,
    source: &ScanSource,
    segment: Option<&Segment>,
) -> ItemScan {
    let per_file_cap = pool.options.per_file_cap.max(1) as usize;
    let stop_after_first = pool.options.stop_after_first;
    let carry_limit = scan_carry_len(pool.matchers);
    let (keep_start, keep_end, read_start) = match segment {
        Some(segment) => (
            segment.start,
            segment.start + segment.len,
            segment.start.saturating_sub(carry_limit as u64),
        ),
        None => (0, u64::MAX, 0),
    };
    // 跨段尾的命中归本段：读到段尾之后 carry 字节即可完整看到它们。
    let stop_at = keep_end.saturating_add(carry_limit as u64);
    let mut scan = ItemScan::default();
    let ScanScratch {
        carry,
        scan_buf,
        hits_buf,
        matcher,
    } = scratch;
    carry.clear();
    let mut next_offset = read_start;
    let visitor = |chunk: &[u8]| -> anyhow::Result<()> {
        if pool.cancel_flag.is_cancelled() {
            return Err(CancelMarker.into());
        }
        let chunk_base = next_offset;
        next_offset += chunk.len() as u64;
        let carry_len = carry.len();
        let window_start = chunk_base - carry_len as u64;
        scan_buf.clear();
        scan_buf.extend_from_slice(carry);
        scan_buf.extend_from_slice(chunk);
        scan.complete_pending(scan_buf, window_start);
        if !scan.stop_requested {
            hits_buf.clear();
            pool.matchers
                .scan_chunk_with(scan_buf, carry_len, hits_buf, matcher);
            // 稳定排序：每文件上限保留文件序最前的命中，同偏移保持匹配器顺序。
            hits_buf.sort_by_key(|hit| hit.start);
            for hit in hits_buf.iter() {
                let absolute = window_start + hit.start as u64;
                if absolute < keep_start || absolute >= keep_end {
                    continue;
                }
                scan.hit_count += 1;
                if scan.stored.len() < per_file_cap {
                    scan.store(hit, absolute, scan_buf, window_start);
                }
                if stop_after_first {
                    scan.stop_requested = true;
                    break;
                }
            }
        }
        let keep = carry_limit.min(scan_buf.len());
        carry.clear();
        carry.extend_from_slice(&scan_buf[scan_buf.len() - keep..]);
        if scan.pending.is_empty() {
            if scan.stop_requested {
                return Err(StopAfterFirstMarker.into());
            }
            if next_offset >= stop_at {
                return Err(SegmentDone.into());
            }
        }
        Ok(())
    };
    let chunk_bytes = pool.layout.chunk_bytes;
    let result = match segment {
        Some(_) => {
            session.stream_file_range(source.read_id(), read_start, u64::MAX, chunk_bytes, visitor)
        }
        None => session.stream_file(source.read_id(), chunk_bytes, visitor),
    };
    match result {
        Ok(_) => {}
        Err(err) if err.downcast_ref::<CancelMarker>().is_some() => scan.cancelled = true,
        Err(err)
            if err.downcast_ref::<StopAfterFirstMarker>().is_some()
                || err.downcast_ref::<SegmentDone>().is_some() => {}
        // Nothing to search, and nothing failed to read: the file has no data
        // stream on the image (NTFS metadata, a record caught mid-update).
        Err(err) if err.downcast_ref::<MissingDataStream>().is_some() => {
            tracing::debug!(
                "global_search no data stream node={}: {err:#}",
                source.node_id
            );
        }
        Err(err) => {
            scan.read_error = true;
            tracing::warn!("global_search stream err node={}: {err:#}", source.node_id);
        }
    }
    scan
}

/// 由命中构建落库记录。上下文缺前文的命中（超长正则）在这里回读。
fn build_records(
    session: &mut FsSession<'_>,
    pool: &ScanPool<'_>,
    source: &ScanSource,
    hits: Vec<FileHit>,
    match_count: u64,
) -> Vec<GlobalSearchHit> {
    hits.into_iter()
        .map(|mut hit| {
            let raw = match hit.context.take() {
                Some(bytes) => bytes,
                None => {
                    let (start, end) = context_span(hit.absolute, hit.len);
                    match session.read_file_range(source.read_id(), start, (end - start) as usize) {
                        Ok(bytes) => bytes,
                        Err(err) => {
                            tracing::debug!(
                                "global_search context read failed node={}: {err:#}",
                                source.node_id
                            );
                            Vec::new()
                        }
                    }
                }
            };
            build_hit_record(
                source,
                &hit,
                &raw,
                pool.patterns,
                pool.keyword_count,
                match_count,
            )
        })
        .collect()
}

/// 构建一条命中记录（`raw` 是 [`context_span`] 区间的原始字节）。
fn build_hit_record(
    source: &ScanSource,
    hit: &FileHit,
    raw: &[u8],
    patterns: &[String],
    keyword_count: usize,
    match_count: u64,
) -> GlobalSearchHit {
    let pattern = patterns.get(hit.pattern_index).cloned().unwrap_or_default();
    let pattern_kind = if hit.pattern_index < keyword_count {
        "keyword"
    } else {
        "regex"
    };
    let decoded = hit.encoding.decode_to_string(raw);
    GlobalSearchHit {
        hit_seq: 0,
        node_id: source.node_id.clone(),
        volume_key: source.volume_key.clone(),
        file_path: if source.volume_key.is_empty() {
            source.path.clone()
        } else {
            format!("{}:{}", source.volume_key, source.path)
        },
        file_name: source.name.clone(),
        file_size: source.size_bytes,
        file_offset: hit.absolute,
        match_len: hit.len as u64,
        encoding: hit.encoding.label().to_string(),
        pattern,
        pattern_kind: pattern_kind.to_string(),
        match_count,
        context_preview: collapse_newlines(&decoded, CONTEXT_PREVIEW_CHARS),
        context_full: context_full(&decoded, CONTEXT_FULL_CHARS, CONTEXT_FULL_LINES),
        md5: String::new(),
        sha256: String::new(),
        source_kind: source.source_kind.to_string(),
    }
}

// ── 上下文修剪 ─────────────────────────────────────────────────────────

/// 把 `\r\n` / `\n` 折叠为 " / " 并截断到 `cap` 字符。
fn collapse_newlines(text: &str, cap: usize) -> String {
    let mut out = String::new();
    let mut emitted = 0usize;
    let mut first = true;
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if !first {
            if emitted + 3 >= cap {
                break;
            }
            out.push_str(" / ");
            emitted += 3;
        }
        first = false;
        let take = cap.saturating_sub(emitted);
        let clipped: String = line.chars().take(take).collect();
        emitted += clipped.chars().count();
        out.push_str(&clipped);
        if emitted >= cap {
            break;
        }
    }
    out
}

/// 保留换行，截断到 `cap_chars` 字符 / 约 `cap_lines` 行。
fn context_full(text: &str, cap_chars: usize, cap_lines: usize) -> String {
    let mut out = String::with_capacity(cap_chars.min(text.len()));
    let mut lines = 0usize;
    for (chars, ch) in text.chars().enumerate() {
        if ch == '\n' {
            lines += 1;
            if lines >= cap_lines {
                break;
            }
        }
        if chars >= cap_chars {
            break;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
#[path = "global_search_scan_tests.rs"]
mod tests;
