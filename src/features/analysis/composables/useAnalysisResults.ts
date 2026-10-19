
import { tr } from '../../../composables/useLocale';
// 分析结果页状态层:树 检材→分类→分析器运行→记录类型。选中记录类型 → 服务端
// 分页表格(列与字段类型来自插件声明,可按列排序),点行 → 右侧详情;选中运行
// → 运行审计(输入、选项、画像事实、告警与读不出的源字节)。任务完成只"标脏",
// 面板激活时整树重拉(WS 只发信号不搬数据,前端无状态视图)。
import { computed, effectScope, ref, shallowRef, watch, type EffectScope } from 'vue';

import {
  analysisRecordDetail,
  analysisRecordsPage,
  analysisResultTree,
  analysisRunGet,
  type AnalysisRecord,
  type AnalysisResultNode,
  type AnalysisRunAudit,
  type RecordColumn,
  type RecordPropertyLayout,
} from '../../../lib/ipc/analysisResults';
import { buildAnalysisGroups } from '../model/analysisResultView';
import { analysisCaseDirectory, analysisCompletedRuns } from './analysisContext';

/** 分析结果表格的分页大小。 */
export const ANALYSIS_PAGE_SIZE = 100;

/** Per-export cancellation and page progress, independent of the visible result selection. */
export interface AnalysisExportOptions {
  signal?: AbortSignal;
  onProgress?: (progress: { loaded: number; total: number }) => void;
}

/** The table order: a column id (or `title`) and its direction. */
export interface AnalysisSort {
  column: string;
  descending: boolean;
}

// ── 模块级单例状态(面板卸载后保留,激活时按脏标记重拉) ────────────────
const tree = shallowRef<AnalysisResultNode[]>([]);
const treeLoading = ref(false);
const treeError = ref<string | null>(null);
/** 任务事件置脏;激活/挂载时 `ensureFresh` 整树重拉。 */
const dirty = ref(true);

/** A record type (`result`, the table) or a run (`function`, its audit). */
const selectedNode = shallowRef<AnalysisResultNode | null>(null);
const records = shallowRef<AnalysisRecord[]>([]);
const columns = shallowRef<RecordColumn[]>([]);
const properties = shallowRef<RecordPropertyLayout | null>(null);
const recordsTotal = ref(0);
const pageIndex = ref(0);
const keyword = ref('');
const sort = shallowRef<AnalysisSort | null>(null);
const recordsLoading = ref(false);
const detail = shallowRef<AnalysisRecord | null>(null);
const detailColumns = shallowRef<RecordColumn[]>([]);
const recordsError = ref<string | null>(null);
const detailLoading = ref(false);
const detailError = ref<string | null>(null);
const selectedRecordId = ref<string | null>(null);
const audit = shallowRef<AnalysisRunAudit | null>(null);
const auditLoading = ref(false);
const auditError = ref<string | null>(null);
const currentImageId = ref<string | null>(null);
let treeRequestId = 0;
let recordsRequestId = 0;
let detailRequestId = 0;
let auditRequestId = 0;
let caseVersion = 0;
/** Run ID or fully qualified function whose results the next tree load opens (任务中心「查看分析结果」). */
let requestedAnalyzer: string | null = null;

/** Show one run or fully qualified function's results instead of the default first populated item.
 *  Marks the tree dirty, so the panel reloads it and the new run is included. */
export function focusAnalysisResult(analyzer: string): void {
  requestedAnalyzer = analyzer;
  dirty.value = true;
}

/** Results with records come first; an analyzer without any still opens its first item. */
function firstPopulated(results: AnalysisResultNode[]): AnalysisResultNode | null {
  return results.find(node => node.childCount > 0) ?? results[0] ?? null;
}

function closeDetail(): void {
  detailRequestId += 1;
  detail.value = null;
  detailColumns.value = [];
  detailLoading.value = false;
  detailError.value = null;
  selectedRecordId.value = null;
}

function clearSelection(): void {
  recordsRequestId += 1;
  auditRequestId += 1;
  selectedNode.value = null;
  records.value = [];
  columns.value = [];
  properties.value = null;
  recordsTotal.value = 0;
  pageIndex.value = 0;
  keyword.value = '';
  sort.value = null;
  recordsError.value = null;
  recordsLoading.value = false;
  audit.value = null;
  auditLoading.value = false;
  auditError.value = null;
  closeDetail();
}

/** Scope the shared directory and table to the active evidence tab. */
function setImageId(imageId: string): void {
  if (currentImageId.value === imageId) return;
  currentImageId.value = imageId;
  treeRequestId += 1;
  treeLoading.value = false;
  clearSelection();
  dirty.value = true;
}

let subscriptions: EffectScope | null = null;

/** 树里是否还有当前选中节点(重拉后保持选中)。 */
function findNode(
  nodes: AnalysisResultNode[],
  nodeId: string,
): AnalysisResultNode | null {
  for (const node of nodes) {
    if (node.nodeId === nodeId) return node;
    const child = findNode(node.children, nodeId);
    if (child) return child;
  }
  return null;
}

