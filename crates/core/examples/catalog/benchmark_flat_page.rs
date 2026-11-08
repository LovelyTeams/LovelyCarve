//! Real-image performance probe for 文件全显 (`fs_flat_page` / `core::catalog::filedb::filedb_flat`).
//!
//! Loads a real image, builds the filedb index for one volume (largest by
//! default), then times the paged flat-query matrix against the SQLite index —
//! first page, deep offsets, keyword / glob filters, server sorts, subtree
//! scope, extension histogram — each cold (first call) and warm (second call).
//! For comparison it also times the no-index fallback (live walk + in-memory
//! filter/sort/page) once, which is what the app pays only when neither an
//! index nor a snapshot can answer.
//!
//! Usage:
//!
//! ```text
//! just run-example-fast benchmark_flat_page <image> [volume_key]
//! ```

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use lovelycarve_core::catalog::filedb::filedb_flat::{self, FlatPageQuery};
use lovelycarve_core::catalog::filedb::filedb_store::{self, IndexSource};
use lovelycarve_core::catalog::flat_query::{
    flat_filter_keeps, FlatFileFilter, FlatSort, FlatSortKey, FsFlatPage,
};
use lovelycarve_core::catalog::snapshot::volume_snapshot::{
    build_volume_snapshot, snapshot_file_name, DEFAULT_MAX_SNAPSHOT_ENTRIES,
};
use lovelycarve_core::filesystem::evidence_session::EvidenceSession;
use lovelycarve_core::filesystem::fs_all::FsAll;
use lovelycarve_core::filesystem::tree::fs_tree::{self, FsNode, FsWalkEntry, NodeKind};
use tokio_util::sync::CancellationToken;

