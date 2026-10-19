<template>
  <section class="analysis-results" :aria-label="tr('分析结果')">
    <p class="sr-only" role="status">{{ statusText }}</p>
    <!-- 搜索、阅读方式与导出共用一条工具栏，完整路径与完成时间留在页脚。 -->
    <div v-if="selectedNode" class="ui-toolbar results-toolbar">
      <span v-if="selectedNode.kind === 'result'" class="result-scope" :title="nodeLabel(selectedNode)"><span>{{ nodeLabel(selectedNode) }}</span><small class="ui-count">{{ recordsTotal.toLocaleString() }}</small></span>
      <form v-if="selectedNode.kind === 'result'" class="ui-search record-search" :class="{ applied: keyword }" @submit.prevent="applyKeyword(keywordInput)">
        <Search :size="14" />
        <input v-model="keywordInput" :aria-label="tr('搜索当前结果')" :placeholder="tr('搜索当前结果，回车确认…')" />
        <button v-if="keywordInput || keyword" class="ui-icon-btn sm" type="button" :aria-label="tr('清除搜索')" :title="tr('清除搜索')" @click="keywordInput = ''; applyKeyword('')"><X :size="13" /></button>
      </form>
      <span v-else class="toolbar-title"><ClipboardList :size="14" />{{ tr("运行信息") }}</span>
      <div v-if="selectedNode.kind === 'result'" class="ui-seg reading-mode" role="group" :aria-label="tr('结果展示方式')">
        <button class="ui-seg-item" type="button" :aria-pressed="presentation === 'properties'" :title="tr('纵向阅读属性和值')" @click="setPresentation('properties')"><List :size="13" aria-hidden="true" />{{ tr("属性视图") }}</button>
        <button class="ui-seg-item" type="button" :aria-pressed="presentation === 'table'" :title="tr('横向对比多条记录')" @click="setPresentation('table')"><Table2 :size="13" aria-hidden="true" />{{ tr("表格视图") }}</button>
      </div>
      <div class="ui-toolbar-actions">
        <template v-if="selectedNode.kind === 'result'">
          <button v-if="runNode && !runNode.legacy" class="ui-btn ghost" @click="selectNode(runNode)"><ClipboardList :size="14" />{{ tr("运行信息") }}</button>
          <button v-if="exporting && exportCanCancel" class="ui-btn ghost" @click="cancelExport">{{ tr("取消导出") }}</button>
          <button class="ui-btn" :disabled="exporting || treeLoading || !!treeError || recordsLoading || recordsTotal === 0" @click="exportCsv"><Loader2 v-if="exporting" :size="14" class="spin" /><Download v-else :size="14" />{{ exporting ? tr("导出中…") : tr("导出 CSV") }}</button>
        </template>
        <button class="ui-icon-btn" :disabled="treeLoading" :aria-label="tr('刷新分析结果')" :title="tr('刷新')" @click="reload"><Loader2 v-if="treeLoading" :size="16" class="spin" /><RefreshCw v-else :size="16" /></button>
      </div>
    </div>
    <div v-if="treeError" class="ui-empty state-box error" role="alert"><span class="ui-empty-icon"><CircleAlert :size="24" /></span><p class="ui-empty-desc">{{ tr("分析结果读取失败：{0}", { 0: treeError }) }}</p><div class="ui-empty-actions"><button class="ui-btn" @click="reload">{{ tr("重试") }}</button></div></div>
    <WorkspaceStartState v-else-if="!selectedNode" :icon="DatabaseZap" :loading="treeLoading"
      :title="treeLoading ? tr('正在读取分析结果…') : tree.length ? tr('选择一个结果项') : tr('当前检材还没有分析结果')"
      :description="tree.length ? tr('在左侧结果目录中选择结果项，查看解析出的记录。') : tr('选择分析功能，完成后结果会显示在这里。')"
      :action-label="tree.length ? undefined : tr('快速分析')" :action-icon="DatabaseZap" role="status" @action="emit('quickAnalysis')">
      <template #toolbar><button class="ui-btn ghost" :disabled="treeLoading" @click="reload"><RefreshCw :size="14" />{{ tr("刷新") }}</button></template>
    </WorkspaceStartState>
    <div v-else-if="selectedNode.kind === 'function'" class="results-body">
      <div v-if="auditLoading" class="ui-empty state-box" role="status"><span class="ui-empty-icon accent"><Loader2 :size="22" class="spin" /></span><p class="ui-empty-desc">{{ tr("正在读取运行信息…") }}</p></div>
      <div v-else-if="auditError" class="ui-empty state-box error" role="alert"><span class="ui-empty-icon"><CircleAlert :size="22" /></span><p class="ui-empty-desc">{{ tr("运行信息读取失败：{0}", { 0: auditError }) }}</p><div class="ui-empty-actions"><button class="ui-btn" @click="retryAudit">{{ tr("重试") }}</button></div></div>
      <div v-else-if="!audit" class="ui-empty state-box" role="status"><span class="ui-empty-icon"><Inbox :size="24" /></span><p class="ui-empty-desc">{{ tr("旧版本的分析结果没有运行信息。") }}</p></div>
      <AnalysisRunAuditView v-else :audit="audit" :node="selectedNode" :locating="locating" @open-file="target => emit('openFile', target)" />
    </div>
    <div v-else class="results-body">
      <div class="result-table">
        <div class="table-scroll" :aria-busy="recordsLoading">
          <div v-if="recordsLoading" class="ui-empty state-box" role="status"><span class="ui-empty-icon accent"><Loader2 :size="22" class="spin" /></span><p class="ui-empty-desc">{{ tr("正在读取记录…") }}</p></div>
          <div v-else-if="recordsError" class="ui-empty state-box error" role="alert"><span class="ui-empty-icon"><CircleAlert :size="22" /></span><p class="ui-empty-desc">{{ tr("记录读取失败：{0}", { 0: recordsError }) }}</p><div class="ui-empty-actions"><button class="ui-btn" @click="retryRecords">{{ tr("重试") }}</button></div></div>
          <div v-else-if="records.length === 0" class="ui-empty state-box" role="status"><span class="ui-empty-icon"><Inbox :size="24" /></span><p class="ui-empty-desc">{{ keyword ? tr("没有匹配的记录") : tr("此结果项暂无记录") }}</p><div v-if="keyword" class="ui-empty-actions"><button class="ui-btn" @click="keywordInput = ''; applyKeyword('')">{{ tr("清除搜索") }}</button></div></div>
          <AnalysisPropertyList v-else-if="presentation === 'properties'" :node="selectedNode" :records="records" :columns="columns" :properties="propertyLayout" :selected-record-id="selectedRecordId" :locating="locating" @open-record="openRecord" @open-file="target => emit('openFile', target)" />
          <table v-else class="data-table records-table" :style="{ minWidth: `${244 + (showTitleColumn ? 240 : 0) + tableColumns.length * 200}px` }" :aria-label="nodeLabel(selectedNode)">
            <colgroup>
              <col class="col-index" />
              <col v-if="showTitleColumn" class="col-name" />
              <col v-for="column in tableColumns" :key="column.id" class="col-field" />
              <col class="col-locate" />
            </colgroup>
            <thead><tr>
              <th class="row-number">{{ tr("序号") }}</th>
              <th v-if="showTitleColumn" :aria-sort="ariaSort('title')"><button class="sort-header" type="button" :title="tr('按名称排序')" @click="toggleSort('title')"><span>{{ tr("名称") }}</span><component :is="sortIcon('title')" :size="12" aria-hidden="true" /></button></th>
              <th v-for="column in tableColumns" :key="column.id" :class="{ numeric: NUMERIC_KINDS.has(column.kind) }" :aria-sort="ariaSort(column.id)"><button class="sort-header" type="button" :title="`${columnLabel(selectedNode, column)} · ${column.id}`" @click="toggleSort(column.id)"><span>{{ columnLabel(selectedNode, column) }}</span><component :is="sortIcon(column.id)" :size="12" aria-hidden="true" /></button></th>
              <th>{{ tr("源文件") }}</th>
            </tr></thead>
            <tbody><tr v-for="(record, index) in records" :key="record.recordId" :class="{ active: selectedRecordId === record.recordId }" :aria-selected="selectedRecordId === record.recordId" tabindex="0" @click="openRecord(record.recordId)" @keydown.enter.prevent="openRecord(record.recordId)">
              <td class="row-number mono">{{ pageIndex * ANALYSIS_PAGE_SIZE + index + 1 }}</td>
              <td v-if="showTitleColumn" :title="record.title"><span class="title-cell"><span v-if="record.severity" class="ui-badge sm" :class="severityTone(record.severity)">{{ severityLabel(record.severity, tr) }}</span><span class="title-text">{{ record.title }}</span></span></td>
              <td v-for="(column, columnIndex) in tableColumns" :key="column.id" :class="{ mono: MONO_KINDS.has(column.kind), numeric: NUMERIC_KINDS.has(column.kind) }" :title="cellText(record, column, Infinity)"><span v-if="columnIndex === 0 && record.severity" class="title-cell"><span class="ui-badge sm" :class="severityTone(record.severity)">{{ severityLabel(record.severity, tr) }}</span><span class="title-text">{{ cellText(record, column) }}</span></span><template v-else>{{ cellText(record, column) }}</template></td>
              <td><span class="table-sources"><button class="ui-btn link sm table-source" :aria-label="tr('定位源文件')" :disabled="locating || !sources(record).length" :title="sources(record)[0] ? `${tr('定位源文件')} · ${sources(record)[0]?.label}` : tr('此记录未提供源文件位置')" @click.stop="locateRecord(record)" @keydown.enter.stop><LocateFixed :size="13" aria-hidden="true" /><span class="mono">{{ sources(record)[0]?.label || tr('无来源') }}</span></button><button v-if="sources(record).length > 1" class="ui-btn ghost sm source-more mono" type="button" :title="tr('查看全部来源（{0}）', { 0: sources(record).length })" :aria-label="tr('查看全部来源（{0}）', { 0: sources(record).length })" @click.stop="openRecord(record.recordId)" @keydown.enter.stop>+{{ (sources(record).length - 1).toLocaleString() }}</button></span></td>
            </tr></tbody>
          </table>
        </div>
        <footer class="ui-footbar table-footer">
          <span class="footer-crumb" :title="crumbLabel"><Table2 :size="12" /><span>{{ crumbLabel }}</span></span>
          <span class="ui-footbar-stat"><strong :key="recordsTotal" class="ui-bump">{{ recordsTotal.toLocaleString() }}</strong> {{ keyword ? tr("条匹配记录") : tr("条记录") }}</span>
          <span v-if="completedLabel" class="ui-footbar-stat footer-completed">{{ tr("完成于（本地）") }} <strong>{{ completedLabel }}</strong></span>
          <span class="ui-footbar-spacer"></span>
          <div class="pager"><button class="ui-icon-btn sm" :disabled="pageIndex === 0 || recordsLoading" :aria-label="tr('上一页')" :title="tr('上一页')" @click="goToPage(pageIndex - 1)"><ChevronLeft :size="16" /></button><span class="mono">{{ pageIndex + 1 }} / {{ pageCount }}</span><button class="ui-icon-btn sm" :disabled="pageIndex >= pageCount - 1 || recordsLoading" :aria-label="tr('下一页')" :title="tr('下一页')" @click="goToPage(pageIndex + 1)"><ChevronRight :size="16" /></button></div>
        </footer>
      </div>
      <UiTransition kind="dock">
        <DetailDock v-if="selectedRecordId" :label="tr('记录详情')" :close-label="tr('关闭详情')" :title="detail?.title || tr('记录详情')" :content-key="selectedRecordId" @close="closeDetail">
          <div v-if="detailLoading" class="ui-empty state-box" role="status"><span class="ui-empty-icon accent"><Loader2 :size="20" class="spin" /></span><p class="ui-empty-desc">{{ tr("加载中…") }}</p></div>
          <div v-else-if="detailError" class="ui-empty state-box error" role="alert"><p class="ui-empty-desc">{{ tr("详情读取失败：{0}", { 0: detailError }) }}</p><div class="ui-empty-actions"><button class="ui-btn" @click="openRecord(selectedRecordId)">{{ tr("重试") }}</button></div></div>
          <FileDetailLayout v-else-if="detail" :title="detail.title">
            <template #icon><Table2 :size="20" /></template>
            <template #badges><span class="ui-badge">{{ nodeLabel(selectedNode) }}</span><span v-if="detail.severity" class="ui-badge" :class="severityTone(detail.severity)">{{ severityLabel(detail.severity, tr) }}</span><span v-if="detail.legacy" class="ui-badge">{{ tr("旧版") }}</span></template>
            <FileDetailSection :title="tr('源文件')" wide>
              <div v-if="sources(detail).length" class="source-files">
                <div v-for="source in sources(detail)" :key="`${source.label}:${source.note}`" class="source-file">
                  <button class="ui-btn link sm" :disabled="locating" :title="source.label" @click="emit('openFile', source.target)"><LocateFixed :size="14" /><span>{{ source.label }}</span></button>
                  <span v-if="source.note" class="source-note">{{ source.note }}</span>
                </div>
              </div>
              <p v-else class="source-missing">{{ tr("此记录未提供源文件位置。") }}</p>
            </FileDetailSection>
            <FileDetailSection :title="tr('记录内容')" wide>
              <FileDetailFields :fields="detailFields" />
            </FileDetailSection>
            <FileDetailSection :key="selectedRecordId" :title="tr('技术信息')" collapsible wide>
              <FileDetailFields :fields="[
                { label: tr('记录 ID'), value: detail.recordId, mono: true, copyable: true },
                { label: tr('记录类型'), value: detail.recordType, mono: true, copyable: true },
                { label: tr('运行 ID'), value: detail.runId, mono: true, copyable: true },
              ]" />
            </FileDetailSection>
          </FileDetailLayout>
        </DetailDock>
      </UiTransition>
    </div>
  </section>
