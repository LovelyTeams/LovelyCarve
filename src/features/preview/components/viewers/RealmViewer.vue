<template>
  <div class="realm-viewer">
    <PreviewState v-if="loading" tone="loading" :title="tr('正在解析 Realm 数据库…')" />
    <PreviewState v-else-if="error" tone="error" :icon="CircleAlert" :title="tr('Realm 解析失败')" :description="error" mono-description />
    <PreviewState v-else-if="digest && !digest.tables.length" :icon="Database" :title="tr('库内没有可显示的表。')" />

    <template v-else-if="digest">
      <!-- 面板较窄时表清单收进视图栏的下拉框；库的概况与翻页放进状态栏。 -->
      <PreviewChromeSlot area="tools">
        <UiSelect :model-value="activeTableNumber" class="sm mono source-picker" :options="tableOptions" :aria-label="tr('数据表')"
          @update:model-value="selectTable" />
      </PreviewChromeSlot>
      <PreviewChromeSlot area="statusStart">
        <span class="ui-badge sm mono">Realm</span>
        <span class="ui-footbar-stat"><strong>{{ digest.tables.length.toLocaleString() }}</strong> {{ tr("表") }}</span>
        <span class="ui-footbar-stat">{{ tr("读取") }} <strong>{{ formatBytes(digest.bytesRead) }}</strong></span>
      </PreviewChromeSlot>

      <aside class="table-list">
        <button
          v-for="table in digest.tables"
          :key="table.tableNumber"
          class="table-item"
          :class="{ active: table.tableNumber === activeTableNumber }"
          :title="table.name"
          @click="selectTable(table.tableNumber)"
        >
          <Table2 :size="13" class="table-icon" />
          <span class="table-name mono">{{ table.name }}</span>
          <span class="row-pill mono" :class="{ warn: !!table.error }">
            {{ table.error ? '!' : table.rowCount ?? '—' }}
          </span>
        </button>
      </aside>

      <div class="grid-pane">
        <p v-if="digest.notes.length" class="ui-alert warn realm-note" role="note">
          <AlertTriangle :size="14" />
          <span>{{ digest.notes.join(' ') }}</span>
        </p>

        <PreviewState v-if="activeTableNumber === null" :icon="Table2" :title="tr('选择一张表查看数据。')" />
        <PreviewState v-else-if="rowsLoading && !rows.length" tone="loading" :title="tr('加载中…')" />
        <PreviewState v-else-if="rowsError" tone="error" :icon="CircleAlert" :title="tr('读取表失败')" :description="rowsError" mono-description />

        <template v-else>
          <div class="grid-scroll">
            <table class="data-table realm-table">
              <thead>
                <tr>
                  <th class="rownum-th mono">{{ tr("序号") }}</th>
                  <th v-for="column in columns" :key="column.index" :title="columnTitle(column)">
                    <span class="col-head">
                      <span class="col-name mono">{{ columnName(column) }}</span>
                      <span class="col-meta mono">
                        #{{ column.index }}{{ column.nullable ? ' · NULL' : '' }}{{ column.indexed ? ' · INDEX' : '' }}
                      </span>
                    </span>
                  </th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="row in rows" :key="row.index">
                  <td class="rownum mono">{{ row.index + 1 }}</td>
                  <td v-for="(cell, index) in row.cells" :key="index" :class="cellClass(cell)">
                    <div class="cell-wrap">
                      <span class="type-pill mono">{{ cellKindLabel(cell) }}</span>
                      <span class="cell-value mono" :title="cellText(cell)">{{ cellText(cell) }}</span>
                    </div>
                  </td>
                </tr>
                <tr v-if="!rows.length">
                  <td class="empty mono" :colspan="columns.length + 1">{{ tr("（无数据）") }}</td>
                </tr>
              </tbody>
            </table>
          </div>

          <PreviewChromeSlot area="statusEnd">
            <span class="ui-footbar-stat pager-info">
              <Loader2 v-if="rowsLoading" :size="12" class="spin" />
              <strong>{{ rangeStart.toLocaleString() }}–{{ rangeEnd.toLocaleString() }}</strong> / {{ total.toLocaleString() }}
            </span>
            <div class="pager">
              <button type="button" class="ui-icon-btn sm" :title="tr('上一页')" :aria-label="tr('上一页')" :disabled="pageIndex <= 0 || rowsLoading" @click="prevPage">
                <ChevronLeft :size="14" />
              </button>
              <span class="pager-page mono">{{ pageIndex + 1 }} / {{ pageCount }}</span>
              <button type="button" class="ui-icon-btn sm" :title="tr('下一页')" :aria-label="tr('下一页')" :disabled="pageIndex >= pageCount - 1 || rowsLoading" @click="nextPage">
                <ChevronRight :size="14" />
              </button>
            </div>
          </PreviewChromeSlot>
        </template>
      </div>
    </template>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../../composables/useLocale';

