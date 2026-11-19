// 时间线 IPC:投影物化视图的命令面。
// Wire 契约与 `commands/timeline.rs` / `core/timeline_store.rs` /
// `core/timeline_project.rs` 镜像：全部 PLAIN SNAKE_CASE 原样透传（先例：
// search.ts ↔ global_search_store.rs）。
//
// 时间线按检材存放：每个查询都带 `imageId`，检材还没有建时间线时返回空结果。
//
// 数据流：`timelineStatus` 查状态机（buildRequired 未创建 / running 正在
// 构建 / stale 有新数据未处理 / ready 就绪）+ 时间范围；`timelineStart`
// 创建重建任务（TaskKind=timelineBuild，可用 `taskCancel({ taskId:
// 'timeline-build-<imageId>' })` 取消），进度经 `timeline-progress` 事件
// 推送；`timelineCount` strftime 时间桶聚合（钻取柱状图）、`timelineDatas`
// 明细分页（默认 tl_time desc）、`timelineTypes` 两级（应用→数据类型）
// 维度清单；插件任务收尾的写路径增量投影在后端自动发生。
import { invokeCmd } from './client';
import { onEvent, type UnlistenFn } from './events';
import type { ImageId } from './ids';

/** 时间桶粒度：年/月/日/时/分/秒，默认 year。 */
export type TimelineUnit = 'year' | 'month' | 'day' | 'hour' | 'minute' | 'second';

/** 两级维度树的一个勾选对（配置弹窗「应用 → 数据类型」精确过滤）。 */
export interface TimelineTypePair {
  from_app: string;
  data_type: number;
}

/** 时间桶、明细与导出查询共享的参数族（检材由各命令的 `imageId` 指定）。 */
export interface TimelineQuery {
  /** unix 秒，闭区间。 */
  start?: number;
  end?: number;
  /** 聚合时区偏移小时数（strftime `'%+d hours'` 修饰符）。 */
  time_zone_hours: number;
  time_unit: TimelineUnit;
  /** from_app 过滤（空 = 全部；pairs 非空时忽略）。 */
  from_apps: string[];
  /** datatype 编号过滤（空 = 全部；pairs 非空时忽略）。 */
  data_types: number[];
  /** (from_app, data_type) 对过滤（配置弹窗两级勾选）。 */
  pairs: TimelineTypePair[];
  /** content LIKE 过滤。 */
  text?: string;
  /** 是否包含源已删除的行。 */
  show_delete: boolean;
}

/** 一条时间线投影行。 */
export interface TimelineRow {
  id: number;
  /** 源记录主键：file → filedb nid；plugin_event → event_id；USN → 卷键与日志偏移（历史引用）。 */
  oid: string;
  /** 来源任务：plugin_event → run_id；file → 空。 */
  tid: string;
  /** 所属检材（镜像 id）。 */
  eid: string;
  deleted: boolean;
  /** unix 秒。 */
  tl_time: number;
  from_app: string;
  /** 1 文件元数据；2 插件时间线；3 NTFS 变更日志。 */
  data_type: number;
  /** 时间语义标记（创建/修改/访问…；file 固定「修改」）。 */
  tl_flag: string;
  /** FullDesc 摘要。 */
  content: string;
  extra: string;
}

/** 一个聚合桶（柱状图一格）。 */
export interface TimelineBucket {
  date_str: string;
  count: number;
}

/** 明细分页结果。 */
export interface TimelineDataPage {
  items: TimelineRow[];
  total: number;
}

/** `/timeline/types` 的数据类型条目。 */
export interface TimelineTypeInfo {
  data_type: number;
  data_type_name: string;
  count: number;
}

/** `/timeline/types` 的两级维度（应用 → 数据类型）。 */
export interface TimelineTypeGroup {
  from_app: string;
  types: TimelineTypeInfo[];
}