</template>

<script setup lang="ts">
import { tr, useLocale } from '../../../composables/useLocale';

import UiTransition from '../../../components/ui/UiTransition.vue';
import WorkspaceStartState from '../../../components/layout/WorkspaceStartState.vue';
import DetailDock from '../../../components/layout/DetailDock.vue';
import FileDetailLayout from '../../../components/layout/FileDetailLayout.vue';
import FileDetailSection from '../../../components/layout/FileDetailSection.vue';
import FileDetailFields, { type FileDetailField } from '../../../components/layout/FileDetailFields.vue';
import AnalysisRunAuditView from './AnalysisRunAuditView.vue';
import AnalysisPropertyList from './AnalysisPropertyList.vue';
import { computed, nextTick, onActivated, onBeforeUnmount, onDeactivated, onMounted, ref, watch } from 'vue';
import { ArrowDown, ArrowUp, ArrowUpDown, ChevronLeft, ChevronRight, CircleAlert, ClipboardList, DatabaseZap, Download, Inbox, List, Loader2, LocateFixed, RefreshCw, Search, Table2, X } from 'lucide-vue-next';
import { ANALYSIS_PAGE_SIZE, useAnalysisResults } from '../composables/useAnalysisResults';
import { useAnalysisLabels } from '../composables/useAnalysisLabels';
import { useAnalysisPresentation } from '../composables/useAnalysisPresentation';
import { saveTextWithDialog } from '../../../composables/export';
import { notifyError, notifyInfo, pushPersistentToast } from '../../../composables/useToast';
import { recordsToCsv, sanitizeExportFilename } from '../../../utils/exportFormat';
import type { AnalysisRecord, AnalysisResultNode, RecordColumn, RecordFieldKind } from '../../../lib/ipc/analysisResults';
import { analysisSources } from '../model/analysisSource';
import { recordCsvValue, recordValueText, severityLabel, severityTone, truncateText } from '../model/analysisValue';
import type { FileLocationTarget } from '../../../lib/models/fileLocation';
const props = defineProps<{ imageId: string; locating?: boolean }>();
const emit = defineEmits<{ quickAnalysis: []; openFile: [target: FileLocationTarget] }>();
const sources = (record: AnalysisRecord) => analysisSources(record, selectedNode.value?.imageId || props.imageId);
function locateRecord(record: AnalysisRecord) {
  const source = sources(record)[0];
  if (source && !props.locating) emit('openFile', source.target);
}
const { tree, treeLoading, treeError, dirty, selectedNode, records, columns, properties, recordsTotal,
  pageIndex, pageCount, keyword, sort, recordsLoading, recordsError, detail, detailColumns, detailLoading,
  detailError, selectedRecordId, audit, auditLoading, auditError, closeDetail: clearDetail, retryRecords, retryAudit,
  setImageId, ensureFresh, selectNode, goToPage, applyKeyword, toggleSort, openRecord: loadRecord, exportRecords, reload } = useAnalysisResults();
