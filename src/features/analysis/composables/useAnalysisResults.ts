
import { tr } from '../../../composables/useLocale';
// 分析结果页状态层:树 检材→分类→功能→结果项,
// 选中结果项 → 服务端分页表格,点行 → 右侧详情。任务完成只"标脏",
// 面板激活时整树重拉(WS 只发信号不搬数据,前端无状态视图)。
import { computed, effectScope, ref, shallowRef, watch, type EffectScope } from 'vue';

import {
  analysisRecordDetail,
  analysisRecordsPage,
  analysisResultTree,
  type AnalysisRecord,
  type AnalysisResultNode,
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

// ── 模块级单例状态(面板卸载后保留,激活时按脏标记重拉) ────────────────
const tree = shallowRef<AnalysisResultNode[]>([]);
const treeLoading = ref(false);
const treeError = ref<string | null>(null);
/** 任务事件置脏;激活/挂载时 `ensureFresh` 整树重拉。 */
const dirty = ref(true);

const selectedNode = shallowRef<AnalysisResultNode | null>(null);
const records = shallowRef<AnalysisRecord[]>([]);
const recordsTotal = ref(0);
const pageIndex = ref(0);
const keyword = ref('');
const recordsLoading = ref(false);
const detail = shallowRef<AnalysisRecord | null>(null);
const recordsError = ref<string | null>(null);
const detailLoading = ref(false);
const detailError = ref<string | null>(null);
const selectedRecordId = ref<string | null>(null);
const currentImageId = ref<string | null>(null);
let treeRequestId = 0;
let recordsRequestId = 0;
let detailRequestId = 0;
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
  detailLoading.value = false;
  detailError.value = null;
  selectedRecordId.value = null;
}

function clearSelection(): void {
  recordsRequestId += 1;
  selectedNode.value = null;
  records.value = [];
  recordsTotal.value = 0;
  pageIndex.value = 0;
  keyword.value = '';
  recordsError.value = null;
  recordsLoading.value = false;
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


// ── 树展开状态(检材/分类默认展开,功能层由用户点开) ────────────────────
const expandedIds = ref(new Set<string>());
/** 用户手动展开/折叠过的节点(覆盖默认展开规则)。 */
const touchedIds = ref(new Set<string>());

function defaultExpanded(node: AnalysisResultNode): boolean {
  return node.kind === 'evidence' || node.kind === 'class';
}

function isExpanded(node: AnalysisResultNode): boolean {
  // 必须无条件读 expandedIds.value:否则树未含被触碰节点时 treeRows
  // computed 收集不到该依赖,点击展开后不会重算(界面无反应)。
  const expanded = expandedIds.value.has(node.nodeId);
  if (!touchedIds.value.has(node.nodeId)) {
    return defaultExpanded(node);
  }
  return expanded;
}

function toggleExpanded(node: AnalysisResultNode): void {
  const collapse = isExpanded(node);
  const nextTouched = new Set(touchedIds.value);
  nextTouched.add(node.nodeId);
  const nextExpanded = new Set(expandedIds.value);
  if (collapse) nextExpanded.delete(node.nodeId);
  else nextExpanded.add(node.nodeId);
  touchedIds.value = nextTouched;
  expandedIds.value = nextExpanded;
}

/** 树的扁平渲染行(仅含当前展开层级可见的节点)。 */
export interface AnalysisTreeRow {
  node: AnalysisResultNode;
  depth: number;
}

const treeRows = computed<AnalysisTreeRow[]>(() => {
  const rows: AnalysisTreeRow[] = [];
  const walk = (nodes: AnalysisResultNode[], depth: number) => {
    for (const node of nodes) {
      rows.push({ node, depth });
      if (node.children.length > 0 && isExpanded(node)) walk(node.children, depth + 1);
    }
  };
  walk(tree.value, 0);
  return rows;
});

let subscriptions: EffectScope | null = null;

/** 树里是否还有当前选中结果项(重拉后保持选中)。 */
function findResultNode(
  nodes: AnalysisResultNode[],
  nodeId: string,
): AnalysisResultNode | null {
  for (const node of nodes) {
    if (node.nodeId === nodeId) return node;
    const child = findResultNode(node.children, nodeId);
    if (child) return child;
  }
  return null;
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
    const results = currentImageId.value ? buildAnalysisGroups(next, currentImageId.value).flatMap(group => group.results) : [];
    const focused = analyzer ? firstPopulated(results.filter(node => node.runId === analyzer || node.function === analyzer)) : null;
    const current = selectedNode.value;
    const again = current ? findResultNode(next, current.nodeId) : null;
    if (focused) {
      clearSelection();
      selectedNode.value = focused;
      await loadRecords();
    } else if (again && (!currentImageId.value || again.imageId === currentImageId.value)) {
      selectedNode.value = again;
      await loadRecords();
    } else {
      clearSelection();
      selectedNode.value = firstPopulated(results);
      if (selectedNode.value) await loadRecords();
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
      imageId: node.imageId, analyzer: node.analyzer, runId: node.runId, artifactType: node.artifactType,
      keyword: keyword.value.trim() || undefined,
      skip: pageIndex.value * ANALYSIS_PAGE_SIZE, limit: ANALYSIS_PAGE_SIZE,
    });
    if (requestId !== recordsRequestId) return;
    records.value = page.items;
    recordsTotal.value = page.total;
  } catch (error) {
    if (requestId !== recordsRequestId) return;
    recordsTotal.value = 0;
    recordsError.value = String(error);
  } finally {
    if (requestId === recordsRequestId) recordsLoading.value = false;
  }
}