const PAGE: usize = 100;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .context("usage: benchmark_flat_page <image> [volume_key]")?;
    let requested_volume = args.next();

    let image = Arc::new(EvidenceSession::open(&path)?);
    println!(
        "image: {path} ({:?}, {:.1} GiB) id={}",
        image.info().source_format,
        image.len() as f64 / 1024.0 / 1024.0 / 1024.0,
        image.id
    );

    let fs_all = FsAll::for_image(&image);
    let volume = requested_volume
        .map(|key| {
            fs_all
                .entries()
                .iter()
                .find(|entry| entry.volume_key == key)
                .cloned()
                .with_context(|| format!("volume {key} not in fs_all"))
        })
        .transpose()?
        .or_else(|| {
            fs_all
                .entries()
                .iter()
                .max_by_key(|entry| entry.size)
                .cloned()
        })
        .context("fs_all has no volume")?;
    let volume_key = volume.volume_key.clone();
    let vol_id = fs_all
        .vol_id_of(&volume_key)
        .with_context(|| format!("volume {volume_key} not in fs_all"))?;
    println!(
        "volume: key={volume_key} vol_id={vol_id} size={:.1} GiB (auto-picked largest)",
        volume.size as f64 / 1024.0 / 1024.0 / 1024.0
    );

    // ── 建索引(应用里由卷快照后台任务完成,这里显式走一遍)。 ──
    let work_dir = tempfile::tempdir()?;
    let snapshot_path = work_dir
        .path()
        .join(snapshot_file_name(&image.id, &volume_key));
    let cancel = CancellationToken::new();
    let mut last = 0u64;
    let mut on_progress = |count: u64| {
        if count >= last + 200_000 {
            last = count;
            println!("  .. walked {count} entries");
        }
    };
    let started = Instant::now();
    let built = build_volume_snapshot(
        &image,
        &volume_key,
        &snapshot_path,
        DEFAULT_MAX_SNAPSHOT_ENTRIES,
        "2026-09-18T00:00:00Z".to_string(),
        &cancel,
        &mut on_progress,
    )?;
    let snapshot_secs = started.elapsed().as_secs_f64();
    println!(
        "index build: walk {} entries truncated={} in {snapshot_secs:.1}s",
        built.entry_count, built.truncated
    );

    let index = work_dir.path().join("index.db");
    let started = Instant::now();
    let rows = filedb_store::build_index(
        &snapshot_path,
        &index,
        &IndexSource {
            image_id: &image.id,
            volume_key: &volume_key,
            vol_id,
            built_at: &built.header.built_at,
            fingerprint: &image.evidence_fingerprint(),
            truncated: built.truncated,
        },
        None,
        None,
    )?;
    let import_secs = started.elapsed().as_secs_f64();
    let index_bytes = std::fs::metadata(&index)?.len();
    println!(
        "index import: {rows} rows in {import_secs:.1}s (index {:.1} MiB)",
        index_bytes as f64 / 1024.0 / 1024.0
    );

    // ── 查询矩阵。每项冷/热各跑一次,报告毫秒与命中数。 ──
    println!("\nfs_flat_page against SQLite index (page size {PAGE}):");
    println!(
        "{:<44} {:>10} {:>10} {:>9} {:>12}",
        "query", "cold ms", "warm ms", "items", "total"
    );

    let empty = FlatFileFilter::default();
    let default_sort = FlatSort::default();

    let first = run_query(
        &index,
        &volume_key,
        &volume_key,
        &empty,
        default_sort,
        0,
        PAGE,
        0,
    )?;
    let total = first.total as usize;

    // 模拟应用:首页查询报出需要实时拼接的已删目录,命令层拼接一次后按起点
    // 缓存,后续深翻页用拼接行数收缩窗口下界——基准同样先拼一次再测深翻页。
    let spliced_total = if first.deleted_dirs.is_empty() {
        0
    } else {
        let started = Instant::now();
        let mut walker = fs_tree::FsSession::new(&image);
        let no_prune = |_node: &FsNode| -> bool { false };
        let mut spliced = 0usize;
        for (dir_id, _dir_path) in &first.deleted_dirs {
            walker.walk_visit_from_pruned(
                dir_id,
                &mut |entry| {
                    if matches!(entry.node.kind, NodeKind::File | NodeKind::Symlink) {
                        spliced += 1;
                    }
                    Ok(true)
                },
                &no_prune,
            )?;
        }
        println!(
            "splice walk: {spliced} rows from {} deleted dirs in {:.1}s (cached per scope in-app)",
            first.deleted_dirs.len(),
            started.elapsed().as_secs_f64()
        );
        spliced
    };

    let deep_skip = total.saturating_sub(PAGE + 1) / 2;
    let last_skip = total.saturating_sub(PAGE);
    let deep_floor = deep_skip.saturating_sub(spliced_total);
    let last_floor = last_skip.saturating_sub(spliced_total);

    run_and_report(
        &index,
        &volume_key,
        &volume_key,
        "default sort (Type,Name), page 0",
        &empty,
        default_sort,
        0,
        0,
    )?;
    run_and_report(
        &index,
        &volume_key,
        &volume_key,
        &format!("default sort, offset {deep_skip} (middle, floored window)"),
        &empty,
        default_sort,
        deep_skip,
        deep_floor,
    )?;
    run_and_report(
        &index,
        &volume_key,
        &volume_key,
        &format!("default sort, offset {last_skip} (tail, floored window)"),
        &empty,
        default_sort,
        last_skip,
        last_floor,
    )?;
    run_and_report(
        &index,
        &volume_key,
        &volume_key,
        "sort name natsort asc, page 0",
        &empty,
        FlatSort {
            key: Some(FlatSortKey::Name),
            desc: false,
        },
        0,
        0,
    )?;
    run_and_report(
        &index,
        &volume_key,
        &volume_key,
        "sort size desc, page 0",
        &empty,
        FlatSort {
            key: Some(FlatSortKey::Size),
            desc: true,
        },
        0,
        0,
    )?;
    run_and_report(
        &index,
        &volume_key,
        &volume_key,
        "sort modified desc, page 0",
        &empty,
        FlatSort {
            key: Some(FlatSortKey::Modified),
            desc: true,
        },
        0,
        0,
    )?;
    run_and_report(
        &index,
        &volume_key,
        &volume_key,
        "keyword 'e' (broad), page 0",
        &FlatFileFilter {
            keyword: Some("e".into()),
            ..FlatFileFilter::default()
        }
        .normalized(),
        default_sort,
        0,
        0,
    )?;
    run_and_report(
        &index,
        &volume_key,
        &volume_key,
        "keyword 'passwd' (sparse), page 0",
        &FlatFileFilter {
            keyword: Some("passwd".into()),
            ..FlatFileFilter::default()
        }
        .normalized(),
        default_sort,
        0,
        0,
    )?;
    run_and_report(
        &index,
        &volume_key,
        &volume_key,
        "name glob '*.conf', page 0",
        &FlatFileFilter {
            name_query: Some("*.conf".into()),
            ..FlatFileFilter::default()
        }
        .normalized(),
        default_sort,
        0,
        0,
    )?;
    run_and_report(
        &index,
        &volume_key,
        &volume_key,
        "size>=1MiB + sort size desc, page 0",
        &FlatFileFilter {
            size_ge: Some(1024 * 1024),
            ..FlatFileFilter::default()
        },
        FlatSort {
            key: Some(FlatSortKey::Size),
            desc: true,
        },
        0,
        0,
    )?;

    // 子树范围:从索引根往下找第一个多级目录(如 /usr),以其 nid 为起点。
    let subtree_scope = find_deep_directory(&index, &volume_key)?;
    if let Some(scope) = subtree_scope {
        let label = format!("subtree scope {} (page 0)", scope.1);
        run_and_report(
            &index,
            &volume_key,
            &scope.0,
            &label,
            &empty,
            default_sort,
            0,
            0,
        )?;
        let deep_in_subtree = scope.2.saturating_sub(PAGE + 1) / 2;
        run_and_report(
            &index,
            &volume_key,
            &scope.0,
            &format!("subtree scope {}, offset {deep_in_subtree}", scope.1),
            &empty,
            default_sort,
            deep_in_subtree.max(1),
            0,
        )?;
    } else {
        println!(
            "{:<44} (no multi-level directory found for subtree scope)",
            "-"
        );
    }

    // 扩展名直方图(类别条计数来源)。
    let started = Instant::now();
    let histogram = filedb_flat::flat_ext_counts(&index, &volume_key, &volume_key)?
        .context("ext counts should be covered")?;
    let histogram_cold_ms = ms(started);
    let started = Instant::now();
    let warm_histogram = filedb_flat::flat_ext_counts(&index, &volume_key, &volume_key)?
        .context("ext counts should be covered")?;
    anyhow::ensure!(
        warm_histogram.total == histogram.total,
        "warm histogram total drifted"
    );
    println!(
        "{:<44} {:>10.1} {:>10.1} {:>9} {:>12}",
        "fs_flat_ext_counts (histogram)",
        histogram_cold_ms,
        ms(started),
        histogram.counts.len(),
        histogram.total
    );
    println!(
        "deleted dirs under volume (live-splice targets): {}",
        first.deleted_dirs.len()
    );

    // ── 无索引回退:实时遍历 + 内存过滤排序分页(仅索引与快照都不可用时)。 ──
    println!("\nno-index fallback (live walk + in-memory filter/sort/page):");
    let started = Instant::now();
    let mut walker = fs_tree::FsSession::new(&image);
    let no_prune = |_node: &FsNode| -> bool { false };
    let mut walked: Vec<FsWalkEntry> = Vec::new();
    walker.walk_visit_from_pruned(
        &volume_key,
        &mut |entry| {
            if matches!(entry.node.kind, NodeKind::File | NodeKind::Symlink) {
                walked.push(entry.clone());
            }
            Ok(true)
        },
        &no_prune,
    )?;
    let walk_secs = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let fallback = page_from_rows(&walked, &empty, default_sort, 0, PAGE);
    let page_ms = ms(started);
    println!(
        "  live walk: {} file rows in {walk_secs:.1}s; filter+sort+page(100): {page_ms:.1} ms \
         (first call pays the walk, then cached per scope)",
        walked.len()
    );
    println!(
        "  fallback page-0 total = {} (index said {total})",
        fallback.total
    );

    let report = serde_json::json!({
        "image": path,
        "imageId": image.id,
        "volumeKey": volume_key,
        "snapshotEntries": built.entry_count,
        "indexRows": rows,
        "walkSeconds": snapshot_secs,
        "importSeconds": import_secs,
        "indexBytes": index_bytes,
        "flatTotal": total,
        "fallbackRows": walked.len(),
    });
    println!("\nsummary json:\n{report}");
    Ok(())
}

