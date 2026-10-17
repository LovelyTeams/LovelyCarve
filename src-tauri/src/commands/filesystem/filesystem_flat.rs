//! 文件全显(前端 `deepsearch` / 后端 `allowAllFiles`)IPC:`fs_flat_page`
//! 与 `fs_flat_ext_counts`。
//!
//! 列表请求与逐级
//! 浏览同构,只多一个布尔——置位后起点 `pid` 从「列一层」变成「以该节点
//! 为根递归平铺」;`keyword` / `skip` / `limit` / `sort` / `sortdesc` / 表达式
//! 过滤原样叠加;响应一次带回行与总数(`{data, count}`),总数来自与行同一
//! 次查询的窗口函数。前端翻页只改 `skip`,不再有任何流式事件。
//!
//! 数据源解析顺序与 `fs_children` 一致:案件卷索引 → 会话卷索引
//! → 内存卷快照 → 实时遍历。索引路径是 SQL 分页(`core::catalog::filedb::filedb_flat`);
//! 快照 / 实时路径先物化起点子树内的文件行,再用 `core::catalog::flat_query` 的同一
//! 套谓词、比较器切页——三条路径对同一查询给出同一结果。索引与快照都不
//! 下探已删除目录,其子树在这里实时拼接(结果进 `FlatWalkCache`)。

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::host::State;
use serde::Serialize;

use crate::commands::filesystem::filesystem_snapshot_index::{
    case_index_targets, session_index_targets, IndexCoverage, SnapshotIndexTarget,
};
use crate::core::catalog::filedb::filedb_flat::{self, FlatIndexPage, FlatPageQuery};
use crate::core::catalog::flat_query::{
    flat_ext_of, flat_filter_keeps, FlatFileFilter, FlatPageAssembler, FlatSort, FlatSortKey,
    FsFlatPage, FLAT_PAGE_DEFAULT_SIZE, FLAT_PAGE_MAX_SIZE, FLAT_WALK_SAFETY_CAP,
};
use crate::core::catalog::snapshot::volume_snapshot::VolumeSnapshot;
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::tree::fs_tree::{self, FsNode, FsWalkEntry, NodeKind};
use crate::state::{AppState, FlatWalkCache, FlatWalkKind, FlatWalkRows};
use {crate::app::access::loaded_image_app_error, crate::app::error::AppError};

/// 起点范围内按扩展名聚合的文件数(文件类别条的计数来源)。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FsFlatExtCounts {
    /// 小写扩展名 → 数量;无扩展名的键为空串。
    pub counts: BTreeMap<String, u64>,
    pub total: u64,
}

/// 一次全显查询解析出的数据源。索引目标可能不止一个(起点是镜像根时每个
/// 卷各一个),快照最多一个(快照按卷划分,镜像根没有覆盖快照)。
struct FlatSources {
    offline_truncated: bool,
    index_targets: Vec<SnapshotIndexTarget>,
    snapshot: Option<Arc<VolumeSnapshot>>,
}

/// 归一化后的一次全显请求。
struct FlatRequest {
    image_id: String,
    parent_id: String,
    filter: FlatFileFilter,
    sort: FlatSort,
    skip: usize,
    limit: usize,
}

#[cfg(test)]
#[path = "filesystem_flat_indexed_tests.rs"]
mod indexed_tests;