export function useAnalysisResults() {
  // 任务完成时只标脏,激活面板时才整树重拉。
  if (!subscriptions) {
    subscriptions = effectScope(true);
    subscriptions.run(() => {
      // 整树重拉时重新展开；该监听随工作区单例一起释放。
      watch(tree, () => {
        touchedIds.value = new Set();
        expandedIds.value = new Set();
      });
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

  /** 选中结果项节点 → 重置分页加载表格;其他节点不选中。 */
  async function selectNode(node: AnalysisResultNode): Promise<void> {
    if (node.kind !== 'result') return;
    if (selectedNode.value?.nodeId === node.nodeId) return;
    selectedNode.value = node;
    pageIndex.value = 0;
    keyword.value = '';
    recordsTotal.value = 0;
    await loadRecords();
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

  async function openRecord(artifactId: string): Promise<void> {
    const requestId = ++detailRequestId;
    selectedRecordId.value = artifactId;
    detail.value = null;
    detailError.value = null;
    detailLoading.value = true;
    // 记录属于当前结果项所在的检材(结果按检材分库存放)。
    const imageId = selectedNode.value?.imageId ?? currentImageId.value ?? '';
    try {
      const record = await analysisRecordDetail({ imageId, artifactId });
      if (requestId !== detailRequestId) return;
      detail.value = record;
      if (!record) detailError.value = tr("该记录已不存在，请刷新结果。");
    } catch (error) {
      if (requestId === detailRequestId) detailError.value = String(error);
    } finally {
      if (requestId === detailRequestId) detailLoading.value = false;
    }
  }

  /** 导出当前结果项(带当前关键词过滤),逐页拉全量后拼 CSV。 */
  async function exportRecords(
    buildCsv: (items: AnalysisRecord[]) => string,
    writeCsv: (csv: string) => Promise<unknown>,
    options: AnalysisExportOptions = {},
  ): Promise<number> {
    const node = selectedNode.value;
    if (!node) return 0;
    const exportKeyword = keyword.value.trim() || undefined;
    const exportCaseVersion = caseVersion;
    const collected: AnalysisRecord[] = [];
    let total: number | undefined;
    const assertActive = () => {
      if (options.signal?.aborted) throw new DOMException(tr("已取消导出"), 'AbortError');
      if (exportCaseVersion !== caseVersion) throw new Error(tr("案件已切换，导出已停止。"));
    };
    while (total === undefined || collected.length < total) {
      assertActive();
      const page = await analysisRecordsPage({
        imageId: node.imageId,
        analyzer: node.analyzer, runId: node.runId,
        artifactType: node.artifactType,
        keyword: exportKeyword,
        skip: collected.length,
        limit: ANALYSIS_PAGE_SIZE,
      });
      assertActive();
      if (total !== undefined && total !== page.total) {
        throw new Error(tr("分析结果数量已变化，请刷新后重新导出。"));
      }
      total = page.total;
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
      const csv = buildCsv(collected);
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
    treeRows,
    isExpanded,
    toggleExpanded,
    selectedNode,
    records,
    recordsTotal,
    pageIndex,
    pageCount,
    keyword,
    recordsLoading,
    recordsError,
    detailLoading,
    detailError,
    selectedRecordId,
    closeDetail,
    setImageId,
    retryRecords: loadRecords,
    detail,
    ensureFresh,
    selectNode,
    goToPage,
    applyKeyword,
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
  selectedNode.value = null;
  records.value = [];
  recordsTotal.value = 0;
  pageIndex.value = 0;
  keyword.value = '';
  recordsLoading.value = false;
  detail.value = null;
  touchedIds.value = new Set();
  expandedIds.value = new Set();
  subscriptions?.stop();
  subscriptions = null;
}

export function resetAnalysisResultsForTests(): void { disposeAnalysisResults(); }