const { nodeLabel, columnLabel, choiceLabel } = useAnalysisLabels();
const { propertyLayout, presentation, setPresentation } = useAnalysisPresentation(selectedNode, columns, properties);
let detailReturnFocus: HTMLElement | null = null;
function openRecord(recordId: string) {
  const focused = document.activeElement;
  if (focused instanceof HTMLElement && !focused.closest('.detail-dock')) detailReturnFocus = focused;
  return loadRecord(recordId);
}
function closeDetail() {
  const target = detailReturnFocus;
  detailReturnFocus = null;
  clearDetail();
  void nextTick(() => { if (target?.isConnected) target.focus(); });
}
const panelActive = ref(false);
function activatePanel() { panelActive.value = true; setImageId(props.imageId); ensureFresh(); }
onMounted(activatePanel);
onActivated(activatePanel);
onDeactivated(() => { panelActive.value = false; detailReturnFocus = null; });
watch(() => props.imageId, () => { if (panelActive.value) activatePanel(); });
watch(dirty, value => { if (value && panelActive.value && !treeLoading.value) reload(); });
const keywordInput = ref(keyword.value);
const exporting = ref(false);
const exportCanCancel = ref(false);
let exportController: AbortController | null = null;
function cancelExport() {
  exportCanCancel.value = false;
  exportController?.abort();
}
onBeforeUnmount(() => exportController?.abort());

