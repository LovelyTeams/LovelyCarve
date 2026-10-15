<template>
  <div class="bcp-workspace">
    <div class="ui-toolbar bcp-toolbar">
      <div class="bcp-toolbar-context">
        <Database :size="16" />
        <span>{{ categoryLabel }}</span>
        <span v-if="catalog" class="ui-badge info">{{ catalog.totalRows.toLocaleString() }} {{ tr("条") }}</span>
      </div>
      <div class="ui-toolbar-actions">
        <input v-model="filter" class="ui-search bcp-search" type="search" :placeholder="tr('筛选当前页…')" :aria-label="tr('筛选当前页')" />
      </div>
    </div>

    <div v-if="!catalog && catalogError" class="ui-alert danger">{{ catalogError }}</div>
    <div v-else-if="!catalog" class="ui-empty bcp-empty">
      <span class="ui-empty-icon"><Database :size="24" /></span>
      <p class="ui-empty-title">{{ tr("正在读取 BCP 索引…") }}</p>
    </div>
    <div v-else class="bcp-body">
      <aside class="bcp-datasets" :aria-label="tr('BCP 数据集')">
        <div class="bcp-datasets-heading">{{ tr("数据集") }} <span>{{ categoryDatasets.length }}</span></div>
        <button v-for="dataset in categoryDatasets" :key="dataset.code" type="button"
          class="bcp-dataset" :class="{ active: dataset.code === selectedCode }"
          @click="selectDataset(dataset.code)">
          <span class="bcp-dataset-label">{{ bcpDatasetLabel(dataset, tr) }}</span>
          <span class="bcp-dataset-code">{{ bcpSuffix(dataset.code) }}</span>
          <span class="bcp-dataset-count">{{ dataset.rows.toLocaleString() }}</span>
        </button>
      </aside>

      <main class="bcp-main">
        <div v-if="selectedDataset" class="bcp-dataset-head">
          <div class="bcp-dataset-summary">
            <strong>{{ bcpDatasetLabel(selectedDataset, tr) }}</strong>
            <span class="bcp-code">{{ selectedDataset.code }}</span>
            <span>{{ selectedDataset.files.length }} {{ tr("个文件 ·") }} {{ selectedDataset.rows.toLocaleString() }} {{ tr("条记录 ·") }} {{ selectedDataset.fields.length }} {{ tr("列") }}</span>
          </div>
          <div class="bcp-page-controls">
            <button class="ui-btn sm" :disabled="history.length === 0 || loading" @click="previousPage"><ChevronLeft :size="14" />{{ tr("上一页") }}</button>
            <span>{{ tr("第") }} {{ history.length + 1 }} {{ tr("页") }}</span>
            <button class="ui-btn sm" :disabled="!page?.nextCursor || loading" @click="nextPage">{{ tr("下一页") }}<ChevronRight :size="14" /></button>
          </div>
        </div>
        <div v-if="error" class="ui-alert danger">{{ error }}</div>
        <div v-else-if="loading" class="bcp-loading"><Loader2 :size="17" class="spin" /> {{ tr("正在读取记录…") }}</div>
        <div v-else-if="page && selectedDataset" class="bcp-table-wrap">
          <table class="bcp-table">
            <thead><tr><th class="bcp-row-number">#</th><th v-for="column in visibleColumns" :key="column.index">{{ column.field.label || column.field.name }}</th><th>{{ tr("来源文件") }}</th></tr></thead>
            <tbody>
              <tr v-for="row in filteredRows" :key="`${row.fileName}:${row.number}`" :class="{ selected: selectedRow === row }" @click="selectedRow = row">
                <td class="bcp-row-number">{{ row.number.toLocaleString() }}</td>
                <td v-for="column in visibleColumns" :key="column.index" :title="row.values[column.index] || ''">{{ row.values[column.index] || '—' }}</td>
                <td class="bcp-source" :title="row.fileName">{{ row.fileName }}</td>
              </tr>
            </tbody>
          </table>
          <div v-if="filteredRows.length === 0" class="ui-empty bcp-no-rows"><p class="ui-empty-title">{{ filter ? tr("当前页没有匹配的记录") : tr("该数据集没有记录") }}</p></div>
        </div>

      </main>
      <UiTransition kind="dock">
        <DetailDock v-if="selectedRow && selectedDataset" :label="tr('记录详情')" :close-label="tr('关闭详情')"
          :title="tr('记录 #{0}', { 0: selectedRow.number.toLocaleString() })" :content-key="`${selectedRow.fileName}:${selectedRow.number}`" @close="selectedRow = null">
          <FileDetailLayout :title="tr('记录 #{0}', { 0: selectedRow.number.toLocaleString() })">
            <template #icon><Database :size="20" /></template>
            <template #badges><span class="ui-badge">{{ tr("BCP 记录") }}</span><span class="ui-badge">{{ bcpDatasetLabel(selectedDataset, tr) }}</span></template>
            <FileDetailSection :title="tr('基本信息')" wide>
              <FileDetailFields :fields="[
                { label: tr('来源文件'), value: selectedRow.fileName, copyable: true },
                { label: tr('数据集'), value: selectedDataset.code, mono: true, copyable: true },
              ]" />
            </FileDetailSection>
            <FileDetailSection :title="tr('记录内容')" wide>
              <FileDetailFields :fields="[
                ...selectedDataset.fields.map((field, index) => ({ key: `${field.key}:${index}`, label: field.label || field.name, value: selectedRow!.values[index], copyable: true })),
                ...selectedRow.values.slice(selectedDataset.fields.length).map((value, index) => ({ key: `extra:${index}`, label: tr('附加字段 {0}', { 0: index + 1 }), value, copyable: true })),
              ]" />
            </FileDetailSection>
          </FileDetailLayout>
        </DetailDock>
      </UiTransition>
    </div>
    <div class="ui-footbar bcp-footbar">
      <span class="ui-footbar-stat">{{ catalog?.path ?? '' }}</span>
      <span class="ui-footbar-stat">{{ page ? tr("当前页 {0} / {1} 条", { 0: filteredRows.length, 1: page.rows.length }) : '' }}</span>
    </div>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import UiTransition from '../../../components/ui/UiTransition.vue';