/** Loads what the selected node shows: its records or its run's audit. */
async function loadSelection(): Promise<void> {
  const node = selectedNode.value;
  if (node?.kind === 'function') await loadAudit();
  else await loadRecords();
}

async function refreshTree(): Promise<void> {
  const requestId = ++treeRequestId;
  const analyzer = requestedAnalyzer;
  treeLoading.value = true;
  treeError.value = null;
  try {
    const next = await analysisResultTree();
    if (requestId !== treeRequestId) return;
    tree.value = next;
    dirty.value = false;
    if (requestedAnalyzer === analyzer) requestedAnalyzer = null;
    const groups = currentImageId.value ? buildAnalysisGroups(next, currentImageId.value) : [];
    const results = groups.flatMap(group => group.results);
    const focusedGroup = analyzer ? groups.find(group => group.run?.runId === analyzer || group.run?.function === analyzer) : undefined;
    const focused = focusedGroup ? firstPopulated(focusedGroup.results) ?? focusedGroup.run : null;
    const current = selectedNode.value;
    const again = current ? findNode(next, current.nodeId) : null;
    if (focused) {
      clearSelection();
      selectedNode.value = focused;
      await loadSelection();
    } else if (again && (!currentImageId.value || again.imageId === currentImageId.value)) {
      selectedNode.value = again;
      await loadSelection();
    } else {
      clearSelection();
      selectedNode.value = firstPopulated(results) ?? groups[0]?.run ?? null;
      if (selectedNode.value) await loadSelection();
    }
  } catch (error) {
    if (requestId === treeRequestId) treeError.value = String(error);
  } finally {
    if (requestId === treeRequestId) treeLoading.value = false;
  }
}

async function loadRecords(): Promise<void> {
  const node = selectedNode.value;
  if (!node || node.kind !== 'result') return;
  const requestId = ++recordsRequestId;
  recordsLoading.value = true;
  recordsError.value = null;
  records.value = [];
  closeDetail();
  try {
    const page = await analysisRecordsPage({
      imageId: node.imageId, runId: node.runId, recordType: node.artifactType, legacy: node.legacy,
      keyword: keyword.value.trim() || undefined,
      sort: sort.value?.column, descending: sort.value?.descending,
      skip: pageIndex.value * ANALYSIS_PAGE_SIZE, limit: ANALYSIS_PAGE_SIZE,
    });
    if (requestId !== recordsRequestId) return;
    records.value = page.items;
    columns.value = page.columns;
    properties.value = page.properties ?? null;
    recordsTotal.value = page.total;
  } catch (error) {
    if (requestId !== recordsRequestId) return;
    recordsTotal.value = 0;
    recordsError.value = String(error);
  } finally {
    if (requestId === recordsRequestId) recordsLoading.value = false;
  }
}

async function loadAudit(): Promise<void> {
  const node = selectedNode.value;
  if (!node || node.kind !== 'function') return;
  const requestId = ++auditRequestId;
  audit.value = null;
  auditError.value = null;
  if (node.legacy) {
    auditLoading.value = false;
    return;
  }
  auditLoading.value = true;
  try {
    const found = await analysisRunGet({ imageId: node.imageId, runId: node.runId });
    if (requestId !== auditRequestId) return;
    audit.value = found;
    if (!found) auditError.value = tr("该运行已不存在，请刷新结果。");
  } catch (error) {
    if (requestId === auditRequestId) auditError.value = String(error);
  } finally {
    if (requestId === auditRequestId) auditLoading.value = false;
  }
}