/// 文件全显的一页。`allow_all_files` 控制是否递归平铺子树;
/// 未置位的请求就是逐级浏览,应走 `fs_children`。
#[cfg_attr(feature = "desktop", tauri::command)]
#[allow(clippy::too_many_arguments)]
pub async fn fs_flat_page(
    state: State<'_, AppState>,
    image_id: String,
    parent_id: String,
    allow_all_files: bool,
    keyword: Option<String>,
    sort: Option<String>,
    sortdesc: Option<bool>,
    skip: Option<usize>,
    limit: Option<usize>,
    filter: Option<FlatFileFilter>,
    refresh: Option<bool>,
) -> Result<FsFlatPage, AppError> {
    if !allow_all_files {
        return Err(AppError::invalid_input(
            "全显请求未携带 allowAllFiles 标志;逐级浏览请使用 fs_children",
        ));
    }
    let sort_key =
        match sort.as_deref().filter(|column| !column.is_empty()) {
            None => None,
            Some(column) => Some(FlatSortKey::from_column(column).ok_or_else(|| {
                AppError::invalid_input(format!("全显不支持按列「{column}」排序"))
            })?),
        };
    let mut filter = filter.unwrap_or_default();
    if keyword.is_some() {
        filter.keyword = keyword;
    }
    let request = FlatRequest {
        image_id: image_id.clone(),
        parent_id: parent_id.clone(),
        filter: filter.normalized(),
        sort: FlatSort {
            key: sort_key,
            desc: sortdesc.unwrap_or(false),
        },
        skip: skip.unwrap_or(0),
        limit: limit
            .unwrap_or(FLAT_PAGE_DEFAULT_SIZE)
            .clamp(1, FLAT_PAGE_MAX_SIZE),
    };

    let image = loaded_image_app_error(&state, &image_id).await?;
    if refresh.unwrap_or(false) {
        state
            .flat_walk_cache
            .lock()
            .invalidate(&image_id, &parent_id);
    }
    let sources = resolve_sources(&state, &image, &image_id, &parent_id).await;
    let cache = Arc::clone(&state.flat_walk_cache);
    let started = std::time::Instant::now();
    let result =
        tokio::task::spawn_blocking(move || compute_flat_page(&image, sources, &request, &cache))
            .await
            .map_err(|err| AppError::task_panicked("fs_flat_page", err))?;
    match &result {
        Ok(page) => tracing::debug!(
            "fs_flat_page done image={image_id} parent={parent_id} elapsed={:.3?} items={} total={} truncated={}",
            started.elapsed(),
            page.items.len(),
            page.total,
            page.truncated
        ),
        Err(err) => tracing::warn!(
            "fs_flat_page err image={image_id} parent={parent_id} elapsed={:.3?} err={}",
            started.elapsed(),
            err.message
        ),
    }
    result
}

/// 起点范围内的扩展名直方图,供文件类别条在全显下按整棵子树计数(与逐级
/// 浏览按整目录计数同口径,不受列表过滤影响)。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_flat_ext_counts(
    state: State<'_, AppState>,
    image_id: String,
    parent_id: String,
) -> Result<FsFlatExtCounts, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let sources = resolve_sources(&state, &image, &image_id, &parent_id).await;
    let cache = Arc::clone(&state.flat_walk_cache);
    let image_id_for_task = image_id.clone();
    let parent_id_for_task = parent_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        compute_ext_counts(
            &image,
            sources,
            &image_id_for_task,
            &parent_id_for_task,
            &cache,
        )
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_flat_ext_counts", err))?;
    if let Err(err) = &result {
        tracing::warn!(
            "fs_flat_ext_counts err image={image_id} parent={parent_id} err={}",
            err.message
        );
    }
    result
}

/// 与 `fs_children` 同序解析数据源。只有完整(未截断)的索引 / 快照才能回答
/// 全显:截断的快照记录了它从未展开的目录,平铺时无法与真正的空目录区分,
/// 会悄悄丢掉整棵子树——取证列表不能接受,退回实时遍历。
/// 离线会话只显示已索引文件,允许截断索引但明确标记覆盖不完整,不实时补读。
async fn resolve_sources(
    state: &State<'_, AppState>,
    image: &EvidenceSession,
    image_id: &str,
    parent_id: &str,
) -> FlatSources {
    let (index_targets, snapshot, has_case, offline_truncated) = {
        let active = state.active_case.lock().await;
        let has_case = active.is_some();
        let index_targets = active
            .as_ref()
            .map(|case| {
                case_index_targets(
                    case,
                    image,
                    image_id,
                    if image.is_index_only() {
                        IndexCoverage::Any
                    } else {
                        IndexCoverage::Complete
                    },
                )
            })
            .unwrap_or_default();
        let snapshot = active.as_ref().and_then(|case| {
            let snapshot = case.covering_snapshot(image_id, parent_id)?;
            case.volume(image_id, &snapshot.header.volume_key)
                .is_some_and(|record| !record.truncated)
                .then_some(snapshot)
        });
        let offline_truncated = image.is_index_only()
            && active.as_ref().is_some_and(|case| {
                case.volumes().iter().any(|record| {
                    record.image_id == image_id
                        && (record.truncated
                            || !index_targets
                                .iter()
                                .any(|target| target.volume_key == record.volume_key))
                })
            });
        (index_targets, snapshot, has_case, offline_truncated)
    };
    // 开着案件时案件是唯一权威:案件快照覆盖但截断的起点必须实时遍历,不能
    // 再被会话快照回答。
    if has_case {
        return FlatSources {
            offline_truncated,
            index_targets,
            snapshot,
        };
    }
    let index_targets =
        session_index_targets(state, image, image_id, IndexCoverage::Complete).await;
    let snapshot = {
        let sessions = state.session_snapshots.lock().await;
        crate::app::workflows::snapshot::snapshot_session::covering_session_snapshot(
            &sessions, image_id, parent_id,
        )
        .filter(|snapshot| !snapshot.truncated)
        .and_then(|snapshot| snapshot.body.clone())
    };
    FlatSources {
        offline_truncated: false,
        index_targets,
        snapshot,
    }
}