fn ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

#[allow(clippy::too_many_arguments)]
fn run_query(
    index: &Path,
    volume_key: &str,
    parent_id: &str,
    filter: &FlatFileFilter,
    sort: FlatSort,
    skip: usize,
    limit: usize,
    window_floor: usize,
) -> anyhow::Result<filedb_flat::FlatIndexPage> {
    filedb_flat::flat_page(FlatPageQuery {
        index,
        volume_key,
        parent_id,
        filter,
        sort,
        skip,
        limit,
        force_window: false,
        window_floor,
    })?
    .with_context(|| format!("flat page should be covered for parent {parent_id}"))
}

#[allow(clippy::too_many_arguments)]
fn run_and_report(
    index: &Path,
    volume_key: &str,
    parent_id: &str,
    label: &str,
    filter: &FlatFileFilter,
    sort: FlatSort,
    skip: usize,
    window_floor: usize,
) -> anyhow::Result<()> {
    let started = Instant::now();
    let page = run_query(
        index,
        volume_key,
        parent_id,
        filter,
        sort,
        skip,
        PAGE,
        window_floor,
    )?;
    let cold_ms = ms(started);
    let started = Instant::now();
    let warm = run_query(
        index,
        volume_key,
        parent_id,
        filter,
        sort,
        skip,
        PAGE,
        window_floor,
    )?;
    let warm_ms = ms(started);
    anyhow::ensure!(warm.total == page.total, "warm total drifted for {label}");
    println!(
        "{:<44} {:>10.1} {:>10.1} {:>9} {:>12}",
        label,
        cold_ms,
        warm_ms,
        page.items.len(),
        page.total
    );
    Ok(())
}