import { computed, ref, watch } from 'vue';
import {
  AlertTriangle,
  ChevronLeft,
  ChevronRight,
  CircleAlert,
  Database,
  Loader2,
  Table2,
} from 'lucide-vue-next';
import { fsRealmPreview, fsRealmTableRows, type RealmCell, type RealmColumn, type RealmDigest, type RealmLink } from '../../../../lib/ipc/realm';
import { formatBytes } from '../../../../utils/format';
import UiSelect, { type UiSelectOption } from '../../../../components/ui/UiSelect.vue';
import PreviewChromeSlot from '../PreviewChromeSlot.vue';
import PreviewState from '../PreviewState.vue';
import type { ImageId, NodeId } from '../../../../lib/ipc/ids';

const props = defineProps<{
  imageId: ImageId;
  nodeId: NodeId;
}>();

const PAGE_SIZE = 100;

const loading = ref(false);
const error = ref<string | null>(null);
const digest = ref<RealmDigest | null>(null);
const activeTableNumber = ref<number | null>(null);
const rowsLoading = ref(false);
const rowsError = ref<string | null>(null);
const columns = ref<RealmColumn[]>([]);
const rows = ref<{ index: number; cells: RealmCell[] }[]>([]);
const total = ref(0);
const offset = ref(0);
let requestSeq = 0;

const pageIndex = computed(() => Math.floor(offset.value / PAGE_SIZE));
const pageCount = computed(() => Math.max(1, Math.ceil(total.value / PAGE_SIZE)));
const rangeStart = computed(() => (total.value === 0 ? 0 : offset.value + 1));
const rangeEnd = computed(() => offset.value + rows.value.length);

function columnName(column: RealmColumn): string {
  return column.name || `(backlink #${column.index})`;
}

function columnTitle(column: RealmColumn): string {
  const parts = [tr("列 #{0}", { 0: column.index }), column.name ? tr("名称：{0}", { 0: column.name }) : tr("未命名 backlink")];
  if (column.nullable) parts.push(tr("可为空"));
  if (column.indexed) parts.push(tr("已索引"));
  return parts.join(' · ');
}

// 窄面板里代替表清单的下拉框。
const tableOptions = computed<UiSelectOption<number | null>[]>(() => [
  ...(activeTableNumber.value === null ? [{ value: null, label: tr('选择数据表'), disabled: true }] : []),
  ...(digest.value?.tables ?? []).map(table => ({
    value: table.tableNumber,
    label: table.error ? tr('{0}（解析失败）', { 0: table.name })
      : table.rowCount === null ? table.name
        : tr('{0}（{1} 行）', { 0: table.name, 1: table.rowCount.toLocaleString() }),
  })),
]);

function selectTable(tableNumber: number | null) {
  if (tableNumber === null || tableNumber === activeTableNumber.value) return;
  activeTableNumber.value = tableNumber;
  loadRows(0);
}

function prevPage() {
  if (pageIndex.value > 0) loadRows(offset.value - PAGE_SIZE);
}

function nextPage() {
  if (pageIndex.value < pageCount.value - 1) loadRows(offset.value + PAGE_SIZE);
}

function linkText(link: RealmLink): string {
  return `T${link.targetTableNumber}#${link.rowNumber}`;
}

function cellText(cell: RealmCell): string {
  switch (cell.kind) {
    case 'null': return 'NULL';
    case 'int': return String(cell.value);
    case 'bool': return cell.value ? 'true' : 'false';
    case 'string': return `${cell.value.text}${cell.value.truncated ? '…' : ''}`;
    case 'binary': return `<BIN ${formatBytes(cell.value.size)}> ${groupHex(cell.value.hex)}${cell.value.truncated ? ' …' : ''}`;
    case 'table': return `<TABLE ${cell.value.rows} rows>`;
    case 'timestamp': return cell.value;
    case 'float':
    case 'double': return String(cell.value);
    case 'link': return `→ ${linkText(cell.value)}`;
    case 'linkList': {
      const links = cell.value.links.map(linkText).join(', ');
      return `[${links}${cell.value.truncated ? ', …' : ''}]`;
    }
    case 'backlink': {
      const rowsText = cell.value.rowNumbers.map(row => `#${row}`).join(', ');
      return `← T${cell.value.originTableNumber}.C${cell.value.originColumnNumber} [${rowsText}${cell.value.truncated ? ', …' : ''}]`;
    }
    case 'unsupported': return cell.value;
    case 'error': return cell.value;
  }
}

