//! 全局搜索 IPC:后台搜索任务与持久化命中结果的命令面。
//!
//! `global_search_start` 把一次搜索注册为后台任务（`TaskKind::GlobalSearch`，
//! 可 `taskCancel`），任务行即时落库（`running`）——任务即历史；命中经
//! [`crate::core::search::global_search_store`] 流式落库，前端用
//! `global_search_hits_page` 服务端分页 + `after_seq` 增量轮询。
//! 进度经 `global-search-progress`
//! 事件与通用 `task-progress` 双通道上抛。取消后已落库的部分结果保留
//! （任务行改 `cancelled`）。导出复用与表格一致的过滤/去重条件。

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::host::{AppHandle, State};
use serde::Serialize;

use crate::app::error::AppError;
use crate::core::search::global_search::{
    self, GlobalSearchOptions, GlobalSearchParams, GlobalSearchSummary,
};
use crate::core::search::global_search_store::{
    self, GlobalSearchDistinct, GlobalSearchMeta, GlobalSearchPage, GlobalSearchPageQuery,
};
use crate::core::search::keyword::keyword_search::MatcherSet;
use crate::state::{AppState, GlobalSearchRunState};
use crate::task_manager::{TaskCount, TaskFlag, TaskKind, TaskOutcome, TaskProgressUnit};
use {crate::app::access::loaded_image_app_error, crate::commands::quote_csv_field};

/// 进度事件（前端按 `search_id`/`image_id` 过滤）。
pub const GLOBAL_SEARCH_PROGRESS_EVENT: &str = "global-search-progress";

/// 命中分页上限，供滚动预取。
const PAGE_MAX_LIMIT: u64 = 2000;
const PAGE_DEFAULT_LIMIT: u64 = 200;
/// 删除运行中搜索时等待扫描停止的上限。
const DELETE_CANCEL_WAIT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize)]
pub struct GlobalSearchProgress {
    pub search_id: String,
    pub image_id: String,
    pub files_done: u64,
    pub files_total: u64,
    pub hits: u64,
    pub matched_files: u64,
}