/// 从索引根向下找一个「其子目录仍有子目录」的目录,返回 (nid, path, subtree total),
/// 用于按真实子树范围计时。找不到返回 None。
fn find_deep_directory(
    index: &Path,
    volume_key: &str,
) -> anyhow::Result<Option<(String, String, usize)>> {
    let children = |parent: &str| -> anyhow::Result<Vec<FsNode>> {
        Ok(
            lovelycarve_core::catalog::filedb::filedb_query::children(index, parent)?
                .unwrap_or_default(),
        )
    };
    let root_dirs: Vec<FsNode> = children(volume_key)?
        .into_iter()
        .filter(|node| node.kind == NodeKind::Dir && !node.deleted)
        .collect();
    for dir in root_dirs {
        let sub_dirs: Vec<FsNode> = children(&dir.id)?
            .into_iter()
            .filter(|node| node.kind == NodeKind::Dir && !node.deleted)
            .collect();
        if sub_dirs.is_empty() {
            continue;
        }
        // 该目录下有一定体量(按其子目录再下一层的出现与否粗判),取第一个即可。
        let total = filedb_flat::flat_ext_counts(index, volume_key, &dir.id)?
            .map(|counts| counts.total)
            .unwrap_or(0);
        if total >= 1000 {
            return Ok(Some((
                dir.id.clone(),
                format!("/{}", dir.name),
                total as usize,
            )));
        }
    }
    Ok(None)
}

/// `commands::filesystem::filesystem_flat::page_from_rows` 的内存侧等价实现,用于回退路径对比。
fn page_from_rows(
    rows: &[FsWalkEntry],
    filter: &FlatFileFilter,
    sort: FlatSort,
    skip: usize,
    limit: usize,
) -> FsFlatPage {
    let mut kept: Vec<&FsWalkEntry> = rows
        .iter()
        .filter(|entry| flat_filter_keeps(entry, filter))
        .collect();
    kept.sort_by(|left, right| sort.compare(left, right));
    let total = kept.len() as u64;
    let items = kept.into_iter().skip(skip).take(limit).cloned().collect();
    FsFlatPage {
        items,
        total,
        truncated: false,
    }
}