function cellKindLabel(cell: RealmCell): string {
  switch (cell.kind) {
    case 'null': return 'NULL';
    case 'int': return 'Int';
    case 'bool': return 'Bool';
    case 'string': return 'String';
    case 'binary': return 'Binary';
    case 'table': return 'Table';
    case 'timestamp': return 'Time';
    case 'float': return 'Float';
    case 'double': return 'Double';
    case 'link': return 'Link';
    case 'linkList': return 'LinkList';
    case 'backlink': return 'Backlink';
    case 'unsupported': return 'Unsupported';
    case 'error': return 'Error';
  }
}

function cellClass(cell: RealmCell): string {
  if (cell.kind === 'null') return 'cell-null';
  if (cell.kind === 'error') return 'cell-error';
  if (cell.kind === 'unsupported') return 'cell-warn';
  if (cell.kind === 'binary' || cell.kind === 'link' || cell.kind === 'linkList' || cell.kind === 'backlink') {
    return 'cell-ref';
  }
  return '';
}

function groupHex(hex: string): string {
  return hex.match(/.{1,2}/g)?.join(' ') ?? '';
}

async function loadRows(nextOffset: number) {
  const tableNumber = activeTableNumber.value;
  if (tableNumber === null) return;
  const meta = digest.value?.tables.find(table => table.tableNumber === tableNumber);
  if (meta?.error) {
    rowsError.value = meta.error;
    columns.value = meta.columns;
    rows.value = [];
    total.value = 0;
    offset.value = 0;
    return;
  }

  const my = ++requestSeq;
  rowsLoading.value = true;
  rowsError.value = null;
  try {
    const result = await fsRealmTableRows({
      imageId: props.imageId,
      nodeId: props.nodeId,
      tableNumber,
      offset: nextOffset,
      limit: PAGE_SIZE,
    });
    if (my !== requestSeq) return;
    columns.value = result.columns;
    rows.value = result.rows;
    total.value = result.total;
    offset.value = result.offset;
  } catch (err) {
    if (my !== requestSeq) return;
    rowsError.value = String(err);
  } finally {
    if (my === requestSeq) rowsLoading.value = false;
  }
}

async function loadDigest() {
  loading.value = true;
  error.value = null;
  digest.value = null;
  activeTableNumber.value = null;
  rows.value = [];
  columns.value = [];
  total.value = 0;
  offset.value = 0;
  rowsError.value = null;
  try {
    const result = await fsRealmPreview({ imageId: props.imageId, nodeId: props.nodeId });
    digest.value = result;
    const first = result.tables[0]?.tableNumber ?? null;
    if (first !== null) {
      activeTableNumber.value = first;
      loadRows(0);
    }
  } catch (err) {
    error.value = String(err);
  } finally {
    loading.value = false;
  }
}

watch(
  () => [props.imageId, props.nodeId],
  () => { void loadDigest(); },
  { immediate: true },
);
</script>

<style scoped src="../../../../styles/data-table.css"></style>

<style scoped src="../../../../styles/db-browser.css"></style>
<style scoped>
.realm-table { width: max-content; min-width: 100%; }
.realm-table td { max-width: 420px; min-width: 140px; }

.realm-viewer {
  display: flex;
  align-items: stretch;
  min-height: 0;
  overflow: hidden;
  background: var(--surface-secondary);
  color: var(--text-primary);
}

.realm-note {
  flex: 0 0 auto;
  margin: 8px 12px;
}

.col-head {
  display: inline-flex;
  flex-direction: column;
  gap: 1px;
}

.cell-wrap {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

.type-pill {
  flex: 0 0 auto;
  font-size: 11px;
  line-height: 1.5;
  padding: 0 6px;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
  background: var(--surface-tertiary);
}

.cell-value {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.cell-null .cell-value {
  color: var(--text-secondary);
  opacity: 0.65;
}

.cell-ref .type-pill {
  color: var(--sev-info-fg);
  border-color: var(--sev-info-border);
  background: var(--sev-info-bg);
}

.cell-warn .type-pill {
  color: var(--sev-warn-fg);
  border-color: var(--sev-warn-border);
  background: var(--sev-warn-bg);
}

.cell-error .type-pill {
  color: var(--sev-critical-fg);
  border-color: var(--sev-critical-border);
  background: var(--sev-critical-bg);
}

.empty {
  padding: 16px;
  text-align: center;
  color: var(--text-secondary);
}
</style>