watch(keyword, value => { keywordInput.value = value; });

/** Values set in the monospace face: offsets, hashes, addresses, paths and counts. */
const MONO_KINDS = new Set<RecordFieldKind>(['int', 'float', 'size', 'duration', 'hash', 'ip', 'path', 'timestamp']);
const NUMERIC_KINDS = new Set<RecordFieldKind>(['int', 'float', 'size', 'duration']);
const tableColumns = computed(() => columns.value.filter(column => !column.hidden));
// Schema fields already describe the record; keep a title fallback for results without visible fields.
const showTitleColumn = computed(() => tableColumns.value.length === 0);

/** The run the selected record type belongs to (its audit). */
const runNode = computed(() => {
  const node = selectedNode.value;
  if (!node) return null;
  const find = (nodes: AnalysisResultNode[]): AnalysisResultNode | null => {
    for (const item of nodes) {
      if (item.kind === 'function' && item.children.some(child => child.nodeId === node.nodeId)) return item;
      const found = find(item.children);
      if (found) return found;
    }
    return null;
  };
  return find(tree.value);
});

const statusText = computed(() => {
  const node = selectedNode.value;
  if (treeError.value || recordsError.value || !node) return '';
  if (node.kind === 'function') return auditLoading.value ? tr("正在读取运行信息…") : nodeLabel(node);
  if (recordsLoading.value) return tr("正在读取记录…");
  return tr("{0}，共 {1} 条{2}，第 {3} 页", { 0: nodeLabel(node), 1: recordsTotal.value.toLocaleString(), 2: keyword.value ? tr("匹配记录") : tr("记录"), 3: pageIndex.value + 1 });
});