#[derive(Debug, Serialize)]
pub struct GlobalSearchExportResult {
    pub output_path: String,
    pub record_count: u64,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// 关键词/正则去重（保留首个顺序），trim 后为空的行丢弃。
fn normalize_patterns(raw: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for item in raw {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        if seen.insert(trimmed.to_string()) {
            out.push(trimmed.to_string());
        }
    }
    out
}

/// 启动一次全局搜索任务。长任务：命令在扫描完成后返回摘要（含
/// `status=cancelled` 的部分结果摘要）；取消走 `taskCancel({taskId: searchId})`。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn global_search_start(
    app: AppHandle,
    state: State<'_, AppState>,
    image_id: String,
    search_id: String,
    options: GlobalSearchOptions,
) -> Result<GlobalSearchSummary, AppError> {
    let keywords = normalize_patterns(&options.keys);
    let regexes = normalize_patterns(&options.regexes);
    if keywords.is_empty() && regexes.is_empty() {
        return Err(AppError::invalid_input("至少需要一个关键词或正则"));
    }
    let encodings = global_search::resolve_encodings(&options.codepages)
        .map_err(|err| AppError::invalid_input(err.to_string()))?;
    let mut patterns = keywords.clone();
    patterns.extend(regexes.iter().cloned());
    let keyword_count = keywords.len();
    let matchers = MatcherSet::build_plan(&keywords, &regexes, options.match_case, &encodings)
        .map_err(AppError::invalid_input)?;

    // 注册取消令牌在任何 await 之前（早到的取消不能落进注册间隙）。
    let task = state.tasks.register(
        search_id.clone(),
        TaskKind::GlobalSearch,
        Some(image_id.clone()),
    );
    task.update(|view| view.subject = patterns.clone());
    let resource_permit = state.tasks.acquire_resources(&task).await?;
    let token = task.token();
    let started = Instant::now();
    tracing::info!(
        "global_search_start begin image={image_id} search={search_id} keys={} regexes={} codepages={} scope_parts={} tags={} nodes={}",
        keywords.len(),
        regexes.len(),
        encodings.len(),
        options.scope.partitions.len(),
        options.scope.tags.len(),
        options.scope.tree_nodes.len()
    );

    let image = loaded_image_app_error(&state, &image_id).await?;
    let case_dir = {
        let active = state.active_case.lock().await;
        let case = active
            .as_ref()
            .ok_or_else(|| AppError::invalid_input("全局搜索需要先打开案件"))?;
        if !case.has_evidence(&image_id) {
            return Err(AppError::invalid_input("检材不在当前案件中"));
        }
        case.dir.clone()
    };

    // 任务行先行落库：启动即可见（任务即历史）。
    let started_ms = now_ms();
    let meta = GlobalSearchMeta {
        search_id: search_id.clone(),
        image_id: image_id.clone(),
        keywords,
        regexes,
        codepages: options.codepages.clone(),
        match_case: options.match_case,
        search_deleted: options.search_deleted,
        search_empty_space: options.search_empty_space,
        calc_hash: options.calc_hash,
        scope_json: serde_json::to_string(&serialize_scope(&options))
            .map_err(|err| AppError::from_message(format!("序列化搜索范围失败：{err}")))?,
        status: "running".to_string(),
        error: None,
        started_ms,
        finished_ms: None,
        files_total: 0,
        files_scanned: 0,
        matched_file_count: 0,
        total_matches: 0,
        record_count: 0,
        read_errors: 0,
        truncated: false,
    };
    // 先登记在跑、再落 running 行：任务列表先读行、后取存活集，读到的
    // running 行必然已登记，刚启动的搜索不会被当成孤儿改判 failed。
    state.global_searches.lock().await.insert(
        search_id.clone(),
        GlobalSearchRunState {
            image_id: image_id.clone(),
            started_ms,
            files_done: 0,
            files_total: 0,
            hits: 0,
            matched_files: 0,
        },
    );
    let init_meta = meta.clone();
    let init_case_dir = case_dir.clone();
    let created = match tokio::task::spawn_blocking(move || {
        global_search_store::create_running(&init_case_dir, &init_meta)
    })
    .await
    {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(AppError::from_anyhow(err.context("创建搜索任务记录失败"))),
        Err(err) => Err(AppError::task_panicked("global_search_start", err)),
    };
    if let Err(error) = created {
        state.global_searches.lock().await.remove(&search_id);
        return Err(error);
    }

    let log_image_id = image_id.clone();
    let log_search_id = search_id.clone();
    let runs = Arc::clone(&state.global_searches);
    let tasks = state.tasks.clone();
    let app_for_progress = app.clone();
    // 闭包拿走扫描所需的克隆，原值留给收尾（任务行落终态、登记移除）。
    let scan_image = Arc::clone(&image);
    let scan_case_dir = case_dir.clone();
    let scan_image_id = image_id.clone();
    let scan_search_id = search_id.clone();
    let outcome = tokio::task::spawn_blocking(move || -> anyhow::Result<GlobalSearchSummary> {
        let _resource_permit = resource_permit;
        let on_progress = |files_done: u64, files_total: u64, hits: u64, matched_files: u64| {
            let progress = GlobalSearchProgress {
                search_id: scan_search_id.clone(),
                image_id: scan_image_id.clone(),
                files_done,
                files_total,
                hits,
                matched_files,
            };
            if let Err(err) = app_for_progress.emit(GLOBAL_SEARCH_PROGRESS_EVENT, &progress) {
                tracing::warn!("emit {GLOBAL_SEARCH_PROGRESS_EVENT} failed: {err}");
            }
            tasks.update(&progress.search_id, |view| {
                view.counts.insert(TaskCount::Hits, hits);
                view.counts.insert(TaskCount::MatchedFiles, matched_files);
            });
            tasks.emit_progress(
                &progress.search_id,
                TaskProgressUnit::Entries,
                files_done,
                Some(files_total),
                None,
            );
            if let Ok(mut runs) = runs.try_lock() {
                if let Some(run) = runs.get_mut(&progress.search_id) {
                    run.files_done = files_done;
                    run.files_total = files_total;
                    run.hits = hits;
                    run.matched_files = matched_files;
                }
            }
        };
        let params = GlobalSearchParams {
            image: &scan_image,
            case_dir: &scan_case_dir,
            image_id: &scan_image_id,
            search_id: &scan_search_id,
            options: &options,
            patterns: &patterns,
            keyword_count,
            matchers: &matchers,
            cancel_flag: &token,
            on_progress: &on_progress,
        };
        global_search::run_global_search(&params)
    })
    .await;
    let scanned = match outcome {
        Ok(Ok(summary)) => Ok(summary),
        Ok(Err(err)) => Err(AppError::from_anyhow(err.context("全局搜索执行失败"))),
        Err(err) => Err(AppError::task_panicked("global_search_start", err)),
    };

    // 收尾：任务行改终态（扫描失败也落 failed，不留 running 行）+ 滚动清理
    // 旧任务；终态落库之后才移除在跑登记，任务列表据此判孤儿。
    let mut final_meta = meta;
    final_meta.finished_ms = Some(now_ms());
    match &scanned {
        Ok(summary) => {
            final_meta.status = summary.status.clone();
            final_meta.files_total = summary.files_total;
            final_meta.files_scanned = summary.files_scanned;
            final_meta.matched_file_count = summary.matched_file_count;
            final_meta.total_matches = summary.total_matches;
            final_meta.read_errors = summary.read_errors;
            final_meta.truncated = summary.truncated;
        }
        Err(error) => {
            final_meta.status = "failed".to_string();
            final_meta.error = Some(error.message.clone());
        }
    }
    let finish_case_dir = case_dir.clone();
    let finish_meta = final_meta.clone();
    let finish_result = tokio::task::spawn_blocking(move || {
        global_search_store::finish(&finish_case_dir, &finish_meta)
    })
    .await;
    // 任务行收尾失败不吞掉搜索结果——记 warn，历史里该行保持 running，
    // 登记移除后由任务列表的孤儿修复改判 failed。
    match finish_result {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            tracing::warn!(
                "global_search finish meta failed image={log_image_id} search={log_search_id}: {err:#}"
            );
        }
        Err(err) => {
            tracing::warn!(
                "global_search finish meta task crashed image={log_image_id} search={log_search_id}: {err}"
            );
        }
    }
    state.global_searches.lock().await.remove(&search_id);
    match &scanned {
        Ok(summary) => {
            task.update(|view| {
                view.counts.insert(TaskCount::Hits, summary.total_matches);
                view.counts
                    .insert(TaskCount::MatchedFiles, summary.matched_file_count);
                if summary.truncated {
                    view.flags.insert(TaskFlag::Truncated);
                }
            });
            task.finish(if summary.status == "cancelled" {
                TaskOutcome::Cancelled
            } else {
                TaskOutcome::Succeeded
            });
        }
        Err(_) => task.finish_with(&scanned),
    }
    let summary = scanned?;

    tracing::info!(
        "global_search_start done image={log_image_id} search={log_search_id} status={} elapsed={:.3?} files={}/{} matched={} matches={} truncated={}",
        summary.status,
        started.elapsed(),
        summary.files_scanned,
        summary.files_total,
        summary.matched_file_count,
        summary.total_matches,
        summary.truncated
    );
    Ok(summary)
}

