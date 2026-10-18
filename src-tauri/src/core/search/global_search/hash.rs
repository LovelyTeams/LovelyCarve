//! hash implementation.
use super::*;

/// CalcHash：命中过的源多 worker 并行二次流式计算 MD5 + SHA256，回填在调用
/// 线程的写入连接上分批提交。单个源读取失败只记 warn；回填失败的批次留待
/// 重试，积压超限或收尾仍写不进去时返回错误，哈希不会静默缺失。
pub(super) fn fill_source_hashes(
    params: &GlobalSearchParams<'_>,
    sources: &[&ScanSource],
    reopen_path: Option<&Path>,
    workers: usize,
    writer: &mut HitWriter,
) -> anyhow::Result<()> {
    let next_source = AtomicUsize::new(0);
    let stop_flag = AtomicBool::new(false);
    let (tx, rx) = sync_channel::<(usize, String, String)>(workers.saturating_mul(4).max(4));
    thread::scope(|scope| {
        for _ in 0..workers.min(sources.len()).max(1) {
            let tx = tx.clone();
            let next_source = &next_source;
            let stop_flag = &stop_flag;
            scope.spawn(move || {
                let image = match worker_image(params.image, reopen_path) {
                    Ok(image) => image,
                    Err(err) => {
                        tracing::warn!("global_search hash worker image open failed: {err:#}");
                        return;
                    }
                };
                let mut session = FsSession::new(&image);
                loop {
                    if params.cancel_flag.is_cancelled() || stop_flag.load(Ordering::Relaxed) {
                        return;
                    }
                    let index = next_source.fetch_add(1, Ordering::Relaxed);
                    let Some(source) = sources.get(index) else {
                        return;
                    };
                    match hash_source(&mut session, source) {
                        Ok((md5_hex, sha256_hex)) => {
                            if tx.send((index, md5_hex, sha256_hex)).is_err() {
                                return;
                            }
                        }
                        Err(err) => {
                            tracing::warn!(
                                "global_search hash failed node={}: {err:#}",
                                source.node_id
                            );
                        }
                    }
                }
            });
        }
        drop(tx);
        let mut batch: Vec<(String, String, String)> = Vec::with_capacity(HASH_WRITEBACK_BATCH);
        let mut write_error: Option<anyhow::Error> = None;
        for (index, md5_hex, sha256_hex) in rx {
            batch.push((sources[index].node_id.clone(), md5_hex, sha256_hex));
            if write_error.is_none() && batch.len() >= HASH_WRITEBACK_BATCH {
                if let Err(err) = write_hashes(writer, &mut batch) {
                    write_error = Some(err);
                    stop_flag.store(true, Ordering::Relaxed);
                }
            }
        }
        let final_write = write_batch(&mut batch, |hashes| writer.set_source_hashes(hashes));
        match write_error {
            Some(err) => Err(err),
            None => final_write
                .with_context(|| format!("保存命中源哈希失败（{} 个源未落库）", batch.len())),
        }
    })
}

/// CalcHash 回填的批量落库（同 [`flush_hits`]，积压上限 [`HASH_RETRY_BACKLOG`]）。
pub(super) fn write_hashes(
    writer: &mut HitWriter,
    batch: &mut Vec<(String, String, String)>,
) -> anyhow::Result<()> {
    write_batch_or_defer(batch, HASH_RETRY_BACKLOG, |hashes| {
        writer.set_source_hashes(hashes)
    })
    .with_context(|| format!("保存命中源哈希失败（{} 个源未落库）", batch.len()))
}

/// 流式计算源的 MD5 + SHA256。
pub(super) fn hash_source(
    session: &mut FsSession<'_>,
    source: &ScanSource,
) -> anyhow::Result<(String, String)> {
    let mut md5_hasher = crate::core::common::hash::Md5::new();
    let mut sha256_hasher = sha2::Sha256::new();
    session.stream_file(source.read_id(), HASH_CHUNK_BYTES, |chunk| {
        md5_hasher.update(chunk);
        sha256_hasher.update(chunk);
        Ok(())
    })?;
    Ok((
        hex::encode(md5_hasher.finalize()),
        hex::encode(sha256_hasher.finalize()),
    ))
}
