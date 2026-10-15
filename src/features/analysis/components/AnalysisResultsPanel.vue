<template>
  <section class="analysis-results" :aria-label="tr('分析结果')">
    <p class="sr-only" role="status">{{ treeError || recordsError ? '' : recordsLoading ? tr("正在读取记录…") : selectedNode ? tr("{0}，共 {1} 条{2}，第 {3} 页", { 0: selectedNode.label, 1: recordsTotal.toLocaleString(), 2: keyword ? tr("匹配记录") : tr("记录"), 3: pageIndex + 1 }) : '' }}</p>
    <!-- 页面不设标题栏：选中结果项后顶部只有检索与导出，结果路径与计数放在底部状态栏。 -->
    <div v-if="selectedNode" class="ui-toolbar results-toolbar">
      <form class="ui-search record-search" :class="{ applied: keyword }" @submit.prevent="applyKeyword(keywordInput)">
        <Search :size="14" />
        <input v-model="keywordInput" :aria-label="tr('搜索当前结果')" :placeholder="tr('搜索当前结果，回车确认…')" />
        <button v-if="keywordInput || keyword" class="ui-icon-btn sm" type="button" :aria-label="tr('清除搜索')" :title="tr('清除搜索')" @click="keywordInput = ''; applyKeyword('')"><X :size="13" /></button>
      </form>
      <div class="ui-toolbar-actions">
        <button v-if="exporting && exportCanCancel" class="ui-btn ghost" @click="cancelExport">{{ tr("取消导出") }}</button>
        <button class="ui-btn" :disabled="exporting || treeLoading || !!treeError || recordsLoading || recordsTotal === 0" @click="exportCsv"><Loader2 v-if="exporting" :size="14" class="spin" /><Download v-else :size="14" />{{ exporting ? tr("导出中…") : tr("导出 CSV") }}</button>
        <button class="ui-icon-btn" :disabled="treeLoading" :aria-label="tr('刷新分析结果')" :title="tr('刷新')" @click="reload"><Loader2 v-if="treeLoading" :size="16" class="spin" /><RefreshCw v-else :size="16" /></button>
      </div>
    </div>
    <div v-if="treeError" class="ui-empty state-box error" role="alert"><span class="ui-empty-icon"><CircleAlert :size="24" /></span><p class="ui-empty-desc">{{ tr("分析结果读取失败：") }}{{ treeError }}</p><div class="ui-empty-actions"><button class="ui-btn" @click="reload">{{ tr("重试") }}</button></div></div>
    <WorkspaceStartState v-else-if="!selectedNode" :icon="DatabaseZap" :loading="treeLoading"
      :title="treeLoading ? tr('正在读取分析结果…') : tree.length ? tr('选择一个结果项') : tr('当前检材还没有分析结果')"
      :description="tree.length ? tr('在左侧结果目录中选择结果项，查看解析出的记录。') : tr('选择分析功能，完成后结果会显示在这里。')"
      :action-label="tree.length ? undefined : tr('快速分析')" :action-icon="DatabaseZap" role="status" @action="emit('quickAnalysis')">
      <template #toolbar><button class="ui-btn ghost" :disabled="treeLoading" @click="reload"><RefreshCw :size="14" />{{ tr("刷新") }}</button></template>
    </WorkspaceStartState>
    <div v-else class="results-body">
      <div class="result-table">
        <div class="table-scroll" :aria-busy="recordsLoading">
          <div v-if="recordsLoading" class="ui-empty state-box" role="status"><span class="ui-empty-icon accent"><Loader2 :size="22" class="spin" /></span><p class="ui-empty-desc">{{ tr("正在读取记录…") }}</p></div>
          <div v-else-if="recordsError" class="ui-empty state-box error" role="alert"><span class="ui-empty-icon"><CircleAlert :size="22" /></span><p class="ui-empty-desc">{{ tr("记录读取失败：") }}{{ recordsError }}</p><div class="ui-empty-actions"><button class="ui-btn" @click="retryRecords">{{ tr("重试") }}</button></div></div>
          <div v-else-if="records.length === 0" class="ui-empty state-box" role="status"><span class="ui-empty-icon"><Inbox :size="24" /></span><p class="ui-empty-desc">{{ keyword ? tr("没有匹配的记录") : tr("此结果项暂无记录") }}</p><div v-if="keyword" class="ui-empty-actions"><button class="ui-btn" @click="keywordInput = ''; applyKeyword('')">{{ tr("清除搜索") }}</button></div></div>
          <table v-else class="data-table records-table" :style="{ minWidth: `${424 + selectedNode.columns.length * 200}px` }" :aria-label="selectedNode.label">
            <colgroup>
              <col class="col-index" />
              <col class="col-name" />
              <col v-for="column in selectedNode.columns" :key="column.key" class="col-field" />
              <col class="col-locate" />
            </colgroup>
            <thead><tr><th class="row-number">{{ tr("序号") }}</th><th>{{ tr("名称") }}</th><th v-for="column in selectedNode.columns" :key="column.key" :title="`${column.label} · ${column.key}`">{{ column.label }}</th><th>{{ tr("定位") }}</th></tr></thead>
            <tbody><tr v-for="(record, index) in records" :key="record.artifactId" :class="{ active: selectedRecordId === record.artifactId }" :aria-selected="selectedRecordId === record.artifactId" tabindex="0" @click="openRecord(record.artifactId)" @keydown.enter.prevent="openRecord(record.artifactId)">
              <td class="row-number mono">{{ pageIndex * ANALYSIS_PAGE_SIZE + index + 1 }}</td><td :title="record.title">{{ record.title }}</td><td v-for="column in selectedNode.columns" :key="column.key" :title="cellText(record.attributes[column.key], Infinity)">{{ cellText(record.attributes[column.key]) }}</td>
              <td><button class="ui-btn link sm" :disabled="locating || !sources(record).length" :title="sources(record)[0]?.label || tr('此记录未提供源文件位置')" @click.stop="locateRecord(record)" @keydown.enter.stop><LocateFixed :size="13" />{{ tr("定位源文件") }}</button></td>
            </tr></tbody>
          </table>
        </div>
        <footer class="ui-footbar table-footer">
          <span class="footer-crumb" :title="crumbLabel"><Table2 :size="12" /><span>{{ crumbLabel }}</span></span>
          <span class="ui-footbar-stat"><strong :key="recordsTotal" class="ui-bump">{{ recordsTotal.toLocaleString() }}</strong> {{ tr("条") }}{{ keyword ? tr("匹配记录") : tr("记录") }}</span>
          <span v-if="completedLabel" class="ui-footbar-stat footer-completed">{{ tr("完成于") }} <strong>{{ completedLabel }}</strong></span>
          <span class="ui-footbar-spacer"></span>
          <div class="pager"><button class="ui-icon-btn sm" :disabled="pageIndex === 0 || recordsLoading" :aria-label="tr('上一页')" :title="tr('上一页')" @click="goToPage(pageIndex - 1)"><ChevronLeft :size="16" /></button><span class="mono">{{ pageIndex + 1 }} / {{ pageCount }}</span><button class="ui-icon-btn sm" :disabled="pageIndex >= pageCount - 1 || recordsLoading" :aria-label="tr('下一页')" :title="tr('下一页')" @click="goToPage(pageIndex + 1)"><ChevronRight :size="16" /></button></div>
        </footer>
      </div>
      <UiTransition kind="dock">
        <DetailDock v-if="selectedRecordId" :label="tr('记录详情')" :close-label="tr('关闭详情')" :title="detail?.title || tr('记录详情')" :content-key="selectedRecordId" @close="closeDetail">
          <div v-if="detailLoading" class="ui-empty state-box" role="status"><span class="ui-empty-icon accent"><Loader2 :size="20" class="spin" /></span><p class="ui-empty-desc">{{ tr("加载中…") }}</p></div>
          <div v-else-if="detailError" class="ui-empty state-box error" role="alert"><p class="ui-empty-desc">{{ tr("详情读取失败：") }}{{ detailError }}</p><div class="ui-empty-actions"><button class="ui-btn" @click="openRecord(selectedRecordId)">{{ tr("重试") }}</button></div></div>
          <FileDetailLayout v-else-if="detail" :title="detail.title">
            <template #icon><Table2 :size="20" /></template>
            <template #badges><span class="ui-badge">{{ tr("分析记录") }}</span><span class="ui-badge">{{ selectedNode.label }}</span></template>
            <FileDetailSection :title="tr('源文件')" wide>
              <div v-if="sources(detail).length" class="source-files">
                <button v-for="source in sources(detail)" :key="source.label" class="ui-btn link sm source-file" :disabled="locating" :title="source.label" @click="emit('openFile', source.target)"><LocateFixed :size="14" /><span>{{ source.label }}</span></button>
              </div>
              <p v-else class="source-missing">{{ tr("此记录未提供源文件位置。") }}</p>
            </FileDetailSection>
            <FileDetailSection :title="tr('记录内容')" wide>
              <FileDetailFields :fields="[
                ...(detail.value !== null && detail.value !== undefined ? [{ label: tr('记录值'), value: cellText(detail.value, Infinity), copyable: true }] : []),
                ...detailFields.map(field => ({ key: field.key, label: field.label, value: cellText(detail!.attributes[field.key], Infinity), copyable: true })),
              ]" />
            </FileDetailSection>
            <FileDetailSection :key="selectedRecordId" :title="tr('技术信息')" collapsible wide>
              <FileDetailFields :fields="[
                { label: tr('记录 ID'), value: detail.artifactId, mono: true, copyable: true },
                { label: tr('结果类型'), value: detail.artifactType, mono: true, copyable: true },
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
import FileDetailFields from '../../../components/layout/FileDetailFields.vue';
import { computed, onActivated, onBeforeUnmount, onDeactivated, onMounted, ref, watch } from 'vue';
import { ChevronLeft, ChevronRight, CircleAlert, DatabaseZap, Download, Inbox, Loader2, LocateFixed, RefreshCw, Search, Table2, X } from 'lucide-vue-next';
import { ANALYSIS_PAGE_SIZE, useAnalysisResults } from '../composables/useAnalysisResults';
import { saveTextWithDialog } from '../../../composables/export';
import { notifyError, notifyInfo, pushPersistentToast } from '../../../composables/useToast';
import { recordsToCsv, sanitizeExportFilename } from '../../../utils/exportFormat';
import type { AnalysisRecord, AnalysisResultNode } from '../../../lib/ipc/analysisResults';
import { analysisSources } from '../model/analysisSource';
import type { FileLocationTarget } from '../../../lib/models/fileLocation';
const props = defineProps<{ imageId: string; locating?: boolean }>();
const emit = defineEmits<{ quickAnalysis: []; openFile: [target: FileLocationTarget] }>();
const sources = (record: AnalysisRecord) => analysisSources(record, selectedNode.value?.imageId || props.imageId);
function locateRecord(record: AnalysisRecord) {
  const source = sources(record)[0];
  if (source && !props.locating) emit('openFile', source.target);
}
const { tree, treeLoading, treeError, dirty, selectedNode, records, recordsTotal,
  pageIndex, pageCount, keyword, recordsLoading, recordsError, detail, detailLoading,
  detailError, selectedRecordId, closeDetail, retryRecords, setImageId, ensureFresh,
  goToPage, applyKeyword, openRecord, exportRecords, reload } = useAnalysisResults();
