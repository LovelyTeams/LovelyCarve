// 文件全显(deepsearch)请求的纯函数部分:把列漏斗 / 大小 / 时间的
// 结构化状态映射成后端 `FlatFileFilter`,以及分页常量与可服务端排序/过滤的
// 列集合。不碰响应式状态,便于
// 单测;DiskFileList 只负责把它拼进 `fsFlatPage` 的参数。
import type { FlatFileFilter, FlatSortColumn } from '../../../lib/ipc/fs/types';
import type { SizeFilterState, TimeColumnKey, TimeFilterState } from './filters';

/** 滚动分页的页大小。 */
export const FLAT_PAGE_SIZE = 100;
/** 跨越大段未加载区间(拖滚动条 / End 键)时的补页大小,减少往返次数;
 *  后端仍以 5000 封顶。 */
export const FLAT_CATCHUP_PAGE_SIZE = 2000;
/** 可视区间末尾距已加载末尾不足这么多行时提前拉下一页。 */
export const FLAT_PREFETCH_ROWS = 60;
/** 搜索框输入 → 自动进入 / 退出全显的去抖(与列漏斗大列表去抖同量级)。 */
export const FLAT_KEYWORD_DEBOUNCE_MS = 300;

/** 全显下由服务端应用的漏斗列。其余列(MD5 / SHA1 / SHA256 / SM3 /
 *  标签 / 识别类型)的数据住在独立存档里,索引 / 快照 / 实时遍历三条数据源
 *  无法统一过滤,全显下漏斗禁用而不是只过滤已加载页——过滤结果必须对整棵
 *  子树成立。 */
export const FLAT_SERVER_FILTER_COLUMNS: ReadonlySet<string> = new Set([
  'name',
  'ext',
  'kind',
  'size',
  'created',
  'accessed',
  'modified',
  'link',
  'inode',
  'path',
  'deleted',
]);

const FLAT_SORT_COLUMNS: ReadonlySet<string> = new Set<FlatSortColumn>([
  'kind',
  'name',
  'ext',
  'size',
  'created',
  'accessed',
  'modified',
  'link',
  'inode',
  'path',
  'deleted',
]);

/** 表格排序键 → 服务端排序列;派生列(哈希/标签等)不可服务端排序,返回 null
 *  时全显退回默认序(类型, 名称)。 */
export function flatSortColumn(key: string | null): FlatSortColumn | null {
  if (key && FLAT_SORT_COLUMNS.has(key)) return key as FlatSortColumn;
  return null;
}

/** 「已删除」布尔漏斗的取值解析,与逐级浏览的客户端过滤同口径:空 = 不过滤;
 *  `是` / `true` / `1` = 只看已删除;其它非空值 = 只看未删除。 */
export function parseDeletedFilter(raw: string): boolean | null {
  const value = raw.trim();
  if (!value) return null;
  return value === '是' || value.toLowerCase() === 'true' || value === '1';
}

export interface FlatFilterSources {
  /** 搜索框关键词,命中名称或路径。 */
  keyword: string;
  /** 各列文本 / 布尔漏斗的生效值(`effectiveFilters`)。 */
  columnFilters: Readonly<Record<string, string>>;
  sizeFilter: Pick<SizeFilterState, 'active' | 'appliedLe' | 'appliedGe' | 'appliedMod'>;
  timeFilters: Readonly<
    Record<TimeColumnKey, Pick<TimeFilterState, 'active' | 'appliedFrom' | 'appliedTo'>>
  >;
}

function textQuery(filters: Readonly<Record<string, string>>, key: string): string | null {
  const raw = filters[key] ?? '';
  // 名称漏斗带通配符时保留原样(空格可能是模式的一部分),其余去首尾空白;
  // 后端归一化时统一小写。
  const value = key === 'name' && /[*?]/.test(raw) ? raw : raw.trim();
  return value ? value : null;
}

function timeRange(
  state: Pick<TimeFilterState, 'active' | 'appliedFrom' | 'appliedTo'>,
): FlatFileFilter['timeCreated'] {
  if (!state.active) return null;
  if (state.appliedFrom == null && state.appliedTo == null) return null;
  return { from: state.appliedFrom, to: state.appliedTo };
}

/** 把当前过滤状态编成后端表达式过滤。未生效的条件一律省略,让一个空对象
 *  与「无过滤」等价。 */
export function buildFlatFilter(sources: FlatFilterSources): FlatFileFilter {
  const filter: FlatFileFilter = {};
  const keyword = sources.keyword.trim();
  if (keyword) filter.keyword = keyword;
  const name = textQuery(sources.columnFilters, 'name');
  if (name) filter.nameQuery = name;
  const path = textQuery(sources.columnFilters, 'path');
  if (path) filter.pathQuery = path;
  const ext = textQuery(sources.columnFilters, 'ext');
  if (ext) filter.extQuery = ext;
  const kind = textQuery(sources.columnFilters, 'kind');
  if (kind) filter.kindQuery = kind;
  const link = textQuery(sources.columnFilters, 'link');
  if (link) filter.linkQuery = link;
  const inode = textQuery(sources.columnFilters, 'inode');
  if (inode) filter.inodeQuery = inode;
  const deleted = parseDeletedFilter(sources.columnFilters.deleted ?? '');
  if (deleted !== null) filter.deleted = deleted;
  if (sources.sizeFilter.active) {
    if (sources.sizeFilter.appliedGe != null) filter.sizeGe = sources.sizeFilter.appliedGe;
    if (sources.sizeFilter.appliedLe != null) filter.sizeLe = sources.sizeFilter.appliedLe;
    if (sources.sizeFilter.appliedMod) {
      filter.sizeMod = {
        divisor: sources.sizeFilter.appliedMod.n,
        remainder: sources.sizeFilter.appliedMod.r,
      };
    }
  }
  const created = timeRange(sources.timeFilters.created);
  if (created) filter.timeCreated = created;
  const accessed = timeRange(sources.timeFilters.accessed);
  if (accessed) filter.timeAccessed = accessed;
  const modified = timeRange(sources.timeFilters.modified);
  if (modified) filter.timeModified = modified;
  return filter;
}

/** 全显行的稳定行键:同一节点 id(硬链接)可出现多行,用全局行号区分。 */
export function flatRowKey(index: number, nodeId: string): string {
  return `flat:${index}:${nodeId}`;
}
