//! 全局搜索端到端基准：对真实镜像建一个案件并发布各卷的快照与索引（缓存在
//! 工作目录，重跑复用），再直接调用 `core::search::global_search::run_global_search`——与
//! `global_search_start` 命令同一条引擎通路（范围解析 → 工作池扫描 → 命中
//! 上下文 → 落库 → CalcHash），打印摘要与耗时。
//!
//! Usage:
//!
//! ```text
//! just run-example-fast bench_global_search <image> <work_dir> [options]
//!   --keys a,b           关键词（逗号分隔，默认 password）
//!   --regex R            正则（可重复）
//!   --codepages A,B      编码页（默认与界面一致：UTF-8,GBK,UTF-16LE,UTF-16BE）
//!   --match-case         区分大小写
//!   --per-file-cap N     每文件命中上限（默认 100）
//!   --global-cap N       全局命中上限（默认 200000）
//!   --stop-after-first   每文件首命中即停
//!   --calc-hash          命中源回填 MD5/SHA256
//!   --no-deleted         不含已删除文件
//!   --partitions K,L     分区范围（卷键）
//!   --repeat N           重复次数（默认 1）
//!   --rebuild            丢弃已缓存的索引重建
//!   --explain            打印可扫源查询的执行计划与行序样本
//!   --check-context      搜索后逐条回读命中区间：重建上下文与扫描时截取的比对，
//!                        并校验命中字节按其编码解码后确实匹配模式（偏移正确）
//!   --result-digest      输出全部记录的规范化 SHA-256（忽略调度产生的 hit_seq），
//!                        用于比较优化前后的命中、编码、上下文与每文件计数
//!   --verify N           抽样 N 个文件（另加全部 ≥128 MiB 的文件），比对整文件流式读、
//!                        `read_file_range` 分片读与 `stream_file_range` 奇数分片读的
//!                        SHA-256，校验随机读路径与流式读逐字节一致
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;

use anyhow::Context;
use lovelycarve_lib::core::casework::case::{
    self, CaseState, CreateCaseOptions, EvidenceRef, SnapshotPublish,
};
use lovelycarve_lib::core::casework::case_ledger_volumes::{self, IndexedVolume};
use lovelycarve_lib::core::catalog::filedb::filedb_store::{self, IndexSource};
use lovelycarve_lib::core::catalog::snapshot::volume_snapshot::{
    build_volume_snapshot, DEFAULT_MAX_SNAPSHOT_ENTRIES,
};
use lovelycarve_lib::core::filesystem::evidence_session::EvidenceSession;
use lovelycarve_lib::core::filesystem::fs_all::FsAll;
use lovelycarve_lib::core::filesystem::tree::fs_tree::FsSession;
use lovelycarve_lib::core::search::global_search::{
    self, GlobalSearchOptions, GlobalSearchParams, GlobalSearchScope,
};
use lovelycarve_lib::core::search::global_search_store;
use lovelycarve_lib::core::search::keyword::keyword_search::MatcherSet;
use lovelycarve_lib::core::{
    casework::case_ledger, casework::case_paths, catalog::filedb::filedb_file,
};
use tokio_util::sync::CancellationToken;

/// 基准固定的检材 id，与镜像句柄的运行时 id 无关，重跑可复用索引。
const BENCH_IMAGE_ID: &str = "bench";