export function useAnalysisResults() {
  // 任务完成时只标脏,激活面板时才整树重拉。
  if (!subscriptions) {
    subscriptions = effectScope(true);
    subscriptions.run(() => {
      watch(analysisCaseDirectory, () => {
        caseVersion += 1;
        treeRequestId += 1;
        requestedAnalyzer = null;
        tree.value = [];
        treeLoading.value = false;
        treeError.value = null;
        clearSelection();
        dirty.value = true;
      }, { flush: 'sync' });
      const seenRunIds = new Set<string>();
      watch(analysisCompletedRuns, list => {
        for (const runId of list) {
          if (!runId || seenRunIds.has(runId)) continue;
          seenRunIds.add(runId);
          dirty.value = true;
        }
      });
    });
  }

  const pageCount = computed(() =>
    Math.max(1, Math.ceil(recordsTotal.value / ANALYSIS_PAGE_SIZE)),
  );

  /** 面板激活/挂载时调用:脏了才整树重拉。 */
  function ensureFresh(): void {
    if (dirty.value && !treeLoading.value) void refreshTree();
  }

  /** 选中记录类型 → 重置分页与排序加载表格;选中运行 → 读取运行审计。 */
  async function selectNode(node: AnalysisResultNode): Promise<void> {
    if (node.kind !== 'result' && node.kind !== 'function') return;
    if (selectedNode.value?.nodeId === node.nodeId) return;
    clearSelection();
    selectedNode.value = node;
    await loadSelection();
  }

  function goToPage(index: number): void {
    const clamped = Math.min(Math.max(index, 0), pageCount.value - 1);
    if (clamped === pageIndex.value) return;
    pageIndex.value = clamped;
    void loadRecords();
  }

  /** 关键词过滤,复用于导出。 */
  function applyKeyword(value: string): void {
    keyword.value = value.trim();
    pageIndex.value = 0;
    void loadRecords();
  }

  /** Orders the table by a column (`title` for the name): ascending first, then descending, then stored order. */
  function toggleSort(column: string): void {
    const current = sort.value;
    sort.value = current?.column !== column
      ? { column, descending: false }
      : current.descending ? null : { column, descending: true };
    pageIndex.value = 0;
    void loadRecords();
  }

  async function openRecord(recordId: string): Promise<void> {
    const requestId = ++detailRequestId;
    selectedRecordId.value = recordId;
    detail.value = null;
    detailColumns.value = [];
    detailError.value = null;
    detailLoading.value = true;
    // 记录属于当前结果项所在的检材(结果按检材分库存放)。
    const imageId = selectedNode.value?.imageId ?? currentImageId.value ?? '';
    try {
      const found = await analysisRecordDetail({ imageId, recordId });
      if (requestId !== detailRequestId) return;
      detail.value = found?.record ?? null;
      detailColumns.value = found?.columns ?? [];
      if (!found) detailError.value = tr("该记录已不存在，请刷新结果。");
    } catch (error) {
      if (requestId === detailRequestId) detailError.value = String(error);
    } finally {
      if (requestId === detailRequestId) detailLoading.value = false;
    }
  }

  /** 导出当前记录类型(带当前关键词与排序),逐页拉全量后拼 CSV。 */
  async function exportRecords(
    buildCsv: (items: AnalysisRecord[], columns: RecordColumn[]) => string,
    writeCsv: (csv: string) => Promise<unknown>,
    options: AnalysisExportOptions = {},
  ): Promise<number> {
    const node = selectedNode.value;
    if (!node || node.kind !== 'result') return 0;
    const exportKeyword = keyword.value.trim() || undefined;
    const exportSort = sort.value;
    const exportCaseVersion = caseVersion;
    const collected: AnalysisRecord[] = [];
    let exportColumns: RecordColumn[] = [];
    let total: number | undefined;
    const assertActive = () => {
      if (options.signal?.aborted) throw new DOMException(tr("已取消导出"), 'AbortError');
      if (exportCaseVersion !== caseVersion) throw new Error(tr("案件已切换，导出已停止。"));
    };
    while (total === undefined || collected.length < total) {
      assertActive();
      const page = await analysisRecordsPage({
        imageId: node.imageId,
        runId: node.runId,
        recordType: node.artifactType,
        legacy: node.legacy,
        keyword: exportKeyword,
        sort: exportSort?.column,
        descending: exportSort?.descending,
        skip: collected.length,
        limit: ANALYSIS_PAGE_SIZE,
      });
      assertActive();
      if (total !== undefined && total !== page.total) {
        throw new Error(tr("分析结果数量已变化，请刷新后重新导出。"));
      }
      total = page.total;
      exportColumns = page.columns;
      if (page.items.some(item => item.runId !== node.runId)) {
        throw new Error(tr("分析结果已更新，请刷新后重新导出。"));
      }
      if ((!page.items.length && collected.length < total) || collected.length + page.items.length > total) {
        throw new Error(tr("分析结果读取不完整，请刷新后重新导出。"));
      }
      collected.push(...page.items);
      options.onProgress?.({ loaded: collected.length, total });
    }
    assertActive();
    if (collected.length > 0) {
      const csv = buildCsv(collected, exportColumns);
      assertActive();
      await writeCsv(csv);
    }
    return collected.length;
  }

  /** 强制整树重拉(手动刷新按钮)。 */
  function reload(): void {
    void refreshTree();
  }

  return {
    tree,
    treeLoading,
    treeError,
    dirty,
    selectedNode,
    records,
    columns,
    properties,
    recordsTotal,
    pageIndex,
    pageCount,
    keyword,
    sort,
    recordsLoading,
    recordsError,
    detailLoading,
    detailError,
    detailColumns,
    selectedRecordId,
    audit,
    auditLoading,
    auditError,
    closeDetail,
    setImageId,
    retryRecords: loadRecords,
    retryAudit: loadAudit,
    detail,
    ensureFresh,
    selectNode,
    goToPage,
    applyKeyword,
    toggleSort,
    openRecord,
    exportRecords,
    reload,
  };
}

/** Release workspace observers and invalidate in-flight result queries. */
export function disposeAnalysisResults(): void {
  caseVersion++;
  treeRequestId += 1;
  requestedAnalyzer = null;
  clearSelection();
  currentImageId.value = null;
  tree.value = [];
  treeLoading.value = false;
  treeError.value = null;
  dirty.value = true;
  subscriptions?.stop();
  subscriptions = null;
}

export function resetAnalysisResultsForTests(): void { disposeAnalysisResults(); }
