<template>
  <div class="realm-viewer">
    <div v-if="loading" class="state-row mono">
      <Loader2 :size="14" class="spin" />
      {{ tr("正在解析 Realm 数据库…") }}
    </div>
    <div v-else-if="error" class="state-row mono error">{{ tr("Realm 解析失败：") }}{{ error }}</div>

    <div v-else-if="digest && !digest.tables.length" class="empty-db">
      <div class="state-row mono">{{ tr("库内没有可显示的表。") }}</div>
    </div>

    <template v-else-if="digest">
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
        <header class="realm-head">
          <div class="realm-title">
            <Database :size="15" />
            <span>Realm</span>
            <span class="ui-badge sm mono">{{ digest.tables.length }} {{ tr("表") }}</span>
          </div>
          <div class="realm-meta mono">
            <span>{{ tr("读取") }} {{ formatBytes(digest.bytesRead) }}</span>
          </div>
        </header>

        <div v-if="digest.notes.length" class="note-row">
          <AlertTriangle :size="13" />
          <span>{{ digest.notes.join(' ') }}</span>
        </div>

        <div v-if="activeTableNumber === null" class="state-row mono">{{ tr("在左侧选择一张表。") }}</div>
        <div v-else-if="rowsLoading && !rows.length" class="state-row mono">
          <Loader2 :size="14" class="spin" />
          {{ tr("加载中…") }}
        </div>
        <div v-else-if="rowsError" class="state-row mono error">{{ rowsError }}</div>

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

          <footer class="pager">
            <span class="pager-info mono">
              <Loader2 v-if="rowsLoading" :size="12" class="spin" />
              {{ rangeStart }}–{{ rangeEnd }} / {{ total }}
            </span>
            <span class="pager-spacer" />
            <button class="ui-icon-btn sm bordered" :title="tr('上一页')" :aria-label="tr('上一页')" :disabled="pageIndex <= 0 || rowsLoading" @click="prevPage">
              <ChevronLeft :size="14" />
            </button>
            <span class="pager-page mono">{{ pageIndex + 1 }} / {{ pageCount }}</span>
            <button class="ui-icon-btn sm bordered" :title="tr('下一页')" :aria-label="tr('下一页')" :disabled="pageIndex >= pageCount - 1 || rowsLoading" @click="nextPage">
              <ChevronRight :size="14" />
            </button>
          </footer>
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
  Database,
  Loader2,
  Table2,
} from 'lucide-vue-next';
import { fsRealmPreview, fsRealmTableRows, type RealmCell, type RealmColumn, type RealmDigest, type RealmLink } from '../../../../lib/ipc/realm';
import { formatBytes } from '../../../../utils/format';

const props = defineProps<{
  imageId: string;
  nodeId: string;
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

function selectTable(tableNumber: number) {
  if (tableNumber === activeTableNumber.value) return;
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

.table-list {
  flex-basis: 220px;
}

.realm-head {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: 12px;
  min-height: 40px;
  padding: 6px 12px;
  border-bottom: 1px solid var(--border-color);
  background: var(--surface-primary);
}

.realm-title {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
  font-size: 13px;
  font-weight: 600;
}

.realm-meta {
  margin-left: auto;
  display: flex;
  gap: 10px;
  font-size: 11px;
  color: var(--text-secondary);
  white-space: nowrap;
}

.note-row {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 12px;
  font-size: 12px;
  color: var(--sev-warn-fg);
  background: var(--sev-warn-bg);
  border-bottom: 1px solid var(--border-color);
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
  font-size: 10px;
  line-height: 1.5;
  padding: 0 5px;
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