import DetailDock from '../../../components/layout/DetailDock.vue';
import FileDetailLayout from '../../../components/layout/FileDetailLayout.vue';
import FileDetailSection from '../../../components/layout/FileDetailSection.vue';
import FileDetailFields from '../../../components/layout/FileDetailFields.vue';
import { computed, ref, watch } from 'vue';
import { ChevronLeft, ChevronRight, Database, Loader2 } from 'lucide-vue-next';
import { bcpRows, type BcpCatalog, type BcpCursor, type BcpPage, type BcpRow } from '../../../lib/ipc/bcp';
import { BCP_CATEGORIES, bcpCategory, bcpDatasetLabel, bcpSuffix } from '../model/bcpView';

const props = defineProps<{ imageId: string; catalog: BcpCatalog | null; catalogError: string; preferredFile: string; category: string }>();
const selectedCode = ref('');
const page = ref<BcpPage | null>(null);
const selectedRow = ref<BcpRow | null>(null);
const history = ref<(BcpCursor | null)[]>([]);
const loading = ref(false);
const error = ref('');
const filter = ref('');
let requestSerial = 0;

const categoryLabel = computed(() => BCP_CATEGORIES.find(category => category.prefix === props.category)?.label ?? tr("BCP 数据"));
const categoryDatasets = computed(() => props.catalog?.datasets.filter(dataset => !props.category || bcpCategory(dataset.code) === props.category) ?? []);
const selectedDataset = computed(() => categoryDatasets.value.find(dataset => dataset.code === selectedCode.value) ?? null);
const visibleColumns = computed(() => {
  const fields = selectedDataset.value?.fields ?? [];
  const indices = fields.map((field, index) => ({ field, index })).filter(({ field }) => field.name !== 'COLLECT_TARGET_ID');
  const chosen = indices.slice(0, 5);
  const content = indices.find(({ field }) => /内容|消息|文本/.test(field.label));
  if (content && !chosen.some(column => column.index === content.index)) chosen.push(content);
  return chosen;
});
const filteredRows = computed(() => {
  const query = filter.value.trim().toLocaleLowerCase();
  if (!query) return page.value?.rows ?? [];
  return page.value?.rows.filter(row => row.values.some(value => value.toLocaleLowerCase().includes(query))) ?? [];
});

async function loadPage(cursor: BcpCursor | null) {
  const serial = ++requestSerial;
  loading.value = true;
  error.value = '';
  selectedRow.value = null;
  try {
    const result = await bcpRows(props.imageId, selectedCode.value, cursor);
    if (serial === requestSerial) page.value = result;
  } catch (cause) {
    if (serial === requestSerial) { page.value = null; error.value = String(cause); }
  } finally {
    if (serial === requestSerial) loading.value = false;
  }
}

function selectDataset(code: string) {
  selectedCode.value = code;
  history.value = [];
  filter.value = '';
  page.value = null;
  void loadPage(null);
}