fn is_flat_row(node: &FsNode) -> bool {
    matches!(node.kind, NodeKind::File | NodeKind::Symlink)
}

fn compute_flat_page(
    image: &Arc<EvidenceSession>,
    sources: FlatSources,
    request: &FlatRequest,
    cache: &parking_lot::Mutex<FlatWalkCache>,
) -> Result<FsFlatPage, AppError> {
    let mut index_pages: Vec<FlatIndexPage> = Vec::new();
    let force_window = sources.index_targets.len() > 1;
    // 拼接行数已知(缓存命中)时,把窗口下界从 0 收缩到 skip - 拼接行数:
    // 参与合并区间 [skip, skip+limit) 的索引行不可能排在自身下标之前。
    let spliced_hint = cache
        .lock()
        .get(
            &request.image_id,
            &request.parent_id,
            FlatWalkKind::DeletedSplice,
        )
        .map(|rows| rows.entries.len())
        .unwrap_or(0);
    for target in &sources.index_targets {
        let indexed = filedb_flat::flat_page(FlatPageQuery {
            index: &target.index,
            volume_key: &target.volume_key,
            parent_id: &request.parent_id,
            filter: &request.filter,
            sort: request.sort,
            skip: request.skip,
            limit: request.limit,
            force_window,
            window_floor: request.skip.saturating_sub(spliced_hint),
        });
        match indexed {
            Ok(Some(page)) => index_pages.push(page),
            Ok(None) => {}
            Err(err) => tracing::warn!(
                "fs_flat_page index err image={} parent={} volume={}: {err:#}",
                request.image_id,
                request.parent_id,
                target.volume_key
            ),
        }
    }
    if index_pages.len() == 1 && !index_pages[0].windowed {
        // 单卷、范围内无已删目录:SQL 已直接切好页(LIMIT/OFFSET + 窗口 count)。
        let page = index_pages.remove(0);
        tracing::debug!(
            "fs_flat_page index-hit image={} parent={} total={}",
            request.image_id,
            request.parent_id,
            page.total
        );
        return Ok(FsFlatPage {
            items: page.items,
            total: page.total,
            truncated: sources.offline_truncated,
        });
    }
    if !index_pages.is_empty() {
        let mut assembler = FlatPageAssembler::new(request.sort, request.skip, request.limit);
        let mut offline_truncated = sources.offline_truncated;
        for page in index_pages {
            if image.is_index_only() && !page.deleted_dirs.is_empty() {
                offline_truncated = true;
            }
            if !image.is_index_only() && !page.deleted_dirs.is_empty() {
                let spliced = deleted_splice_rows(
                    image,
                    &request.image_id,
                    &request.parent_id,
                    page.deleted_dirs,
                    cache,
                )?;
                let (rows, total) = filtered_sorted(&spliced.entries, request);
                assembler.add_sorted_source(rows, total, spliced.truncated);
            }
            assembler.add_sorted_source(page.items, page.total, false);
        }
        tracing::debug!(
            "fs_flat_page index-merge image={} parent={}",
            request.image_id,
            request.parent_id
        );
        let mut page = assembler.finish();
        page.truncated |= offline_truncated;
        return Ok(page);
    }

    if image.is_index_only() {
        return Err(AppError::not_found(
            "此目录未收录在已有索引中，原镜像不可用。",
        ));
    }
    if let Some(snapshot) = &sources.snapshot {
        if let Some((entries, mut truncated)) =
            snapshot.flat_descendants(&request.parent_id, FLAT_WALK_SAFETY_CAP)
        {
            let deleted_dirs: Vec<(String, String)> = entries
                .iter()
                .filter(|entry| entry.node.kind == NodeKind::Dir && entry.node.deleted)
                .map(|entry| (entry.node.id.clone(), entry.path.clone()))
                .collect();
            let mut rows: Vec<FsWalkEntry> = entries
                .into_iter()
                .filter(|entry| is_flat_row(&entry.node))
                .collect();
            if !deleted_dirs.is_empty() && !truncated {
                let spliced = deleted_splice_rows(
                    image,
                    &request.image_id,
                    &request.parent_id,
                    deleted_dirs,
                    cache,
                )?;
                rows.extend(spliced.entries.iter().cloned());
                truncated |= spliced.truncated;
            }
            tracing::debug!(
                "fs_flat_page snapshot-hit image={} parent={} rows={}",
                request.image_id,
                request.parent_id,
                rows.len()
            );
            return Ok(page_from_rows(&rows, request, truncated));
        }
    }

    let walked = subtree_walk_rows(image, &request.image_id, &request.parent_id, cache)?;
    tracing::debug!(
        "fs_flat_page live-walk image={} parent={} rows={} truncated={}",
        request.image_id,
        request.parent_id,
        walked.entries.len(),
        walked.truncated
    );
    Ok(page_from_rows(&walked.entries, request, walked.truncated))
}

