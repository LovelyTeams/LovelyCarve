// 全局搜索 IPC:后台搜索任务与持久化命中结果的命令面。
// Wire 契约与 `commands/global_search.rs` / `core/global_search_store.rs`
// 镜像：全部 PLAIN SNAKE_CASE 原样透传（先例：fs.ts FsNode ↔ fs_tree.rs）。
//
// 数据流：`globalSearchStart` 注册后台任务（可用 `taskCancel` 取消），任务行
// 即时落库；扫描期间命中持续写库，前端用 `globalSearchHitsPage` 服务端分页
// （`afterSeq` 增量轮询），进度经
// `global-search-progress` 事件推送。历史 = 任务列表（含运行中），删除任务
// = 删除整次搜索。
import { invokeCmd } from './client';
import { onEvent, type UnlistenFn } from './events';
import type { ImageId, NodeId, VolumeKey } from './ids';

/** 编码页。空数组 = 后端按全部编码处理。 */
export type SearchCodePage = 'ASCII' | 'UTF-8' | 'UTF-16LE' | 'UTF-16BE' | 'GBK';

/** 搜索范围；全空 = 整镜像。 */
export interface GlobalSearchScope {
  /** 卷键（part:N，后端连带 lvm:N:M）。 */
  partitions: string[];
  /** 标签值勾选。 */
  tags: string[];
  /** 树节点勾选（目录 → 子树，文件 → 单文件）。 */
  tree_nodes: string[];
}

/** 一次全局搜索的参数（对应后端 `GlobalSearchOptions`）。 */
export interface GlobalSearchOptions {
  keys: string[];
  regexes: string[];
  codepages: SearchCodePage[];
  match_case: boolean;
  search_deleted: boolean;
  search_empty_space: boolean;
  calc_hash: boolean;
  scope: GlobalSearchScope;
  per_file_cap: number;
  stop_after_first: boolean;
  global_cap: number;
}

/** `global_search_start` 的返回摘要（status: success | cancelled）。 */
export interface GlobalSearchSummary {
  search_id: string;
  status: string;
  keyword_list: string[];
  regex_list: string[];
  codepages: string[];
  matched_file_count: number;
  total_matches: number;
  record_count: number;
  elapsed_ms: number;
  files_scanned: number;
  files_total: number;
  read_errors: number;
  truncated: boolean;
}

/** 一条命中记录（后端 `GlobalSearchHit` 的 wire 形状，snake_case 原样）。 */
export interface GlobalSearchHit {
  hit_seq: number;
  node_id: NodeId;
  volume_key: VolumeKey;
  file_path: string;
  file_name: string;
  file_size: number;
  /** 命中在源内的字节偏移。 */
  file_offset: number;
  match_len: number;
  encoding: string;
  pattern: string;
  /** 'keyword' | 'regex'。 */
  pattern_kind: string;
  match_count: number;
  context_preview: string;
  context_full: string;
  md5: string;
  sha256: string;
  /** 'file' | 'unallocated' | 'diskgap'。 */
  source_kind: string;
}

/** 任务/历史条目（后端 `GlobalSearchMeta`）。 */
export interface GlobalSearchHistoryEntry {
  search_id: string;
  image_id: ImageId;
  keywords: string[];
  regexes: string[];
  codepages: string[];
  match_case: boolean;
  search_deleted: boolean;
  search_empty_space: boolean;
  calc_hash: boolean;
  scope_json: string;
  /** running | success | cancelled | failed。 */
  status: string;
  error: string | null;
  started_ms: number;
  finished_ms: number | null;
  files_total: number;
  files_scanned: number;
  matched_file_count: number;
  total_matches: number;
  record_count: number;
  read_errors: number;
  truncated: boolean;
}

/** 命中分页去重维度。 */
export type GlobalSearchDistinct = 'none' | 'file' | 'sha256';

/** 分页结果：total 为同条件（含去重）总数。 */
export interface GlobalSearchPage {
  search_id: string;
  items: GlobalSearchHit[];
  total: number;
}

/** `global-search-progress` 事件载荷。 */
export interface GlobalSearchProgress {
  search_id: string;
  image_id: ImageId;
  files_done: number;
  files_total: number;
  hits: number;
  matched_files: number;
}

/** `global_search_export` 的返回（CSV 导出）。 */
export interface GlobalSearchExportResult {
  output_path: string;
  record_count: number;
}

/** 启动一次全局搜索任务。`searchId` 由调用方生成（进度事件过滤与 taskCancel
 *  用）；命令在扫描完成后返回摘要（取消返回 status=cancelled 的部分结果）。
 *  长任务，可用 `taskCancel({ taskId: searchId })` 取消。 */
export function globalSearchStart(args: {
  imageId: ImageId;
  searchId: string;
  options: GlobalSearchOptions;
}): Promise<GlobalSearchSummary> {
  return invokeCmd<GlobalSearchSummary>('global_search_start', args);
}

/** 命中分页（服务端过滤/去重/增量）。`afterSeq` 为增量轮询游标：只取
 *  hit_seq 大于该值的新命中。 */
export function globalSearchHitsPage(args: {
  imageId: ImageId;
  searchId: string;
  skip?: number;
  limit?: number;
  keyword?: string;
  distinct?: GlobalSearchDistinct;
  afterSeq?: number;
}): Promise<GlobalSearchPage> {
  return invokeCmd<GlobalSearchPage>('global_search_hits_page', args);
}

/** 任务列表（任务即历史；含运行中的任务，新→旧）。 */
export function globalSearchHistoryList(args: {
  imageId: ImageId;
  limit?: number;
}): Promise<GlobalSearchHistoryEntry[]> {
  return invokeCmd<GlobalSearchHistoryEntry[]>('global_search_history_list', args);
}

/** 删除整次搜索（运行中的先取消再删除任务与全部命中）。 */
export function globalSearchHistoryDelete(args: {
  imageId: ImageId;
  searchId: string;
}): Promise<boolean> {
  return invokeCmd<boolean>('global_search_history_delete', args);
}

/** 导出命中为 CSV，沿用与表格一致的过滤/去重条件。 */
export function globalSearchExport(args: {
  imageId: ImageId;
  searchId: string;
  destination: string;
  keyword?: string;
  distinct?: GlobalSearchDistinct;
}): Promise<GlobalSearchExportResult> {
  return invokeCmd<GlobalSearchExportResult>('global_search_export', args);
}

/** 订阅 `global-search-progress` 事件（回调内按 `search_id` 过滤）。 */
export function onGlobalSearchProgress(
  handler: Parameters<typeof onEvent<GlobalSearchProgress>>[1],
): Promise<UnlistenFn> {
  return onEvent<GlobalSearchProgress>('global-search-progress', handler);
}
