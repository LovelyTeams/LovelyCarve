//! 无头关键词扫描：真实镜像全文件扫描，使用与 `commands/keyword_search.rs` 一致的
//! 的数据通路（文件树 walk → `stream_file_chunks` → `MatcherSet`），输出
//! 命中统计、扫描速率与按文件大小的分桶计时（定位 FS 层 vs IO 层瓶颈）。
//! 默认关键词 `password`，可追加任意个；`--no-match` 只流式读不做匹配。
//! Usage: cargo run --example keyword_scan -- <image> [keyword...] [--no-match] [--threads N] [--chunk-mb N]
//!
//! `--threads N`（默认 1）：N 个 worker 线程各自打开一份镜像句柄（libvmdk/
//! EWF 读取器支持定位读取，`ImageSource` 保留的 FairMutex 使共享句柄的并行读
//! 退化为串行）——每线程独立 `EvidenceSession` + `FsSession`，匹配器经 Arc 共享。
use std::env;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use lovelycarve_core::filesystem::evidence_session::EvidenceSession;
use lovelycarve_core::filesystem::tree::fs_tree::{FsSession, NodeKind};
use lovelycarve_core::filesystem::tree::fs_tree_walk::{walk_image_from, FsWalkEntry};
use lovelycarve_core::filesystem::unalloc::is_virtual_unalloc_id;
use lovelycarve_core::search::keyword::keyword_search::MatcherSet;

/// 按文件大小分桶：小文件段暴露 FS 层固定开销（inode/树块随机读），
/// 大文件段暴露底层 IO 带宽。
struct Bucket {
    count: usize,
    bytes: u64,
    elapsed: Duration,
    max_file: (u64, String),
}

impl Bucket {
    fn new() -> Self {
        Self {
            count: 0,
            bytes: 0,
            elapsed: Duration::ZERO,
            max_file: (0, String::new()),
        }
    }

    fn merge(&mut self, other: &Bucket) {
        self.count += other.count;
        self.bytes += other.bytes;
        self.elapsed += other.elapsed;
        if other.max_file.0 > self.max_file.0 {
            self.max_file = other.max_file.clone();
        }
    }

    fn report(&self, label: &str) {
        let rate = if self.elapsed.is_zero() {
            0.0
        } else {
            self.bytes as f64 / self.elapsed.as_secs_f64() / 1024.0 / 1024.0
        };
        println!(
            "  {label:<8} {count:>7} files  {bytes:>9.2} MiB  {elapsed:>7.2}s  {rate:>8.0} MiB/s  max {max}",
            count = self.count,
            bytes = self.bytes as f64 / 1048576.0,
            elapsed = self.elapsed.as_secs_f32(),
            max = self.max_file.1
        );
    }
}

fn bucket_of(size: u64) -> usize {
    match size {
        0..=4096 => 0,
        4097..=65536 => 1,
        65537..=1048576 => 2,
        _ => 3,
    }
}

/// 跨线程聚合的扫描计数器（单线程路径同样使用，取 load 即读）。
struct Counters {
    files: AtomicUsize,
    bytes: AtomicU64,
    total_hits: AtomicUsize,
    read_errors: AtomicUsize,
}

impl Counters {
    fn new() -> Self {
        Self {
            files: AtomicUsize::new(0),
            bytes: AtomicU64::new(0),
            total_hits: AtomicUsize::new(0),
            read_errors: AtomicUsize::new(0),
        }
    }
}

/// 每个 worker 的私有统计（结束后合并）。
struct WorkerStats {
    buckets: Vec<Bucket>,
}

/// 大文件切段大小。
const SEGMENT_BYTES: u64 = 64 * 1024 * 1024;
/// 达到该大小的文件切成段跨 worker 扫描。
const SPLIT_MIN: u64 = 2 * SEGMENT_BYTES;