/// 内存行 → 过滤 + 排序后的全部行与总数(装配器的一个来源)。
fn filtered_sorted(rows: &[FsWalkEntry], request: &FlatRequest) -> (Vec<FsWalkEntry>, u64) {
    let mut kept: Vec<&FsWalkEntry> = rows
        .iter()
        .filter(|entry| flat_filter_keeps(entry, &request.filter))
        .collect();
    kept.sort_by(|left, right| request.sort.compare(left, right));
    let total = kept.len() as u64;
    (kept.into_iter().cloned().collect(), total)
}

/// 内存行 → 一页:过滤、排序、跳 `skip` 取 `limit`,总数为过滤后行数。
fn page_from_rows(rows: &[FsWalkEntry], request: &FlatRequest, truncated: bool) -> FsFlatPage {
    let mut kept: Vec<&FsWalkEntry> = rows
        .iter()
        .filter(|entry| flat_filter_keeps(entry, &request.filter))
        .collect();
    kept.sort_by(|left, right| request.sort.compare(left, right));
    let total = kept.len() as u64;
    let items = kept
        .into_iter()
        .skip(request.skip)
        .take(request.limit)
        .cloned()
        .collect();
    FsFlatPage {
        items,
        total,
        truncated,
    }
}

/// 已删除目录子树的实时拼接行。索引 / 快照的构建器从不下探已删目录,而
/// 逐级浏览能实时列出它们可恢复的子项——全显必须同样可达。按起点缓存,
/// 报出的已删目录集合一变即重走。路径 = 已删目录相对起点的路径 + 子项
/// 相对该目录的路径。
fn deleted_splice_rows(
    image: &Arc<EvidenceSession>,
    image_id: &str,
    parent_id: &str,
    deleted_dirs: Vec<(String, String)>,
    cache: &parking_lot::Mutex<FlatWalkCache>,
) -> Result<Arc<FlatWalkRows>, AppError> {
    if let Some(cached) = cache
        .lock()
        .get(image_id, parent_id, FlatWalkKind::DeletedSplice)
    {
        if cached.deleted_dirs == deleted_dirs {
            return Ok(cached);
        }
    }
    let mut walker = fs_tree::FsSession::new(image);
    let no_prune = |_node: &FsNode| -> bool { false };
    let mut entries: Vec<FsWalkEntry> = Vec::new();
    let mut truncated = false;
    for (dir_id, dir_path) in &deleted_dirs {
        if truncated {
            break;
        }
        walker
            .walk_visit_from_pruned(
                dir_id,
                &mut |entry| {
                    if is_flat_row(&entry.node) {
                        entries.push(FsWalkEntry {
                            node: entry.node.clone(),
                            path: format!("{dir_path}{}", entry.path),
                        });
                    }
                    if entries.len() >= FLAT_WALK_SAFETY_CAP {
                        truncated = true;
                        return Ok(false);
                    }
                    Ok(true)
                },
                &no_prune,
            )
            .map_err(AppError::from_anyhow)?;
    }
    let rows = Arc::new(FlatWalkRows {
        entries,
        truncated,
        deleted_dirs,
    });
    cache.lock().put(
        image_id,
        parent_id,
        FlatWalkKind::DeletedSplice,
        Arc::clone(&rows),
    );
    Ok(rows)
}