/** 时间线投影的构建状态。 */
export type TimelineBuildStatus =
  | 'buildRequired' // 未创建（1）
  | 'running' // 正在构建（2）
  | 'stale' // 有新数据未处理（4）
  | 'ready'; // 就绪

/** `timeline_status` 响应：状态 + 时间范围 + 计数 + 构建元数据。 */
export interface TimelineStatusInfo {
  status: TimelineBuildStatus;
  start_time: number | null;
  end_time: number | null;
  record_count: number;
  meta: TimelineMetaRow | null;
}

/** 构建元数据行（后端 `TimelineMetaRow`）。 */
export interface TimelineMetaRow {
  /** running | success | cancelled | failed。 */
  status: string;
  error: string | null;
  started_ms: number;
  finished_ms: number | null;
  file_rows: number;
  event_rows: number;
  source_file_watermark: string;
  source_event_watermark: string;
}

/** 重建结果摘要（status: success | cancelled）。 */
export interface TimelineBuildSummary {
  status: string;
  files_total: number;
  file_rows: number;
  events_total: number;
  event_rows: number;
  skipped_invalid_time: number;
}

/** `timeline-progress` 事件载荷。 */
export interface TimelineProgress {
  image_id: ImageId;
  done: number;
  total: number;
  rows: number;
}

/** 检材时间线的时间范围（无行返回 null）。 */
export interface TimelineRange {
  start_time: number;
  end_time: number;
}

/** `timeline_export` 的返回（CSV 导出）。 */
export interface TimelineExportResult {
  output_path: string;
  record_count: number;
}

/** 查询状态机 + 时间范围（进页第一跳）。 */
export function timelineStatus(args: { imageId: ImageId }): Promise<TimelineStatusInfo> {
  return invokeCmd<TimelineStatusInfo>('timeline_status', args);
}

/** 创建重建任务（遍历存量源数据整体重投影；长任务，取消走
 *  `taskCancel({ taskId: 'timeline-build-' + imageId })`）。 */
export function timelineStart(args: { imageId: ImageId }): Promise<TimelineBuildSummary> {
  return invokeCmd<TimelineBuildSummary>('timeline_start', args);
}

/** 两级（应用 → 数据类型）维度清单。 */
export function timelineTypes(args: { imageId: ImageId }): Promise<TimelineTypeGroup[]> {
  return invokeCmd<TimelineTypeGroup[]>('timeline_types', args);
}

/** strftime 时间桶聚合（`limit` 缺省 = 全部桶）。 */
export function timelineCount(args: {
  imageId: ImageId;
  query: TimelineQuery;
  limit?: number;
  offset?: number;
}): Promise<TimelineBucket[]> {
  return invokeCmd<TimelineBucket[]>('timeline_count', args);
}

/** 明细分页（默认 tl_time desc；order: 'asc' | 'desc'）。 */
export function timelineDatas(args: {
  imageId: ImageId;
  query: TimelineQuery;
  skip?: number;
  limit?: number;
  order?: 'asc' | 'desc';
}): Promise<TimelineDataPage> {
  return invokeCmd<TimelineDataPage>('timeline_datas', args);
}

/** 检材时间线的时间范围 min/max(tl_time)。 */
export function timelineRange(args: { imageId: ImageId }): Promise<TimelineRange | null> {
  return invokeCmd<TimelineRange | null>('timeline_range', args);
}

/** 导出为 CSV，沿用与明细表一致的过滤条件。 */
export function timelineExport(args: {
  imageId: ImageId;
  query: TimelineQuery;
  destination: string;
}): Promise<TimelineExportResult> {
  return invokeCmd<TimelineExportResult>('timeline_export', args);
}

/** 订阅 `timeline-progress` 事件（回调内按 `image_id` 过滤）。 */
export function onTimelineProgress(
  handler: Parameters<typeof onEvent<TimelineProgress>>[1],
): Promise<UnlistenFn> {
  return onEvent<TimelineProgress>('timeline-progress', handler);
}