/// 序列化范围回显（分区/标签/树节点）。
fn serialize_scope(options: &GlobalSearchOptions) -> serde_json::Value {
    serde_json::json!({
        "partitions": options.scope.partitions,
        "tags": options.scope.tags,
        "treeNodes": options.scope.tree_nodes,
    })
}

/// 命中分页：服务端过滤/去重/增量。`distinct` 为 `file`（按文件）或
/// `sha256`（按哈希）；`after_seq` 为增量轮询游标。
#[cfg_attr(feature = "desktop", tauri::command)]
#[allow(clippy::too_many_arguments)]
pub async fn global_search_hits_page(
    state: State<'_, AppState>,
    image_id: String,
    search_id: String,
    skip: Option<u64>,
    limit: Option<u64>,
    keyword: Option<String>,
    distinct: Option<String>,
    after_seq: Option<u64>,
) -> Result<GlobalSearchPage, AppError> {
    let case_dir = active_case_dir(&state).await?;
    let query = GlobalSearchPageQuery {
        search_id,
        skip: skip.unwrap_or(0),
        limit: limit.unwrap_or(PAGE_DEFAULT_LIMIT).clamp(1, PAGE_MAX_LIMIT),
        keyword,
        distinct: GlobalSearchDistinct::parse(distinct.as_deref())
            .map_err(|err| AppError::invalid_input(err.to_string()))?,
        after_seq,
    };
    tokio::task::spawn_blocking(move || global_search_store::page(&case_dir, &image_id, &query))
        .await
        .map_err(|err| AppError::task_panicked("global_search_hits_page", err))?
        .map_err(|err| AppError::from_anyhow(err.context("读取搜索命中失败")))
}

/// 任务列表（含运行中；孤儿 running 行改判 failed）。任务面板运行期间每 2 s
/// 轮询一次：读路径不建库、不建表，孤儿修复只短等写锁（见
/// [`global_search_store::history_list`]）。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn global_search_history_list(
    state: State<'_, AppState>,
    image_id: String,
    limit: Option<u32>,
) -> Result<Vec<GlobalSearchMeta>, AppError> {
    let case_dir = active_case_dir(&state).await?;
    let limit = limit.unwrap_or(50).clamp(1, 100);
    let runs = Arc::clone(&state.global_searches);
    tokio::task::spawn_blocking(move || {
        global_search_store::history_list(&case_dir, &image_id, limit, || {
            runs.blocking_lock().keys().cloned().collect()
        })
    })
    .await
    .map_err(|err| AppError::task_panicked("global_search_history_list", err))?
    .map_err(|err| AppError::from_anyhow(err.context("读取搜索任务列表失败")))
}