function ariaSort(column: string): 'ascending' | 'descending' | 'none' {
  if (sort.value?.column !== column) return 'none';
  return sort.value.descending ? 'descending' : 'ascending';
}

function sortIcon(column: string) {
  if (sort.value?.column !== column) return ArrowUpDown;
  return sort.value.descending ? ArrowDown : ArrowUp;
}

function valueText(value: unknown, column: RecordColumn, detailed = false): string {
  return recordValueText(value, column.kind, { choice: id => choiceLabel(selectedNode.value, column, id), detailed, tr });
}

function cellText(record: AnalysisRecord, column: RecordColumn, maxLength = 120): string {
  return truncateText(valueText(record.values[column.id], column), maxLength);
}

const selectedCrumb = computed<string[]>(() => {
  const crumb: string[] = [];
  const walk = (nodes: AnalysisResultNode[]): boolean => {
    for (const node of nodes) {
      crumb.push(node.kind === 'function' && node.targetLabel ? `${nodeLabel(node)} (${node.targetLabel})` : nodeLabel(node));
      if (node.nodeId === selectedNode.value?.nodeId) return true;
      if (walk(node.children)) return true;
      crumb.pop();
    }
    return false;
  };
  if (!selectedNode.value) return [];
  walk(tree.value);
  return crumb;
});