struct Args {
    image: String,
    work_dir: PathBuf,
    keys: Vec<String>,
    regexes: Vec<String>,
    codepages: Vec<String>,
    match_case: bool,
    per_file_cap: u32,
    global_cap: u32,
    stop_after_first: bool,
    calc_hash: bool,
    search_deleted: bool,
    partitions: Vec<String>,
    repeat: usize,
    rebuild: bool,
    explain: bool,
    verify: usize,
    check_context: bool,
    result_digest: bool,
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_args() -> anyhow::Result<Args> {
    let mut raw = std::env::args().skip(1);
    let usage = "usage: bench_global_search <image> <work_dir> [options]";
    let image = raw.next().context(usage)?;
    let work_dir = PathBuf::from(raw.next().context(usage)?);
    let mut args = Args {
        image,
        work_dir,
        keys: Vec::new(),
        regexes: Vec::new(),
        codepages: split_list("UTF-8,GBK,UTF-16LE,UTF-16BE"),
        match_case: false,
        per_file_cap: 100,
        global_cap: 200_000,
        stop_after_first: false,
        calc_hash: false,
        search_deleted: true,
        partitions: Vec::new(),
        repeat: 1,
        rebuild: false,
        explain: false,
        verify: 0,
        check_context: false,
        result_digest: false,
    };
    while let Some(flag) = raw.next() {
        let mut value = |name: &str| raw.next().with_context(|| format!("{name} needs a value"));
        match flag.as_str() {
            "--keys" => args.keys = split_list(&value("--keys")?),
            "--regex" => args.regexes.push(value("--regex")?),
            "--codepages" => args.codepages = split_list(&value("--codepages")?),
            "--match-case" => args.match_case = true,
            "--per-file-cap" => args.per_file_cap = value("--per-file-cap")?.parse()?,
            "--global-cap" => args.global_cap = value("--global-cap")?.parse()?,
            "--stop-after-first" => args.stop_after_first = true,
            "--calc-hash" => args.calc_hash = true,
            "--no-deleted" => args.search_deleted = false,
            "--partitions" => args.partitions = split_list(&value("--partitions")?),
            "--repeat" => args.repeat = value("--repeat")?.parse::<usize>()?.max(1),
            "--rebuild" => args.rebuild = true,
            "--explain" => args.explain = true,
            "--verify" => args.verify = value("--verify")?.parse()?,
            "--check-context" => args.check_context = true,
            "--result-digest" => args.result_digest = true,
            other => anyhow::bail!("unknown option {other}\n{usage}"),
        }
    }
    if args.keys.is_empty() && args.regexes.is_empty() {
        args.keys.push("password".to_string());
    }
    Ok(args)
}

/// 打开（或新建）基准案件并把镜像登记为检材。
fn open_bench_case(case_dir: &Path, image: &EvidenceSession) -> anyhow::Result<CaseState> {
    let mut case = if case_paths::ledger_path(case_dir).is_file() {
        case::open_case(case_dir)?
    } else {
        case::create_case(
            case_dir,
            CreateCaseOptions {
                name: "bench_global_search".to_string(),
                ..CreateCaseOptions::default()
            },
        )?
    };
    let info = image.info();
    case.register_evidence(&EvidenceRef {
        image_id: BENCH_IMAGE_ID.to_string(),
        path: info.path,
        source_format: info.source_format,
        size_bytes: info.size_bytes,
        added_at: case_ledger::now(),
        platform: None,
        evidence_type: None,
    })?;
    Ok(case)
}

/// 每个文件系统卷建 `.dirz` 快照并导入索引，经案件发布；已有索引时跳过。
fn ensure_index(
    image: &Arc<EvidenceSession>,
    case: &mut CaseState,
    rebuild: bool,
) -> anyhow::Result<()> {
    let existing = case_ledger_volumes::indexed_volumes(&case.dir, BENCH_IMAGE_ID)?;
    if !existing.is_empty() && !rebuild {
        println!("index:   reuse {} volumes", existing.len());
        return Ok(());
    }
    let cancel = CancellationToken::new();
    let fingerprint = image.evidence_fingerprint();
    for entry in FsAll::for_image(image).entries() {
        let staged = case.staging_dir().join(format!(
            "{}.dirz",
            case_paths::volume_segment(&entry.volume_key)
        ));
        let started = Instant::now();
        let mut on_progress = |_count: u64| {};
        let built = match build_volume_snapshot(
            image,
            &entry.volume_key,
            &staged,
            DEFAULT_MAX_SNAPSHOT_ENTRIES,
            case_ledger::now(),
            &cancel,
            &mut on_progress,
        ) {
            Ok(built) => built,
            Err(err) => {
                println!(
                    "index:   skip {} ({:?}): {err:#}",
                    entry.volume_key, entry.kind
                );
                continue;
            }
        };
        let publication = case::snapshot_publication(entry.vol_id, &built, &fingerprint);
        let SnapshotPublish::Published(record) = case.publish_snapshot(&staged, publication)?
        else {
            println!("index:   {} snapshot superseded", entry.volume_key);
            continue;
        };
        let staged_index = filedb_store::staged_index_path(&case.staging_dir(), &entry.volume_key);
        let rows = filedb_store::build_index(
            &case.snapshot_path(&record),
            &staged_index,
            &IndexSource {
                image_id: BENCH_IMAGE_ID,
                volume_key: &entry.volume_key,
                vol_id: entry.vol_id,
                built_at: &record.built_at,
                fingerprint: &fingerprint,
                truncated: record.truncated,
            },
            None,
            None,
        )?;
        case.publish_index(
            BENCH_IMAGE_ID,
            &entry.volume_key,
            &record.built_at,
            &staged_index,
        )?;
        println!(
            "index:   {} ({:?}) {rows} rows in {:.1}s",
            entry.volume_key,
            entry.kind,
            started.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

/// 打印可扫源查询（与引擎 `resolve_sources` 同一条 SQL）的执行计划、耗时与
/// 行序样本：行序决定扫描时的读取局部性。
fn explain_sources(volumes: &[IndexedVolume]) -> anyhow::Result<()> {
    let sql = "SELECT nid, name, size, full_path, full_path_low, is_delete FROM filedb
         WHERE kind = 'file' AND nid NOT LIKE 'unalloc:%' AND nid NOT LIKE 'diskgap:%'";
    for volume in volumes {
        let conn = filedb_file::open_reader(&volume.index)?;
        let mut plan = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
        let details: Vec<String> = plan
            .query_map([], |row| row.get::<_, String>(3))?
            .collect::<Result<_, _>>()?;
        println!("explain: {} -> {}", volume.volume_key, details.join(" | "));
        let started = Instant::now();
        let mut stmt = conn.prepare(sql)?;
        let rows: Vec<(String, String)> = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(3)?))
            })?
            .collect::<Result<_, _>>()?;
        println!(
            "explain: {} rows in {:.2}s; first rows:",
            rows.len(),
            started.elapsed().as_secs_f64()
        );
        for (nid, path) in rows.iter().take(8) {
            println!("  {nid}  {path}");
        }
    }
    Ok(())
}
/// 三条读路径读同一文件的 SHA-256：整文件流式读、`read_file_range` 1 MiB 分片、
/// `stream_file_range` 以奇数分片（3 MiB + 17 字节）从文件头读到尾。
fn ranged_read_digests(
    session: &mut FsSession<'_>,
    node_id: &str,
    size: u64,
) -> anyhow::Result<[String; 3]> {
    use sha2::Digest as _;
    let mut streamed = sha2::Sha256::new();
    session.stream_file(node_id, 4 * 1024 * 1024, |chunk| {
        streamed.update(chunk);
        Ok(())
    })?;
    let mut ranged = sha2::Sha256::new();
    let mut offset = 0u64;
    while offset < size {
        let bytes = session.read_file_range(node_id, offset, 1024 * 1024)?;
        if bytes.is_empty() {
            break;
        }
        offset += bytes.len() as u64;
        ranged.update(&bytes);
    }
    let mut odd = sha2::Sha256::new();
    session.stream_file_range(node_id, 0, size, 3 * 1024 * 1024 + 17, |chunk| {
        odd.update(chunk);
        Ok(())
    })?;
    Ok([
        hex::encode(streamed.finalize()),
        hex::encode(ranged.finalize()),
        hex::encode(odd.finalize()),
    ])
}