/// 一个可扫单元：普通文件整体，或大文件（≥ `SPLIT_MIN`）的一段 64 MiB。
/// 大文件切段后按跨步摊到多个 worker，单个巨型文件（如 swap）不再独占
/// 一个 worker 扫到末尾、其余 worker 空等；各段 worker 都从文件头顺序流，
/// 读的是同一批页，OS 页缓存吸收重复流式的开销。
struct WorkItem {
    entry_idx: usize,
    /// 段起始偏移；0 表示整文件路径。
    seg_start: u64,
    /// 段长度（整文件路径时为文件大小）。
    seg_len: u64,
}

/// 段扫描提前结束的哨兵：流到 `seg_end + carry` 即中止（`stream_file` 的
/// 访客返回 Err 即停，与 `read_file_range` 的 RangeComplete 同法）。
#[derive(Debug)]
struct SegmentDone;

impl std::fmt::Display for SegmentDone {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("segment window scanned")
    }
}

impl std::error::Error for SegmentDone {}

/// 按 `start + k * stride` 的跨步方式扫描 items 子集（walk 顺序）。每个线程
/// 持有自己的 `EvidenceSession`（自己的 libyal 句柄）与 `FsSession`，因此 IO 真正
/// 并行。
#[allow(clippy::too_many_arguments)]
fn scan_entries(
    image: &Arc<EvidenceSession>,
    entries: &[FsWalkEntry],
    items: &[WorkItem],
    start: usize,
    stride: usize,
    matchers: Option<&MatcherSet>,
    chunk_size: usize,
    counters: &Counters,
    hit_counts: &[AtomicUsize],
) -> anyhow::Result<WorkerStats> {
    let carry_len = matchers
        .map(|m| m.max_pattern_len())
        .unwrap_or(1)
        .saturating_sub(1);
    let mut buckets: Vec<Bucket> = (0..4).map(|_| Bucket::new()).collect();
    // 跨 chunk 复用的缓冲：symlink 没有可扫内容（ext4 的 symlink 是 inode 内
    // 的目标字符串，kind 标注为 Symlink），只扫普通文件。
    let mut scan_buf: Vec<u8> = Vec::with_capacity(chunk_size + carry_len);
    let mut hitbuf: Vec<lovelycarve_core::search::keyword::keyword_search::ByteHit> = Vec::new();
    // 一个 session 扫整树：卷按卷根缓存，而不是每个文件重开一次文件系统。
    let mut session = FsSession::new(image);

    let mut pos = start;
    while pos < items.len() {
        let item = &items[pos];
        let entry = &entries[item.entry_idx];
        let whole = item.seg_start == 0 && item.seg_len == entry.node.size;
        if whole || item.seg_start == 0 {
            // 分段文件的首段负责计数（每文件只计一次）。
            counters.files.fetch_add(1, Ordering::Relaxed);
        }
        let bucket = bucket_of(entry.node.size);
        if entry.node.size > buckets[bucket].max_file.0 {
            buckets[bucket].max_file = (entry.node.size, entry.path.clone());
        }
        let mut carry: Vec<u8> = Vec::with_capacity(carry_len);
        let mut hits = 0usize;
        let file_elapsed = Instant::now();
        if whole {
            match session.stream_file(&entry.node.id, chunk_size, |chunk| {
                if let Some(matchers) = matchers {
                    scan_buf.clear();
                    scan_buf.extend_from_slice(&carry);
                    scan_buf.extend_from_slice(chunk);
                    hitbuf.clear();
                    matchers.scan_chunk(&scan_buf, carry.len(), &mut hitbuf);
                    hits += hitbuf.len();
                    let keep = carry_len.min(chunk.len());
                    carry.clear();
                    carry.extend_from_slice(&chunk[chunk.len() - keep..]);
                }
                Ok(())
            }) {
                Ok(total) => {
                    counters.bytes.fetch_add(total, Ordering::Relaxed);
                    buckets[bucket].bytes += total;
                }
                Err(err) => {
                    counters.read_errors.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!("read {} failed: {err:#}", entry.path);
                }
            }
        } else {
            // 段扫描：从头顺序流，跳过本段之前的字节；命中按绝对偏移过滤到
            // [seg_start, seg_end)。跨段边界的模式由上一段负责（它扫到
            // seg_start + carry 为止），本段丢弃 start < seg_start 的命中。
            let seg_end = item.seg_start + item.seg_len;
            let mut base: u64 = 0; // 下一个 chunk 的文件偏移
            let mut scanned_any = false;
            match session.stream_file(&entry.node.id, chunk_size, |chunk| {
                let chunk_base = base;
                base += chunk.len() as u64;
                if chunk_base + chunk.len() as u64 <= item.seg_start {
                    return Ok(());
                }
                if !scanned_any && chunk_base < item.seg_start {
                    // 含 seg_start 的首个 chunk：chunk 前缀属于上一段，作
                    // carry 传入并过滤掉从中起头的命中。
                    scanned_any = true;
                    let skip = (item.seg_start - chunk_base) as usize;
                    if let Some(matchers) = matchers {
                        scan_buf.clear();
                        scan_buf.extend_from_slice(chunk);
                        hitbuf.clear();
                        matchers.scan_chunk(&scan_buf, skip, &mut hitbuf);
                        hitbuf.retain(|hit| {
                            hit.start >= skip && chunk_base + (hit.start as u64) < seg_end
                        });
                        hits += hitbuf.len();
                        let keep = carry_len.min(chunk.len());
                        carry.clear();
                        carry.extend_from_slice(&chunk[chunk.len() - keep..]);
                    }
                } else {
                    scanned_any = true;
                    if let Some(matchers) = matchers {
                        let window_base = chunk_base - carry.len() as u64;
                        scan_buf.clear();
                        scan_buf.extend_from_slice(&carry);
                        scan_buf.extend_from_slice(chunk);
                        hitbuf.clear();
                        matchers.scan_chunk(&scan_buf, carry.len(), &mut hitbuf);
                        hitbuf.retain(|hit| window_base + (hit.start as u64) < seg_end);
                        hits += hitbuf.len();
                        let keep = carry_len.min(chunk.len());
                        carry.clear();
                        carry.extend_from_slice(&chunk[chunk.len() - keep..]);
                    }
                }
                // 扫过 seg_end + carry 后中止：跨边界模式已在上一 chunk 集齐。
                if base >= seg_end + carry_len as u64 {
                    return Err(SegmentDone.into());
                }
                Ok(())
            }) {
                Ok(_total) => {
                    counters.bytes.fetch_add(item.seg_len, Ordering::Relaxed);
                    buckets[bucket].bytes += item.seg_len;
                }
                Err(err) if err.downcast_ref::<SegmentDone>().is_some() => {
                    counters.bytes.fetch_add(item.seg_len, Ordering::Relaxed);
                    buckets[bucket].bytes += item.seg_len;
                }
                Err(err) => {
                    counters.read_errors.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!("read {} failed: {err:#}", entry.path);
                }
            }
        }
        let b = &mut buckets[bucket_of(entry.node.size)];
        b.count += 1;
        b.elapsed += file_elapsed.elapsed();
        if hits > 0 {
            counters.total_hits.fetch_add(hits, Ordering::Relaxed);
            // 命中按文件归并（分段文件的各段都累进同一槽），main 侧据此算
            // matched_files 与 most-hits，跨段精确。
            hit_counts[item.entry_idx].fetch_add(hits, Ordering::Relaxed);
        }
        pos += stride;
    }
    Ok(WorkerStats { buckets })
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "usage: keyword_scan <image> [keyword...] [--no-match] [--threads N] [--chunk-mb N]"
        );
        std::process::exit(2);
    }
    let path = &args[1];
    let no_match = args.iter().any(|arg| arg == "--no-match");
    let mut threads = 1usize;
    let mut chunk_mb = 1usize;
    let mut keywords: Vec<String> = Vec::new();
    let mut it = args.iter().skip(2);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--no-match" => {}
            "--threads" => {
                threads = it
                    .next()
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or_else(|| {
                        eprintln!("bad --threads value");
                        std::process::exit(2)
                    })
                    .max(1);
            }
            "--chunk-mb" => {
                chunk_mb = it
                    .next()
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or_else(|| {
                        eprintln!("bad --chunk-mb value");
                        std::process::exit(2)
                    })
                    .max(1);
            }
            other => keywords.push(other.to_string()),
        }
    }
    let keywords: Vec<String> = if keywords.is_empty() {
        vec!["password".to_string(), "secret".to_string()]
    } else {
        keywords
    };
    let case_sensitive = false;
    let chunk_size = chunk_mb * 1024 * 1024;

    let image = Arc::new(EvidenceSession::open(path)?);
    let info = image.info();
    println!("image:  {}", info.path);
    println!(
        "format: {:?}  size: {} bytes",
        info.source_format, info.size_bytes
    );
    for p in &info.partitions {
        println!(
            "  partition #{} kind={:?} start_lba={} length_lba={}",
            p.id, p.kind, p.start_lba, p.length_lba
        );
    }

    let matchers: Option<Arc<MatcherSet>> = if no_match {
        None
    } else {
        Some(Arc::new(MatcherSet::build(&keywords, case_sensitive)))
    };
    println!(
        "keywords: {keywords:?}  case_sensitive={case_sensitive}  no_match={no_match}  threads={threads}  chunk={chunk_mb}MiB"
    );

    let t0 = Instant::now();
    let entries = Arc::new(walk_image_from(&image, "", usize::MAX)?);
    println!(
        "walk:    {} entries in {:.2}s",
        entries.len(),
        t0.elapsed().as_secs_f32()
    );

    // 可扫单元（walk 顺序）：普通文件整体；≥ SPLIT_MIN 的大文件切成 64 MiB
    // 段摊到多个 worker，巨型文件不再独占一个 worker 扫到末尾。
    let mut items: Vec<WorkItem> = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        if entry.node.kind == NodeKind::File
            && entry.node.size > 0
            && !is_virtual_unalloc_id(&entry.node.id)
        {
            if entry.node.size >= SPLIT_MIN {
                let mut seg_start = 0u64;
                while seg_start < entry.node.size {
                    let seg_len = SEGMENT_BYTES.min(entry.node.size - seg_start);
                    items.push(WorkItem {
                        entry_idx: i,
                        seg_start,
                        seg_len,
                    });
                    seg_start += seg_len;
                }
            } else {
                items.push(WorkItem {
                    entry_idx: i,
                    seg_start: 0,
                    seg_len: entry.node.size,
                });
            }
        }
    }
    println!(
        "items:   {} scan units (split >= {} MiB)",
        items.len(),
        SPLIT_MIN / 1048576
    );

    let counters = Counters::new();
    // 每文件一个命中槽：分段文件的各段累进同一槽，结束侧算 matched_files
    // 与 most-hits（分段摊派后单段命中无法代表整文件）。
    let hit_counts: Vec<AtomicUsize> = (0..entries.len()).map(|_| AtomicUsize::new(0)).collect();
    let done = Arc::new(AtomicBool::new(false));
    let scan_start = Instant::now();
    let image_path = image.path.clone();

    let mut merged = WorkerStats {
        buckets: (0..4).map(|_| Bucket::new()).collect(),
    };
    let merged = std::thread::scope(|scope| -> anyhow::Result<WorkerStats> {
        // 进度汇报线程：每 2s 读一次原子计数器。
        {
            let done = done.clone();
            let counters = &counters;
            scope.spawn(move || {
                let mut last_report = Instant::now();
                while !done.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(200));
                    if last_report.elapsed() < Duration::from_secs(2) {
                        continue;
                    }
                    last_report = Instant::now();
                    if done.load(Ordering::Relaxed) {
                        break;
                    }
                    let files = counters.files.load(Ordering::Relaxed);
                    let bytes = counters.bytes.load(Ordering::Relaxed);
                    let read_errors = counters.read_errors.load(Ordering::Relaxed);
                    let rate = bytes as f64 / scan_start.elapsed().as_secs_f64() / 1024.0 / 1024.0;
                    println!(
                        "  {files} files, {:.2} GiB, {rate:.0} MiB/s, {} read errs",
                        bytes as f64 / 1073741824.0,
                        read_errors
                    );
                }
            });
        }

        // 扫描 worker：worker 0 复用主线程已打开的镜像，其余在线程内各自
        // 开新句柄（libyal 句柄非线程安全，每线程一份才能并行 IO）。
        let mut handles = Vec::new();
        for worker in 0..threads {
            let entries = entries.clone();
            let matchers = matchers.clone();
            let counters = &counters;
            let image = image.clone();
            let image_path = image_path.clone();
            let items = &items;
            let hit_counts = &hit_counts;
            handles.push(scope.spawn(move || -> anyhow::Result<WorkerStats> {
                let worker_image = if worker == 0 {
                    image
                } else {
                    Arc::new(EvidenceSession::open(&image_path)?)
                };
                scan_entries(
                    &worker_image,
                    &entries,
                    items,
                    worker,
                    threads,
                    matchers.as_deref(),
                    chunk_size,
                    counters,
                    hit_counts,
                )
            }));
        }
        // 先收齐所有 worker 的结果（含 panic），置位 done 再传播错误——否则
        // 进度线程不退出，scope 会卡在 join。
        let mut results = Vec::new();
        for handle in handles {
            results.push(
                handle
                    .join()
                    .map_err(|_| anyhow::anyhow!("scan worker panicked")),
            );
        }
        done.store(true, Ordering::Relaxed);
        for result in results {
            let stats = result??;
            for (merged_bucket, worker_bucket) in merged.buckets.iter_mut().zip(&stats.buckets) {
                merged_bucket.merge(worker_bucket);
            }
        }
        Ok(merged)
    })?;

    let elapsed = scan_start.elapsed();
    let bytes = counters.bytes.load(Ordering::Relaxed);
    let rate = bytes as f64 / elapsed.as_secs_f64() / 1024.0 / 1024.0;
    let matched_files = hit_counts
        .iter()
        .filter(|count| count.load(Ordering::Relaxed) > 0)
        .count();
    let max_hits_file = hit_counts
        .iter()
        .enumerate()
        .map(|(i, count)| (count.load(Ordering::Relaxed), &entries[i]))
        .filter(|(hits, _)| *hits > 0)
        .max_by_key(|(hits, _)| *hits)
        .map(|(hits, entry)| (hits, entry.path.clone()))
        .unwrap_or_default();
    println!();
    println!(
        "scan:    {} files, {:.2} GiB streamed in {:.2}s ({rate:.0} MiB/s)",
        counters.files.load(Ordering::Relaxed),
        bytes as f64 / 1073741824.0,
        elapsed.as_secs_f32()
    );
    println!(
        "  matched files: {matched_files}   total hits: {}",
        counters.total_hits.load(Ordering::Relaxed)
    );
    println!(
        "  read errors:   {}",
        counters.read_errors.load(Ordering::Relaxed)
    );
    println!(
        "  most hits:     {} hits @ {}",
        max_hits_file.0, max_hits_file.1
    );
    println!("  buckets (by size):");
    for (i, label) in ["<=4K", "4K-64K", "64K-1M", ">1M"].iter().enumerate() {
        merged.buckets[i].report(label);
    }
    Ok(())
}