/// 删除整次搜索：运行中的先取消并等扫描停下，
/// 再删除任务行与全部命中。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn global_search_history_delete(
    state: State<'_, AppState>,
    image_id: String,
    search_id: String,
) -> Result<bool, AppError> {
    let running = {
        let runs = state.global_searches.lock().await;
        runs.get(&search_id)
            .is_some_and(|run| run.image_id == image_id)
    };
    if running {
        state.tasks.cancel(&search_id);
        let deadline = Instant::now() + DELETE_CANCEL_WAIT;
        while Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let still_running = {
                let runs = state.global_searches.lock().await;
                runs.contains_key(&search_id)
            };
            if !still_running {
                break;
            }
        }
    }
    let case_dir = active_case_dir(&state).await?;
    let delete_case_dir = case_dir.clone();
    let delete_image_id = image_id.clone();
    let delete_search_id = search_id.clone();
    let deleted = tokio::task::spawn_blocking(move || {
        global_search_store::delete(&delete_case_dir, &delete_image_id, &delete_search_id)
    })
    .await
    .map_err(|err| AppError::task_panicked("global_search_history_delete", err))?
    .map_err(|err| AppError::from_anyhow(err.context("删除搜索任务失败")))?;
    state.global_searches.lock().await.remove(&search_id);
    Ok(deleted)
}

/// 导出命中为 UTF-8 BOM CSV，沿用表格当前的过滤与去重。
/// 逐行流式写入，不整表驻留内存。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn global_search_export(
    state: State<'_, AppState>,
    image_id: String,
    search_id: String,
    destination: String,
    keyword: Option<String>,
    distinct: Option<String>,
) -> Result<GlobalSearchExportResult, AppError> {
    let case_dir = active_case_dir(&state).await?;
    let query = GlobalSearchPageQuery {
        search_id,
        skip: 0,
        limit: u64::MAX,
        keyword,
        distinct: GlobalSearchDistinct::parse(distinct.as_deref())
            .map_err(|err| AppError::invalid_input(err.to_string()))?,
        after_seq: None,
    };
    tokio::task::spawn_blocking(move || -> Result<GlobalSearchExportResult, AppError> {
        let file = std::fs::File::create(&destination)
            .map_err(|err| AppError::from_io("导出全局搜索结果", err))?;
        let mut writer = std::io::BufWriter::new(file);
        writer
            .write_all("\u{FEFF}序号,偏移,长度,编码,类型,模式,文件名,路径,大小,命中数,MD5,SHA256,来源,上下文\r\n".as_bytes())
            .map_err(|err| AppError::from_io("导出全局搜索结果", err))?;
        let mut count = 0u64;
        global_search_store::export_rows(&case_dir, &image_id, &query, |hit| {
            let fields = [
                hit.hit_seq.to_string(),
                format!("{:X}h", hit.file_offset),
                format!("0x{:X}", hit.match_len),
                hit.encoding.clone(),
                if hit.pattern_kind == "regex" { "正则".to_string() } else { "关键词".to_string() },
                hit.pattern.clone(),
                hit.file_name.clone(),
                hit.file_path.clone(),
                hit.file_size.to_string(),
                hit.match_count.to_string(),
                hit.md5.clone(),
                hit.sha256.clone(),
                source_kind_label(&hit.source_kind).to_string(),
                hit.context_preview.clone(),
            ];
            let row = fields
                .iter()
                .map(|field| quote_csv_field(field))
                .collect::<Vec<_>>()
                .join(",");
            writer
                .write_all(row.as_bytes())
                .and_then(|_| writer.write_all(b"\r\n"))
                .map_err(|err| anyhow::anyhow!("导出全局搜索结果失败：{err}"))?;
            count += 1;
            Ok(())
        })
        .map_err(|err| AppError::from_anyhow(err.context("导出全局搜索结果失败")))?;
        writer
            .flush()
            .map_err(|err| AppError::from_io("导出全局搜索结果", err))?;
        Ok(GlobalSearchExportResult {
            output_path: destination,
            record_count: count,
        })
    })
    .await
    .map_err(|err| AppError::task_panicked("global_search_export", err))?
}

fn source_kind_label(source_kind: &str) -> &'static str {
    match source_kind {
        "unallocated" => "未分配",
        "diskgap" => "分区间隙",
        _ => "文件",
    }
}

async fn active_case_dir(state: &AppState) -> Result<PathBuf, AppError> {
    let active = state.active_case.lock().await;
    active
        .as_ref()
        .map(|case| case.dir.clone())
        .ok_or_else(|| AppError::invalid_input("全局搜索需要先打开案件"))
}