fn verify_ranged_reads(
    image: &Arc<EvidenceSession>,
    volumes: &[IndexedVolume],
    sample: usize,
) -> anyhow::Result<()> {
    let mut files: Vec<(String, String, u64)> = Vec::new();
    for volume in volumes {
        let conn = filedb_file::open_reader(&volume.index)?;
        let mut stmt = conn.prepare(
            "SELECT nid, full_path, size FROM filedb
             WHERE kind = 'file' AND size > 0
             AND nid NOT LIKE 'unalloc:%' AND nid NOT LIKE 'diskgap:%'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get::<_, i64>(2)?.max(0) as u64,
            ))
        })?;
        for row in rows {
            files.push(row?);
        }
    }
    let stride = (files.len() / sample.max(1)).max(1);
    let picked: Vec<&(String, String, u64)> = files
        .iter()
        .enumerate()
        .filter(|(index, (_, _, size))| index % stride == 0 || *size >= 128 * 1024 * 1024)
        .map(|(_, file)| file)
        .collect();
    println!("verify:  {} of {} files", picked.len(), files.len());
    let started = Instant::now();
    let mut session = FsSession::new(image);
    let mut mismatches = 0usize;
    let mut failures = 0usize;
    let mut bytes = 0u64;
    for (node_id, path, size) in picked {
        match ranged_read_digests(&mut session, node_id, *size) {
            Ok([streamed, ranged, odd]) => {
                bytes += size;
                if streamed != ranged || streamed != odd {
                    mismatches += 1;
                    println!("  MISMATCH {node_id} {path} ({size} bytes)");
                }
            }
            Err(err) => {
                failures += 1;
                println!("  read failed {node_id} {path}: {err:#}");
            }
        }
    }
    println!(
        "verify:  {:.2} GiB compared in {:.1}s, {mismatches} mismatches, {failures} read failures",
        bytes as f64 / 1_073_741_824.0,
        started.elapsed().as_secs_f64()
    );
    if mismatches > 0 {
        anyhow::bail!("{mismatches} files differ between streamed and ranged reads");
    }
    Ok(())
}