const panelActive = ref(false);
function activatePanel() { panelActive.value = true; setImageId(props.imageId); ensureFresh(); }
onMounted(activatePanel);
onActivated(activatePanel);
onDeactivated(() => { panelActive.value = false; });
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

const selectedCrumb = computed<string[]>(() => {
  const crumb: string[] = [];
  const walk = (nodes: AnalysisResultNode[]): boolean => {
    for (const node of nodes) {
      crumb.push(node.label);
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

/** 状态栏的结果路径：检材 › 分类 › 功能 › 结果项。 */
const crumbLabel = computed(() => selectedCrumb.value.join(' › ') || selectedNode.value?.label || '');
const { locale } = useLocale();
const completedLabel = computed(() => {
  const completedAt = selectedNode.value?.completedAt;
  if (!completedAt) return '';
  const completed = new Date(completedAt);
  return Number.isNaN(completed.getTime()) ? '' : completed.toLocaleString(locale.value, { hour12: false });
});

function cellText(value: unknown, maxLength = 120): string {
  if (value === null || value === undefined) return '—';
  if (typeof value === 'string') return value.length > maxLength ? `${value.slice(0, maxLength - 1)}…` : value;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  try {
    const text = JSON.stringify(value);
    return text.length > maxLength ? `${text.slice(0, maxLength - 1)}…` : text;
  } catch {
    return String(value);
  }
}

/** 详情字段:声明列在前,其余属性键补在后。 */
const detailFields = computed(() => {
  if (!detail.value || !selectedNode.value) return [];
  const declared = selectedNode.value.columns.map(column => ({ key: column.key, label: column.label }));
  const declaredKeys = new Set(declared.map(column => column.key));
  const extras = Object.keys(detail.value.attributes)
    .filter(key => !declaredKeys.has(key))
    .sort()
    .map(key => ({ key, label: key }));
  return [...declared, ...extras];
});

async function exportCsv(): Promise<void> {
  const node = selectedNode.value;
  if (!node || exporting.value) return;
  const columns = [tr("名称"), ...node.columns.map(column => column.label)];
  const controller = new AbortController();
  exportController = controller;
  exporting.value = true;
  exportCanCancel.value = true;
  const progress = pushPersistentToast({
    title: tr("正在导出分析结果"),
    detail: tr("{0} · 正在读取记录…", { 0: node.label }),
    progress: 0,
    action: {
      get label() { return tr("取消"); },
      onClick: cancelExport,
    },
  });
  controller.signal.addEventListener('abort', () => {
    progress.update({ detail: tr("{0} · 正在取消…", { 0: node.label }), action: { get label() { return tr("取消中…"); }, onClick: () => {}, disabled: true } });
  }, { once: true });
  try {
    const count = await exportRecords(
      items =>
        recordsToCsv(
          items.map(item => ({
            名称: item.title,
            ...Object.fromEntries(
              node.columns.map(column => [column.label, item.attributes[column.key] ?? '']),
            ),
          })),
          columns,
        ),
      csv => {
        exportCanCancel.value = false;
        progress.update({ detail: tr("{0} · 请选择保存位置", { 0: node.label }), progress: 100, action: undefined });
        return saveTextWithDialog({
          title: tr("导出分析结果"),
          defaultPath: tr("分析结果-{0}.csv", { 0: sanitizeExportFilename(node.label) }),
          filters: [{ name: 'CSV', extensions: ['csv'] }],
          content: csv,
        });
      },
      {
        signal: controller.signal,
        onProgress: ({ loaded, total }) => progress.update({
          detail: tr("{0} · 已读取 {1} / {2} 条记录", { 0: node.label, 1: loaded.toLocaleString(), 2: total.toLocaleString() }),
          progress: total ? loaded / total * 100 : 100,
        }),
      },
    );
    if (count === 0) notifyInfo(tr("没有可导出的记录"), tr("请刷新分析结果后重试。"));
  } catch (error) {
    if (controller.signal.aborted) notifyInfo(tr("已取消导出"), node.label);
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
.record-search { flex: 0 1 320px; min-width: 180px; padding-right: 4px; }
.record-search.applied { border-color: color-mix(in srgb, var(--primary-color) 55%, var(--border-color)); background: color-mix(in srgb, var(--primary-color) 5%, var(--surface-secondary)); }
.record-search.applied > svg { color: var(--primary-color); opacity: 1; }

.records-table { table-layout: fixed; }
.col-index { width: 64px; }
.col-name { width: 240px; }
.col-field { width: 200px; }
.col-locate { width: 120px; }
.records-table th:last-child, .records-table td:last-child { position: sticky; right: 0; background: var(--surface-secondary); }
/* 吸附的末列画在行描边之上：选中行由它补齐上、右、下三段描边（下边借底边框，与行描边同一像素行），
   选中浅底下再垫一层不透明底色，横向滚动时下面的列不会透出来。 */
.records-table tr.active td:last-child {
  border-bottom-color: var(--row-selected-ring);
  background: linear-gradient(var(--sidebar-active-bg), var(--sidebar-active-bg)), var(--surface-secondary);
  box-shadow: inset 0 1px 0 var(--row-selected-ring), inset -1px 0 0 var(--row-selected-ring);
}
.source-files { display: flex; flex-direction: column; align-items: flex-start; gap: 8px; }
.source-file { max-width: 100%; height: auto; text-align: left; }
.source-file span { overflow-wrap: anywhere; white-space: normal; }
.source-missing { margin: 0; color: var(--text-secondary); font-size: 12px; }
.records-table th { overflow: hidden; text-overflow: ellipsis; }
.records-table tr { cursor: pointer; }
.records-table .row-number { text-align: right; font-variant-numeric: tabular-nums; }
.records-table td.row-number { color: var(--text-secondary); font-size: 12px; }
.records-table .ui-btn.link { --ui-btn-h: 24px; padding: 0; }

.results-body { container: detail-host / inline-size; position: relative; display: flex; flex: 1; min-height: 0; }
.result-table { flex: 1; min-width: 0; display: flex; flex-direction: column; }
.table-scroll { flex: 1; min-height: 0; overflow: auto; }
.state-box { flex: 1; min-height: 180px; overflow-wrap: anywhere; }
.state-box.error { color: var(--sev-critical-fg); }
.state-box.error .ui-empty-icon { background: var(--sev-critical-bg); color: var(--sev-critical-fg); }

.table-footer { padding-right: 10px; }
.footer-crumb { display: inline-flex; align-items: center; gap: 4px; min-width: 0; max-width: 55%; overflow: hidden; color: var(--text-primary); font-weight: 500; white-space: nowrap; }
.footer-crumb > svg { flex-shrink: 0; color: var(--primary-color); }
.footer-crumb > span { overflow: hidden; text-overflow: ellipsis; }
.pager { display: flex; align-items: center; gap: 6px; white-space: nowrap; }
.pager .mono { min-width: 48px; color: var(--text-primary); text-align: center; }

@container analysis-results (max-width: 760px) {
  .footer-completed { display: none; }
}
</style>