/** 状态栏的结果路径：检材 › 分类 › 分析器 › 记录类型。 */
const crumbLabel = computed(() => selectedCrumb.value.join(' › ') || (selectedNode.value ? nodeLabel(selectedNode.value) : ''));
const { locale } = useLocale();
const completedLabel = computed(() => {
  const completedAt = runNode.value?.completedAt;
  if (!completedAt) return '';
  const completed = new Date(completedAt);
  return Number.isNaN(completed.getTime()) ? '' : completed.toLocaleString(locale.value, { hour12: false });
});

/** 详情字段:类型声明的列(含只在详情显示的列)在前,未声明的值补在后。 */
const detailFields = computed<FileDetailField[]>(() => {
  const record = detail.value;
  if (!record) return [];
  const declared = detailColumns.value;
  const declaredIds = new Set(declared.map(column => column.id));
  const fields: FileDetailField[] = declared.map(column => ({
    key: column.id,
    label: columnLabel(selectedNode.value, column),
    value: valueText(record.values[column.id], column, true),
    mono: MONO_KINDS.has(column.kind),
    copyable: true,
  }));
  for (const key of Object.keys(record.values).filter(key => !declaredIds.has(key)).sort()) {
    fields.push({ key, label: key, value: recordValueText(record.values[key], null, { tr }), copyable: true });
  }
  return fields;
});