/// 逐条回读命中区间（前 64 字节起、共 `len + 192` 字节）并解码，校验落库的
/// 上下文是回读文本的前缀（上下文只按行数截断，截断处必是换行）。
fn check_contexts(
    image: &Arc<EvidenceSession>,
    case_dir: &Path,
    search_id: &str,
) -> anyhow::Result<()> {
    use lovelycarve_lib::core::search::global_search_store::{
        GlobalSearchDistinct, GlobalSearchPageQuery,
    };
    use lovelycarve_lib::core::search::keyword::keyword_search::SearchEncoding;
    let started = Instant::now();
    let mut session = FsSession::new(image);
    let mut after_seq: Option<u64> = None;
    let mut checked = 0u64;
    let mut mismatches = 0u64;
    let mut full_match: std::collections::HashMap<String, Option<regex::Regex>> =
        std::collections::HashMap::new();
    loop {
        let page = global_search_store::page(
            case_dir,
            BENCH_IMAGE_ID,
            &GlobalSearchPageQuery {
                search_id: search_id.to_string(),
                skip: 0,
                limit: 2000,
                keyword: None,
                distinct: GlobalSearchDistinct::None,
                after_seq,
            },
        )?;
        let Some(last) = page.items.last() else {
            break;
        };
        after_seq = Some(last.hit_seq);
        for hit in &page.items {
            let start = hit.file_offset.saturating_sub(64);
            let raw = session.read_file_range(&hit.node_id, start, hit.match_len as usize + 192)?;
            let encoding = SearchEncoding::from_label(&hit.encoding)
                .with_context(|| format!("unknown encoding {}", hit.encoding))?;
            let decoded = encoding.decode_to_string(&raw);
            let consistent = decoded.starts_with(&hit.context_full)
                && (decoded.len() == hit.context_full.len()
                    || decoded[hit.context_full.len()..].starts_with('\n'));
            // 命中自身的字节：按命中编码解码后应等于关键词（不区分大小写）或整段匹配正则。
            let matched_start = (hit.file_offset - start) as usize;
            let matched = raw
                .get(matched_start..matched_start + hit.match_len as usize)
                .map(|bytes| encoding.decode_to_string(bytes))
                .is_some_and(|text| match hit.pattern_kind.as_str() {
                    "regex" => full_match
                        .entry(hit.pattern.clone())
                        .or_insert_with(|| {
                            regex::Regex::new(&format!("(?i)(?s)^(?:{})$", hit.pattern)).ok()
                        })
                        .as_ref()
                        .is_some_and(|re| re.is_match(&text)),
                    _ => text.to_lowercase() == hit.pattern.to_lowercase(),
                });
            if !matched {
                mismatches += 1;
                if mismatches <= 5 {
                    println!(
                        "  OFFSET MISMATCH {} @{:#x} len {} ({}): {:?}",
                        hit.file_path, hit.file_offset, hit.match_len, hit.encoding, hit.pattern
                    );
                }
            }
            if !consistent {
                mismatches += 1;
                if mismatches <= 5 {
                    println!(
                        "  CONTEXT MISMATCH {} @{:#x}: stored {:?} vs reread {:?}",
                        hit.file_path, hit.file_offset, hit.context_full, decoded
                    );
                }
            }
            checked += 1;
        }
    }
    println!(
        "context: {checked} records re-read in {:.1}s, {mismatches} mismatches",
        started.elapsed().as_secs_f64()
    );
    if mismatches > 0 {
        anyhow::bail!("{mismatches} stored contexts differ from re-read bytes");
    }
    Ok(())
}