/// 索引与快照都不可用时的回退:实时遍历起点子树,只留文件行,结果按起点
/// 缓存供翻页 / 排序 / 过滤复用。5,000,000 条安全阀命中即截断。
fn subtree_walk_rows(
    image: &Arc<EvidenceSession>,
    image_id: &str,
    parent_id: &str,
    cache: &parking_lot::Mutex<FlatWalkCache>,
) -> Result<Arc<FlatWalkRows>, AppError> {
    if let Some(cached) = cache.lock().get(image_id, parent_id, FlatWalkKind::Subtree) {
        return Ok(cached);
    }
    let mut walker = fs_tree::FsSession::new(image);
    let no_prune = |_node: &FsNode| -> bool { false };
    let mut entries: Vec<FsWalkEntry> = Vec::new();
    let mut truncated = false;
    walker
        .walk_visit_from_pruned(
            parent_id,
            &mut |entry| {
                if is_flat_row(&entry.node) {
                    entries.push(entry.clone());
                }
                if entries.len() >= FLAT_WALK_SAFETY_CAP {
                    truncated = true;
                    return Ok(false);
                }
                Ok(true)
            },
            &no_prune,
        )
        .map_err(AppError::from_anyhow)?;
    let rows = Arc::new(FlatWalkRows {
        entries,
        truncated,
        deleted_dirs: Vec::new(),
    });
    cache.lock().put(
        image_id,
        parent_id,
        FlatWalkKind::Subtree,
        Arc::clone(&rows),
    );
    Ok(rows)
}

fn add_rows_to_histogram(rows: &[FsWalkEntry], summary: &mut FsFlatExtCounts) {
    for entry in rows {
        if entry.node.kind != NodeKind::File {
            continue;
        }
        *summary.counts.entry(flat_ext_of(&entry.node)).or_insert(0) += 1;
        summary.total += 1;
    }
}

fn compute_ext_counts(
    image: &Arc<EvidenceSession>,
    sources: FlatSources,
    image_id: &str,
    parent_id: &str,
    cache: &parking_lot::Mutex<FlatWalkCache>,
) -> Result<FsFlatExtCounts, AppError> {
    let mut summary = FsFlatExtCounts::default();
    let mut index_hit = false;
    for target in &sources.index_targets {
        match filedb_flat::flat_ext_counts(&target.index, &target.volume_key, parent_id) {
            Ok(Some(counts)) => {
                index_hit = true;
                for (extension, count) in counts.counts {
                    *summary.counts.entry(extension).or_insert(0) += count;
                }
                summary.total += counts.total;
                if !image.is_index_only() && !counts.deleted_dirs.is_empty() {
                    let spliced =
                        deleted_splice_rows(image, image_id, parent_id, counts.deleted_dirs, cache)?;
                    add_rows_to_histogram(&spliced.entries, &mut summary);
                }
            }
            Ok(None) => {}
            Err(err) => tracing::warn!(
                "fs_flat_ext_counts index err image={image_id} parent={parent_id} volume={}: {err:#}",
                target.volume_key
            ),
        }
    }
    if index_hit {
        return Ok(summary);
    }

    if let Some(snapshot) = &sources.snapshot {
        if let Some((entries, truncated)) =
            snapshot.flat_descendants(parent_id, FLAT_WALK_SAFETY_CAP)
        {
            let deleted_dirs: Vec<(String, String)> = entries
                .iter()
                .filter(|entry| entry.node.kind == NodeKind::Dir && entry.node.deleted)
                .map(|entry| (entry.node.id.clone(), entry.path.clone()))
                .collect();
            add_rows_to_histogram(&entries, &mut summary);
            if !deleted_dirs.is_empty() && !truncated {
                let spliced = deleted_splice_rows(image, image_id, parent_id, deleted_dirs, cache)?;
                add_rows_to_histogram(&spliced.entries, &mut summary);
            }
            return Ok(summary);
        }
    }

    let walked = subtree_walk_rows(image, image_id, parent_id, cache)?;
    add_rows_to_histogram(&walked.entries, &mut summary);
    Ok(summary)
}