async function exportCsv(): Promise<void> {
  const node = selectedNode.value;
  if (!node || node.kind !== 'result' || exporting.value) return;
  const label = nodeLabel(node);
  const controller = new AbortController();
  exportController = controller;
  exporting.value = true;
  exportCanCancel.value = true;
  const progress = pushPersistentToast({
    title: tr("正在导出分析结果"),
    detail: tr("{0} · 正在读取记录…", { 0: label }),
    progress: 0,
    action: {
      get label() { return tr("取消"); },
      onClick: cancelExport,
    },
  });
  controller.signal.addEventListener('abort', () => {
    progress.update({ detail: tr("{0} · 正在取消…", { 0: label }), action: { get label() { return tr("取消中…"); }, onClick: () => {}, disabled: true } });
  }, { once: true });
  try {
    const count = await exportRecords(
      (items, exportColumns) => {
        const nameColumn = tr("名称");
        // Headers key the CSV rows: a label that repeats gets its column id.
        const used = new Set([nameColumn]);
        const fields = exportColumns.map(column => {
          let label = columnLabel(node, column);
          if (used.has(label)) label = `${label} (${column.id})`;
          used.add(label);
          return { id: column.id, label };
        });
        const headers = [nameColumn, ...fields.map(field => field.label)];
        return recordsToCsv(
          items.map(item => ({
            [nameColumn]: item.title,
            ...Object.fromEntries(fields.map(field => [field.label, recordCsvValue(item.values[field.id])])),
          })),
          headers,
        );
      },
      csv => {
        exportCanCancel.value = false;
        progress.update({ detail: tr("{0} · 请选择保存位置", { 0: label }), progress: 100, action: undefined });
        return saveTextWithDialog({
          title: tr("导出分析结果"),
          defaultPath: tr("分析结果-{0}.csv", { 0: sanitizeExportFilename(label) }),
          filters: [{ name: 'CSV', extensions: ['csv'] }],
          content: csv,
        });
      },
      {
        signal: controller.signal,
        onProgress: ({ loaded, total }) => progress.update({
          detail: tr("{0} · 已读取 {1} / {2} 条记录", { 0: label, 1: loaded.toLocaleString(), 2: total.toLocaleString() }),
          progress: total ? loaded / total * 100 : 100,
        }),
      },
    );
    if (count === 0) notifyInfo(tr("没有可导出的记录"), tr("请刷新分析结果后重试。"));
  } catch (error) {
    if (controller.signal.aborted) notifyInfo(tr("已取消导出"), label);
    else notifyError(tr("导出失败"), String(error));
  } finally {
    progress.dismiss();
    exportController = null;
    exportCanCancel.value = false;
    exporting.value = false;
  }
}
</script>

<style scoped src="../../../styles/data-table.css"></style>

<style scoped>
.analysis-results { height: 100%; min-height: 0; display: flex; flex-direction: column; background: var(--surface-secondary); color: var(--text-primary); container: analysis-results / inline-size; }
.result-scope { display: flex; align-items: center; gap: 8px; min-width: 0; max-width: 200px; font-size: 13px; font-weight: 600; }
.result-scope > span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.result-scope > small { flex-shrink: 0; }
.reading-mode { flex-shrink: 0; }
.reading-mode button { gap: 6px; }
.record-search { flex: 0 1 320px; min-width: 180px; padding-right: 4px; }
.record-search.applied { border-color: color-mix(in srgb, var(--primary-color) 55%, var(--border-color)); background: color-mix(in srgb, var(--primary-color) 5%, var(--surface-secondary)); }
.record-search.applied > svg { color: var(--primary-color); opacity: 1; }
.toolbar-title { display: inline-flex; align-items: center; gap: 6px; color: var(--text-secondary); font-size: 12px; font-weight: 500; }

.records-table { table-layout: fixed; }
.col-index { width: 64px; }
.col-name { width: 240px; }
.col-field { width: 200px; }
.col-locate { width: 180px; }
.records-table th:last-child, .records-table td:last-child { position: sticky; right: 0; background: var(--surface-secondary); }
.records-table th:last-child { z-index: 3; background: color-mix(in srgb, var(--surface-tertiary) 60%, var(--surface-secondary)); }
.records-table .numeric { text-align: right; font-variant-numeric: tabular-nums; }
.records-table .numeric .sort-header, .records-table .numeric .title-cell { justify-content: flex-end; }
/* 吸附的末列画在行描边之上：选中行由它补齐上、右、下三段描边（下边借底边框，与行描边同一像素行），
   选中浅底下再垫一层不透明底色，横向滚动时下面的列不会透出来。 */
