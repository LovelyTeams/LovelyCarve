//! scan implementation.
use super::*;
use crate::core::filesystem::evidence_session::EvidenceSession;

// ── 扫描编排 ───────────────────────────────────────────────────────────

/// 毒锁恢复：worker panic 不应让整个搜索崩溃。
pub(super) fn recover_poison<T>(result: std::sync::LockResult<T>) -> T {
    result.unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 封装镜像（libyal 式句柄非线程安全）的 worker 各自重开一份句柄；其余来源
/// 共享同一 `EvidenceSession`。
pub(super) fn worker_reopen_path(image: &EvidenceSession) -> Option<&Path> {
    matches!(
        image.info().source_format,
        ImageFormat::Ewf
            | ImageFormat::Vmdk
            | ImageFormat::Vhd
            | ImageFormat::Vhdx
            | ImageFormat::Pmfx
    )
    .then_some(image.path.as_path())
}

/// worker 使用的镜像句柄：需要时重开，否则共享。
pub(super) fn worker_image(
    image: &Arc<EvidenceSession>,
    reopen_path: Option<&Path>,
) -> anyhow::Result<Arc<EvidenceSession>> {
    match reopen_path {
        Some(path) => EvidenceSession::open(path).map(Arc::new),
        None => Ok(Arc::clone(image)),
    }
}

/// 引擎主入口：解析范围 → 建工作单元 → 工作池扫描 → drain（编排 hit_seq、
/// 批量落库、节流进度、全局上限）→ CalcHash 收尾。取消时已落库的部分结果
/// 保留（返回 `status=cancelled` 的摘要，不作为错误）。
pub fn run_global_search(params: &GlobalSearchParams<'_>) -> anyhow::Result<GlobalSearchSummary> {
    let started = Instant::now();
    let global_cap = params.options.global_cap.max(1) as u64;

    let (sources, files_total) = resolve_sources(params)?;
    let layout = ScanLayout::default();
    let plan = {
        let mut probe = FsSession::new(params.image);
        build_work_plan(&mut probe, &sources, params.options, layout)
    };
    tracing::info!(
        "global_search sources={} files_total={} work_items={} batches={} resolve_ms={}",
        sources.len(),
        files_total,
        plan.item_count(),
        plan.batch_count(),
        started.elapsed().as_millis()
    );

    let reopen_path = worker_reopen_path(params.image);
    let workers = thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
        .clamp(1, MAX_SEARCH_WORKERS)
        .min(plan.batch_count().max(1));

    let mut writer = HitWriter::open(params.case_dir, params.image_id, params.search_id)?;
    let stop_flag = AtomicBool::new(false);
    let pool = ScanPool {
        plan: &plan,
        sources: &sources,
        matchers: params.matchers,
        options: params.options,
        patterns: params.patterns,
        keyword_count: params.keyword_count,
        cancel_flag: params.cancel_flag,
        stop_flag: &stop_flag,
        layout,
        next_batch: AtomicUsize::new(0),
        files_done: AtomicU64::new(plan.zero_byte_sources),
        files_scanned: AtomicU64::new(0),
    };
    let first_error = std::sync::Mutex::new(None::<anyhow::Error>);
    let (tx, rx) = sync_channel::<SourceOutcome>(workers.saturating_mul(8).max(8));

    let mut next_ordinal: u64 = 0;
    let mut pending_hits: Vec<GlobalSearchHit> = Vec::new();
    // 积压超限的命中落库错误：记下后 worker 被叫停、不再中途落库，整次搜索
    // 以它失败。
    let mut write_error: Option<anyhow::Error> = None;
    // 命中源（CalcHash 二次遍历对象），按源下标去重一次。
    let mut matched_source_set: HashSet<usize> = HashSet::new();
    let mut matched_sources: Vec<usize> = Vec::new();
    let mut last_flush = Instant::now();
    let mut last_progress = Instant::now();
    let mut matched_file_count: u64 = 0;
    let mut total_matches: u64 = 0;
    let mut read_errors: u64 = 0;
    let mut truncated = false;

    thread::scope(|scope| {
        for _ in 0..workers {
            let tx = tx.clone();
            let pool = &pool;
            let first_error = &first_error;
            scope.spawn(move || match worker_image(params.image, reopen_path) {
                Ok(image) => scan_worker(pool, &image, &tx),
                Err(err) => {
                    *recover_poison(first_error.lock()) = Some(err);
                    pool.stop_flag.store(true, Ordering::Relaxed);
                }
            });
        }
        drop(tx);
        let report_progress = |total_matches: u64, matched_file_count: u64| {
            (params.on_progress)(
                pool.files_done.load(Ordering::Relaxed),
                files_total,
                total_matches,
                matched_file_count,
            );
        };
        loop {
            let flush_due = match rx.recv_timeout(FLUSH_THROTTLE) {
                Ok(outcome) => {
                    if outcome.read_error {
                        read_errors += 1;
                    }
                    if outcome.hit_count > 0 || !outcome.records.is_empty() {
                        if matched_source_set.insert(outcome.source_idx) {
                            matched_file_count += 1;
                            matched_sources.push(outcome.source_idx);
                        }
                        total_matches += outcome.hit_count;
                        for mut record in outcome.records {
                            record.hit_seq = next_ordinal;
                            next_ordinal += 1;
                            pending_hits.push(record);
                        }
                        if total_matches >= global_cap {
                            truncated = true;
                            stop_flag.store(true, Ordering::Relaxed);
                        }
                    }
                    if last_progress.elapsed() >= FLUSH_THROTTLE {
                        report_progress(total_matches, matched_file_count);
                        last_progress = Instant::now();
                    }
                    pending_hits.len() >= HIT_FLUSH_EVERY || last_flush.elapsed() >= FLUSH_THROTTLE
                }
                Err(RecvTimeoutError::Timeout) => {
                    report_progress(total_matches, matched_file_count);
                    last_progress = Instant::now();
                    true
                }
                Err(RecvTimeoutError::Disconnected) => break,
            };
            if flush_due && write_error.is_none() && !pending_hits.is_empty() {
                if let Err(err) = flush_hits(&mut writer, &mut pending_hits) {
                    write_error = Some(err);
                    stop_flag.store(true, Ordering::Relaxed);
                }
                last_flush = Instant::now();
            }
        }
    });
    // 收尾落库：没有下一批可以重试了，失败即整次搜索失败——不能报告完成却
    // 缺命中。中途已因积压超限中止时也先把手里的命中写进去。
    let final_flush = write_batch(&mut pending_hits, |hits| writer.append(hits));

    if let Some(err) = recover_poison(first_error.into_inner()) {
        return Err(err);
    }
    if let Some(err) = write_error {
        return Err(err);
    }
    final_flush.with_context(|| format!("保存搜索命中失败（{} 条未保存）", pending_hits.len()))?;

    let cancelled = params.cancel_flag.is_cancelled();

    if params.options.calc_hash && !matched_sources.is_empty() && !cancelled {
        let matched: Vec<&ScanSource> = matched_sources
            .iter()
            .map(|&index| &sources[index])
            .collect();
        fill_source_hashes(params, &matched, reopen_path, workers, &mut writer)?;
    }

    let files_scanned = pool.files_scanned.into_inner();
    let record_count = next_ordinal;
    let status = if cancelled { "cancelled" } else { "success" };
    Ok(GlobalSearchSummary {
        search_id: params.search_id.to_string(),
        status: status.to_string(),
        keyword_list: params.patterns[..params.keyword_count.min(params.patterns.len())].to_vec(),
        regex_list: params.patterns[params.keyword_count.min(params.patterns.len())..].to_vec(),
        codepages: params.options.codepages.clone(),
        matched_file_count,
        total_matches,
        record_count,
        elapsed_ms: started.elapsed().as_millis() as u64,
        files_scanned,
        files_total,
        read_errors,
        truncated,
    })
}

/// 缓冲整批落库，成功才清空；失败时缓冲原样保留（单事务已回滚），可随下一批
/// 整批重试——命中 `INSERT OR REPLACE` 按序号覆盖、哈希按源 UPDATE，重试都
/// 幂等。
pub(super) fn write_batch<T>(
    buffer: &mut Vec<T>,
    write: impl FnOnce(&[T]) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    if !buffer.is_empty() {
        write(buffer)?;
        buffer.clear();
    }
    Ok(())
}

/// 扫描进行中的落库：失败的数据留在缓冲里等下一批重试，只记 warn；积压到
/// `backlog_limit` 仍写不进去才返回错误。
pub(super) fn write_batch_or_defer<T>(
    buffer: &mut Vec<T>,
    backlog_limit: usize,
    write: impl FnOnce(&[T]) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    match write_batch(buffer, write) {
        Err(err) if buffer.len() < backlog_limit => {
            tracing::warn!(
                "global_search writeback failed, {} rows kept for retry: {err:#}",
                buffer.len()
            );
            Ok(())
        }
        result => result,
    }
}

/// drain 的批量落命中。失败的命中留在 `pending` 里随下一批重试，不会丢；
/// 积压超过 [`HIT_RETRY_BACKLOG`] 才返回错误，由调用方中止扫描。
pub(super) fn flush_hits(
    writer: &mut HitWriter,
    pending: &mut Vec<GlobalSearchHit>,
) -> anyhow::Result<()> {
    write_batch_or_defer(pending, HIT_RETRY_BACKLOG, |hits| writer.append(hits))
        .with_context(|| format!("保存搜索命中失败（{} 条未保存）", pending.len()))
}