fn result_digest(case_dir: &Path, search_id: &str) -> anyhow::Result<()> {
    use global_search_store::{GlobalSearchDistinct, GlobalSearchPageQuery};
    use sha2::Digest as _;

    let mut records = Vec::new();
    let mut after_seq = None;
    loop {
        let page = global_search_store::page(
            case_dir,
            BENCH_IMAGE_ID,
            &GlobalSearchPageQuery {
                search_id: search_id.to_string(),
                skip: 0,
                limit: 2000,
                keyword: None,
                distinct: GlobalSearchDistinct::None,
                after_seq,
            },
        )?;
        let Some(last) = page.items.last() else {
            break;
        };
        after_seq = Some(last.hit_seq);
        for mut record in page.items {
            record.hit_seq = 0;
            records.push(serde_json::to_vec(&record)?);
        }
    }
    records.sort();
    let mut hash = sha2::Sha256::new();
    for record in &records {
        hash.update((record.len() as u64).to_le_bytes());
        hash.update(record);
    }
    println!(
        "digest:  {} records {}",
        records.len(),
        hex::encode(hash.finalize())
    );
    Ok(())
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();
    let args = parse_args()?;
    let case_dir = args.work_dir.join("case");

    // 快照头记录镜像 id，导入时与索引来源校验一致——固定 id 才能跨次复用索引。
    let image = Arc::new(EvidenceSession::open_with_id(
        BENCH_IMAGE_ID.to_string(),
        &args.image,
    )?);
    println!(
        "image:   {} ({:?}, {} bytes)",
        args.image,
        image.info().source_format,
        image.info().size_bytes
    );
    let mut case = open_bench_case(&case_dir, &image)?;
    ensure_index(&image, &mut case, args.rebuild)?;
    let volumes = case_ledger_volumes::indexed_volumes(&case_dir, BENCH_IMAGE_ID)?;
    if args.explain {
        explain_sources(&volumes)?;
    }
    if args.verify > 0 {
        return verify_ranged_reads(&image, &volumes, args.verify);
    }

    let options = GlobalSearchOptions {
        keys: args.keys.clone(),
        regexes: args.regexes.clone(),
        codepages: args.codepages.clone(),
        match_case: args.match_case,
        search_deleted: args.search_deleted,
        search_empty_space: false,
        calc_hash: args.calc_hash,
        scope: GlobalSearchScope {
            partitions: args.partitions.clone(),
            tags: Vec::new(),
            tree_nodes: Vec::new(),
        },
        per_file_cap: args.per_file_cap,
        stop_after_first: args.stop_after_first,
        global_cap: args.global_cap,
    };
    let encodings = global_search::resolve_encodings(&options.codepages)?;
    let matchers = MatcherSet::build_plan(&args.keys, &args.regexes, args.match_case, &encodings)
        .map_err(|err| anyhow::anyhow!(err))?;
    let mut patterns = args.keys.clone();
    patterns.extend(args.regexes.iter().cloned());
    println!(
        "search:  keys={:?} regexes={:?} codepages={:?} match_case={} per_file_cap={} global_cap={} calc_hash={}",
        args.keys,
        args.regexes,
        args.codepages,
        args.match_case,
        args.per_file_cap,
        args.global_cap,
        args.calc_hash
    );

    for run in 0..args.repeat {
        let search_id = format!("bench-{}-{run}", std::process::id());
        let cancel = CancellationToken::new();
        let started = Instant::now();
        let last_report = Mutex::new(Instant::now());
        let on_progress = |done: u64, total: u64, hits: u64, matched: u64| {
            let Ok(mut last) = last_report.lock() else {
                return;
            };
            if last.elapsed().as_secs_f64() >= 5.0 {
                *last = Instant::now();
                eprintln!(
                    "  .. {:>6.1}s {done}/{total} files, {hits} hits in {matched} files",
                    started.elapsed().as_secs_f64()
                );
            }
        };
        let params = GlobalSearchParams {
            image: &image,
            case_dir: &case_dir,
            image_id: BENCH_IMAGE_ID,
            search_id: &search_id,
            options: &options,
            patterns: &patterns,
            keyword_count: args.keys.len(),
            matchers: &matchers,
            cancel_flag: &cancel,
            on_progress: &on_progress,
        };
        let summary = global_search::run_global_search(&params)?;
        let elapsed = started.elapsed();
        println!(
            "run {run}: {:.2}s status={} files={}/{} matched_files={} matches={} records={} read_errors={} truncated={}",
            elapsed.as_secs_f64(),
            summary.status,
            summary.files_scanned,
            summary.files_total,
            summary.matched_file_count,
            summary.total_matches,
            summary.record_count,
            summary.read_errors,
            summary.truncated
        );
        if args.check_context {
            check_contexts(&image, &case_dir, &search_id)?;
        }
        if args.result_digest {
            result_digest(&case_dir, &search_id)?;
        }
        global_search_store::delete(&case_dir, BENCH_IMAGE_ID, &search_id)?;
    }
    Ok(())
}