.records-table tr.active td:last-child {
  border-bottom-color: var(--row-selected-ring);
  background: linear-gradient(var(--sidebar-active-bg), var(--sidebar-active-bg)), var(--surface-secondary);
  box-shadow: inset 0 1px 0 var(--row-selected-ring), inset -1px 0 0 var(--row-selected-ring);
}
.sort-header { display: flex; align-items: center; gap: 4px; width: 100%; min-width: 0; padding: 0; border: 0; background: transparent; color: inherit; font: inherit; text-align: left; cursor: pointer; }
.sort-header > span { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sort-header > svg { flex-shrink: 0; color: var(--text-secondary); opacity: 0.6; }
th[aria-sort='ascending'] .sort-header > svg, th[aria-sort='descending'] .sort-header > svg { color: var(--primary-color); opacity: 1; }
.sort-header:hover > svg { opacity: 1; }
.sort-header:focus-visible { outline: 2px solid var(--primary-color); outline-offset: 2px; }
.title-cell { display: flex; align-items: center; gap: 6px; min-width: 0; }
.title-cell .ui-badge { flex-shrink: 0; }
.title-text { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.records-table td.mono { font-family: var(--font-mono); font-size: 12px; }
.source-files { display: flex; flex-direction: column; align-items: flex-start; gap: 8px; }
.source-file { display: flex; flex-wrap: wrap; align-items: baseline; gap: 4px 8px; max-width: 100%; }
.source-file .ui-btn { max-width: 100%; height: auto; text-align: left; }
.source-file .ui-btn span { overflow-wrap: anywhere; white-space: normal; }
.source-note { color: var(--text-secondary); font-size: 11px; }
.source-missing { margin: 0; color: var(--text-secondary); font-size: 12px; }
.records-table th { overflow: hidden; text-overflow: ellipsis; }
.records-table tr { cursor: pointer; }
.records-table .row-number { text-align: right; font-variant-numeric: tabular-nums; }
.records-table td.row-number { color: var(--text-secondary); font-size: 12px; }
.records-table .ui-btn.link { --ui-btn-h: 24px; padding: 0; }
.table-source { max-width: 100%; min-width: 0; }
.table-sources { display: flex; align-items: center; min-width: 0; }
.table-source { flex-shrink: 1; }
.table-source > span { overflow: hidden; text-overflow: ellipsis; }
.source-more { --ui-btn-h: 24px; padding: 0 4px; font-size: 11px; }

.results-body { container: detail-host / inline-size; position: relative; display: flex; flex: 1; min-height: 0; }
.result-table { flex: 1; min-width: 0; display: flex; flex-direction: column; container: analysis-records / inline-size; }
.table-scroll { flex: 1; min-height: 0; overflow: auto; }
.state-box { flex: 1; min-height: 180px; overflow-wrap: anywhere; }
.state-box.error { color: var(--sev-critical-fg); }
.state-box.error .ui-empty-icon { background: var(--sev-critical-bg); color: var(--sev-critical-fg); }

.table-footer { padding-right: 10px; }
.footer-crumb { display: inline-flex; align-items: center; gap: 4px; min-width: 0; max-width: 55%; overflow: hidden; color: var(--text-primary); font-weight: 500; white-space: nowrap; }
.footer-crumb > svg { flex-shrink: 0; color: var(--primary-color); }
.footer-crumb > span { overflow: hidden; text-overflow: ellipsis; }
.pager { display: flex; align-items: center; gap: 6px; margin-left: auto; white-space: nowrap; }
.pager .mono { min-width: 48px; color: var(--text-primary); text-align: center; }

@container analysis-results (max-width: 760px) {
  .footer-completed { display: none; }
  .result-scope { max-width: 140px; }
  .record-search { flex: 1; }
  .results-toolbar .ui-toolbar-actions { width: 100%; }
}
</style>