function nextPage() {
  if (!page.value?.nextCursor) return;
  const cursor = page.value.nextCursor;
  history.value.push(cursor);
  void loadPage(cursor);
}

function previousPage() {
  if (!history.value.length) return;
  history.value.pop();
  void loadPage(history.value.at(-1) ?? null);
}

watch(() => [props.catalog, props.category, props.imageId, props.preferredFile], () => {
  const preferred = props.preferredFile.toLocaleLowerCase();
  const first = categoryDatasets.value.find(dataset => dataset.files.some(file => file.name.toLocaleLowerCase() === preferred))
    ?? categoryDatasets.value[0];
  if (first) selectDataset(first.code);
  else { ++requestSerial; selectedCode.value = ''; page.value = null; history.value = []; }
}, { immediate: true });
</script>

<style scoped>
.bcp-workspace { display: flex; flex-direction: column; height: 100%; min-height: 0; color: var(--text-primary); background: var(--bg-primary); }
.bcp-toolbar { flex: none; }
.bcp-toolbar-context, .bcp-page-controls, .bcp-dataset-summary { display: flex; align-items: center; gap: 9px; min-width: 0; }
.bcp-toolbar-context > svg { color: var(--primary-color); }
.bcp-search { width: 220px; }
.bcp-body { position: relative; container: detail-host / inline-size; display: flex; flex: 1; min-height: 0; }
.bcp-datasets { width: 206px; flex: none; overflow: auto; border-right: 1px solid var(--border-color); background: var(--surface-secondary); padding: 12px 8px; }
.bcp-datasets-heading { display: flex; justify-content: space-between; padding: 4px 9px 11px; font-size: 11px; font-weight: 700; color: var(--text-secondary); }
.bcp-dataset { width: 100%; display: grid; grid-template-columns: 1fr auto; gap: 3px 7px; border: 0; border-radius: var(--radius-md); padding: 9px; margin-bottom: 3px; text-align: left; background: transparent; color: var(--text-primary); cursor: pointer; }
.bcp-dataset:hover { background: var(--sidebar-hover-bg); }
.bcp-dataset.active { background: var(--sidebar-active-bg); color: var(--primary-color); }
.bcp-dataset-label { font-size: 12px; font-weight: 600; }
.bcp-dataset-code { font-size: 10px; color: var(--text-secondary); font-family: var(--font-mono); }
.bcp-dataset-count { grid-column: 1 / -1; font-size: 11px; color: var(--text-secondary); font-family: var(--font-mono); }
.bcp-main { display: flex; flex: 1; min-width: 0; min-height: 0; flex-direction: column; }
.bcp-dataset-head { display: flex; justify-content: space-between; gap: 12px; align-items: center; flex: none; padding: 12px 16px; border-bottom: 1px solid var(--border-color); }
.bcp-dataset-summary { flex-wrap: wrap; font-size: 11px; color: var(--text-secondary); }
.bcp-dataset-summary strong { font-size: 14px; color: var(--text-primary); }
.bcp-code { font-family: var(--font-mono); }
.bcp-page-controls { font-size: 11px; color: var(--text-secondary); white-space: nowrap; }
.bcp-loading { display: flex; align-items: center; gap: 8px; margin: 25px; color: var(--text-secondary); }
.bcp-table-wrap { flex: 1; min-height: 0; overflow: auto; }
.bcp-table { width: 100%; border-collapse: collapse; table-layout: fixed; font-size: 12px; }
.bcp-table th { position: sticky; top: 0; z-index: 1; height: 34px; text-align: left; color: var(--text-secondary); background: var(--surface-secondary); border-bottom: 1px solid var(--border-color); }
.bcp-table td { height: 36px; border-bottom: 1px solid var(--border-color); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.bcp-table th, .bcp-table td { padding: 0 11px; min-width: 110px; }
.bcp-table th:first-child, .bcp-table td:first-child { width: 75px; min-width: 75px; }
.bcp-table th:last-child, .bcp-table td:last-child { width: 190px; }
.bcp-table tbody tr { cursor: pointer; }
.bcp-table tbody tr:hover { background: var(--sidebar-hover-bg); }
.bcp-table tbody tr.selected { background: var(--sidebar-active-bg); }
.bcp-row-number, .bcp-source { font-family: var(--font-mono); color: var(--text-secondary); }
.bcp-footbar { flex: none; justify-content: space-between; }
.bcp-footbar span:first-child { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.bcp-empty { margin: auto; }
.bcp-no-rows { padding: 35px; }
@media (max-width: 950px) { .bcp-datasets { width: 155px; } .bcp-dataset-head { align-items: flex-start; flex-direction: column; } }
</style>
