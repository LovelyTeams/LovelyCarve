<template>
  <div class="deleted-pane">
    <div v-if="loading" class="state-row mono"><Loader2 :size="14" class="spin" /> {{ tr("正在扫描已删除记录…") }}</div>

    <div v-else-if="error" class="ui-alert danger load-alert" role="alert">
      <AlertCircle :size="14" />
      <span class="alert-text">{{ tr("扫描已删除记录失败：") }}{{ error }}</span>
      <button type="button" class="ui-btn sm" @click="load">{{ tr("重试") }}</button>
    </div>

    <template v-else-if="result">
      <div class="deleted-toolbar">
        <UiSelect v-model="tableFilter" class="sm" :options="tableOptions" :aria-label="tr('按表筛选')" />
        <UiSelect v-model="originFilter" class="sm" :options="originOptions" :aria-label="tr('按来源筛选')" />
        <span v-if="result.omitted" class="ui-badge sm warn" :title="tr('结果超过 {0} 条，只列出前 {1} 条', { 0: MAX_LISTED, 1: MAX_LISTED })">
          {{ tr("另有") }} {{ result.omitted.toLocaleString() }} {{ tr("条未列出") }}
        </span>
        <span class="toolbar-spacer" />
        <span class="deleted-facts mono">{{ facts }}</span>
      </div>

      <div v-if="result.notes.length || result.schemas.length" class="deleted-extras">
        <div v-for="note in result.notes" :key="note" class="ui-alert warn">
          <AlertTriangle :size="14" />
          <span>{{ note }}</span>
        </div>

        <details v-if="result.schemas.length" v-disclosure-motion class="ui-disclosure schema-box">
          <summary class="schema-summary">
            <ChevronRight :size="12" class="ui-chevron" />
            <span>{{ tr("已删除的表结构") }}</span>
            <span class="ui-count">{{ result.schemas.length }}</span>
          </summary>
          <ul class="schema-list">
            <li v-for="(schema, index) in result.schemas" :key="index" class="schema-item">
              <div class="schema-head">
                <span class="ui-badge sm outline">{{ schema.object_type }}</span>
                <span class="schema-name mono">{{ schema.name }}</span>
                <span v-if="schema.root_page !== null" class="dim mono">{{ tr("根页") }} {{ schema.root_page }}</span>
              </div>
              <pre class="schema-sql mono">{{ schema.sql }}</pre>
            </li>
          </ul>
        </details>
      </div>

      <div v-if="!result.records.length" class="ui-empty">
        <div class="ui-empty-icon"><ArchiveRestore :size="22" /></div>
        <p class="ui-empty-title">{{ tr("未发现已删除记录") }}</p>
        <p class="ui-empty-desc">{{ tr("空闲页、页内空闲区以及 WAL 和回滚日志里都没有可恢复的行。开启 secure_delete 或执行过 VACUUM 的库不会留下这类残留。") }}</p>
      </div>

      <div v-else-if="!filtered.length" class="ui-empty">
        <div class="ui-empty-icon"><FilterX :size="22" /></div>
        <p class="ui-empty-title">{{ tr("没有符合筛选条件的记录") }}</p>
        <div class="ui-empty-actions">
          <button type="button" class="ui-btn sm" @click="clearFilters">{{ tr("清除筛选") }}</button>
        </div>
      </div>

      <template v-else>
        <div class="grid-scroll">
          <table class="data-table db-table">
            <thead>
              <tr>
                <th class="rownum-th mono">{{ tr("序号") }}</th>
                <th>{{ tr("来源") }}</th>
                <th>{{ tr("表") }}</th>
                <th>{{ tr("位置") }}</th>
                <th class="numeric">rowid</th>
                <th class="numeric">{{ tr("置信度") }}</th>
                <th v-for="(header, index) in valueHeaders" :key="index">
                  <span class="col-text">
                    <span class="col-name mono">{{ header.name }}</span>
                    <span v-if="header.meta" class="col-meta mono">{{ header.meta }}</span>
                  </span>
                </th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="(record, index) in pageRecords" :key="offset + index">
                <td class="rownum mono">{{ offset + index + 1 }}</td>
                <td>
                  <span class="ui-badge sm tone" :style="{ '--badge-tone': ORIGIN_TONE[record.origin] }" :title="ORIGIN_TITLE[record.origin]">
                    {{ ORIGIN_LABEL[record.origin] }}
                  </span>
                </td>
                <td class="table-cell" :title="LINK_TITLE[record.link]">
                  <span v-if="record.table" class="mono">{{ record.table }}</span>
                  <span v-else class="dim">{{ tr("未归属") }}</span>
                  <span v-if="record.link === 'inferred'" class="ui-badge sm outline link-badge">{{ tr("推断") }}</span>
                  <span v-else-if="record.link === 'ambiguous'" class="ui-badge sm outline link-badge">{{ tr("多表匹配") }}</span>
                </td>
                <td class="mono" :title="overflowTitle(record)">{{ location(record) }}</td>
                <td class="mono numeric">{{ record.rowid ?? '—' }}</td>
                <td class="mono numeric">{{ record.confidence === null ? '—' : `${Math.round(record.confidence * 100)}%` }}</td>
                <!-- v-for over a number counts from 1 -->
                <td
                  v-for="column in valueHeaders.length"
                  :key="column"
                  class="mono"
                  :class="valueClass(record, column - 1)"
                  :title="valueTitle(record, column - 1)"
                >{{ valueText(record, column - 1) }}</td>
              </tr>
            </tbody>
          </table>
        </div>

        <footer class="pager">
          <span class="pager-info mono">{{ rangeStart }}–{{ rangeEnd }} / {{ filtered.length.toLocaleString() }}</span>
          <span class="pager-spacer" />
          <button class="ui-icon-btn sm bordered" :title="tr('上一页')" :aria-label="tr('上一页')" :disabled="pageIndex <= 0" @click="offset -= PAGE_SIZE">
            <ChevronLeft :size="14" />
          </button>
          <span class="pager-page mono">{{ pageIndex + 1 }} / {{ pageCount }}</span>
          <button class="ui-icon-btn sm bordered" :title="tr('下一页')" :aria-label="tr('下一页')" :disabled="pageIndex >= pageCount - 1" @click="offset += PAGE_SIZE">
            <ChevronRight :size="14" />
          </button>
        </footer>
      </template>
    </template>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../../composables/useLocale';

// SQLite 浏览器的「已删除记录」：后端从空闲页、页内空闲区、溢出链、未检查点
// 的 -wal 和回滚 -journal 中恢复的行（不含现存行）。按表、来源筛选，前端分页；
// 选中某张现存表时按它的列名显示值。回滚日志里修改前的版本，与当前值不同的
// 单元格会标出来。
import { computed, ref, shallowRef, watch } from 'vue';
import {
  AlertCircle, AlertTriangle, ArchiveRestore, ChevronLeft, ChevronRight, FilterX, Loader2,
} from 'lucide-vue-next';
import { vDisclosureMotion } from '../../../../composables/uiMotionDirectives';
import { fsSqliteDeletedRecords, type SqliteCell, type SqliteKey, type SqliteRecordOrigin, type SqliteRecoveredRecord, type SqliteRecovery, type SqliteTable, type SqliteTableLink } from '../../../../lib/ipc/sqlite';
import { sqliteCellClass, sqliteCellText } from '../../model/sqliteCell';
import UiSelect, { type UiSelectOption } from '../../../../components/ui/UiSelect.vue';

const props = defineProps<{
  imageId: string;
  nodeId: string;
  parentId: string | null;
  /** 浏览器解密该库时用的密钥；明文库为 null。 */
  sqliteKey: SqliteKey | null;
  /** 现存的表，用来给选中表的记录配上列名。 */
  tables: SqliteTable[];
}>();

const emit = defineEmits<{
  /** 扫描完成：恢复到的记录总数（含未列出的）。 */
  counted: [count: number];
}>();

const PAGE_SIZE = 100;
const MAX_LISTED = (20_000).toLocaleString();
// 「未归属」筛选项的值；表名不会含 NUL。
const UNATTRIBUTED = '\u0000unattributed';

const ORIGIN_ORDER: SqliteRecordOrigin[] = [
  'freelist_page', 'in_page', 'freeblock', 'prior_version', 'dropped_table',
  'wal_frame', 'wal_commit', 'journal_deleted', 'journal_prior',
];

const ORIGIN_LABEL: Record<SqliteRecordOrigin, string> = {
  get freelist_page() { return tr("空闲页"); },
  get in_page() { return tr("页内空闲区"); },
  get freeblock() { return tr("空闲块重建"); },
  get prior_version() { return tr("更新前版本"); },
  get dropped_table() { return tr("已删除的表"); },
  get wal_frame() { return tr("WAL 帧"); },
  get wal_commit() { return tr("WAL 提交"); },
  get journal_deleted() { return tr("日志 · 已删除"); },
  get journal_prior() { return tr("日志 · 修改前"); },
};

const ORIGIN_TITLE: Record<SqliteRecordOrigin, string> = {
  get freelist_page() { return tr("从释放到空闲链表的页中雕刻"); },
  get in_page() { return tr("从仍在使用的页的空闲区中雕刻"); },
  get freeblock() { return tr("单元头已被空闲块覆盖，按列结构重建，rowid 已丢失"); },
  get prior_version() { return tr("仍存在的行在更新前的旧版本"); },
  get dropped_table() { return tr("已不存在的表释放的页"); },
  get wal_frame() { return tr("未检查点的 -wal 中的页映像"); },
  get wal_commit() { return tr("WAL 提交快照"); },
  get journal_deleted() { return tr("最后一个事务删除的行，来自回滚日志"); },
  get journal_prior() { return tr("最后一个事务修改前的行，来自回滚日志"); },
};

const ORIGIN_TONE: Record<SqliteRecordOrigin, string> = {
  freelist_page: 'var(--chart-1)',
  in_page: 'var(--chart-1)',
  freeblock: 'var(--chart-6)',
  prior_version: 'var(--chart-4)',
  dropped_table: 'var(--chart-5)',
  wal_frame: 'var(--chart-2)',
  wal_commit: 'var(--chart-2)',
  journal_deleted: 'var(--chart-3)',
  journal_prior: 'var(--chart-3)',
};

const LINK_TITLE: Record<SqliteTableLink, string> = {
  get known() { return tr("记录所在的页仍属于这张表的 B 树"); },
  get inferred() { return tr("记录在空闲页上，行结构只与这张表吻合"); },
  get ambiguous() { return tr("行结构与多张表吻合，这里给出最接近的一张"); },
  get unattributed() { return tr("无法归属到现存的表，可能来自已删除的表"); },
};

const loading = ref(false);
const error = ref<string | null>(null);
const result = shallowRef<SqliteRecovery | null>(null);
const tableFilter = ref('');
const originFilter = ref<SqliteRecordOrigin | ''>('');
const offset = ref(0);
// 单调请求序号：密钥或文件变化时丢弃过期响应。
let requestSeq = 0;

const records = computed(() => result.value?.records ?? []);
const tableNames = computed(() =>
  [...new Set(records.value.flatMap((record) => (record.table === null ? [] : [record.table])))]
    .sort((left, right) => left.localeCompare(right)),
);
const hasUnattributed = computed(() => records.value.some((record) => record.table === null));
const origins = computed(() => ORIGIN_ORDER.filter((origin) => records.value.some((record) => record.origin === origin)));
const tableOptions = computed<UiSelectOption<string>[]>(() => [
  { value: '', get label() { return tr("全部表"); } },
  ...tableNames.value.map((name) => ({ value: name, label: name })),
  ...(hasUnattributed.value ? [{ value: UNATTRIBUTED, get label() { return tr("未归属"); } }] : []),
]);
const originOptions = computed<UiSelectOption<SqliteRecordOrigin | ''>[]>(() => [
  { value: '', get label() { return tr("全部来源"); } },
  ...origins.value.map((origin) => ({ value: origin, label: ORIGIN_LABEL[origin] })),
]);

const filtered = computed(() => records.value.filter((record) => {
  const table = tableFilter.value;
  const tableMatches = table === ''
    || (table === UNATTRIBUTED ? record.table === null : record.table === table);
  return tableMatches && (originFilter.value === '' || record.origin === originFilter.value);
}));

const schemaColumns = computed(() => {
  const table = tableFilter.value;
  if (table === '' || table === UNATTRIBUTED) return [];
  return props.tables.find((candidate) => candidate.name === table)?.columns ?? [];
});

const valueHeaders = computed(() => {
  const width = filtered.value.reduce(
    (widest, record) => Math.max(widest, record.values.length),
    schemaColumns.value.length,
  );
  return Array.from({ length: width }, (_, index) => {
    const column = schemaColumns.value[index];
    return column
      ? { name: column.name, meta: column.type_name || '?' }
      : { name: tr("列 {0}", { 0: index + 1 }), meta: '' };
  });
});

const pageRecords = computed(() => filtered.value.slice(offset.value, offset.value + PAGE_SIZE));
const pageIndex = computed(() => Math.floor(offset.value / PAGE_SIZE));
const pageCount = computed(() => Math.max(1, Math.ceil(filtered.value.length / PAGE_SIZE)));
const rangeStart = computed(() => (filtered.value.length === 0 ? 0 : offset.value + 1));
const rangeEnd = computed(() => offset.value + pageRecords.value.length);

const facts = computed(() => {
  const recovery = result.value;
  if (!recovery) return '';
  const parts = [
    recovery.cipher ?? tr("页大小 {0}", { 0: recovery.page_size }),
    tr("{0} 页", { 0: recovery.page_count.toLocaleString() }),
    tr("空闲页 {0}", { 0: recovery.freelist_pages.toLocaleString() }),
  ];
  if (recovery.sidecars.length) parts.push(tr("已结合 {0}", { 0: recovery.sidecars.join('、') }));
  return parts.join(' · ');
});

function location(record: SqliteRecoveredRecord): string {
  const parts: string[] = [];
  if (record.wal_frame !== null) parts.push(tr("WAL 帧 {0}", { 0: record.wal_frame }));
  if (record.page !== null) parts.push(tr("页 {0}", { 0: record.page }));
  if (record.offset !== null) parts.push(`0x${record.offset.toString(16).toUpperCase()}`);
  if (record.overflow_pages.length) parts.push(tr("溢出 {0} 页", { 0: record.overflow_pages.length }));
  return parts.join(' · ') || '—';
}

function overflowTitle(record: SqliteRecoveredRecord): string | undefined {
  return record.overflow_pages.length ? tr("溢出页：{0}", { 0: record.overflow_pages.join(', ') }) : undefined;
}

/** 两个单元格是否已知不同；BLOB 只下发大小，大小相同时无法判断。 */
function differs(left: SqliteCell, right: SqliteCell | undefined): boolean {
  if (!right || left.kind !== right.kind) return true;
  if (left.kind === 'null' || right.kind === 'null') return false;
  if (left.kind === 'blob' && right.kind === 'blob') return left.value.size !== right.value.size;
  return left.value !== right.value;
}

function changedCurrent(record: SqliteRecoveredRecord, column: number): SqliteCell | null {
  const cell = record.values[column];
  const current = record.current_values?.[column];
  if (!cell || !record.current_values || !differs(cell, current)) return null;
  return current ?? { kind: 'null' };
}

function valueText(record: SqliteRecoveredRecord, column: number): string {
  const cell = record.values[column];
  return cell ? sqliteCellText(cell) : '';
}

function valueClass(record: SqliteRecoveredRecord, column: number): string {
  const cell = record.values[column];
  if (!cell) return '';
  return [sqliteCellClass(cell), changedCurrent(record, column) ? 'cell-changed' : ''].join(' ');
}

function valueTitle(record: SqliteRecoveredRecord, column: number): string | undefined {
  const cell = record.values[column];
  if (!cell) return undefined;
  const current = changedCurrent(record, column);
  const text = sqliteCellText(cell);
  return current ? tr("{0}\n当前值：{1}", { 0: text, 1: sqliteCellText(current) }) : text;
}

function clearFilters() {
  tableFilter.value = '';
  originFilter.value = '';
}

async function load() {
  const request = ++requestSeq;
  loading.value = true;
  error.value = null;
  try {
    const recovery = await fsSqliteDeletedRecords({
      imageId: props.imageId,
      nodeId: props.nodeId,
      parentId: props.parentId,
      key: props.sqliteKey ?? undefined,
    });
    if (request !== requestSeq) return;
    result.value = recovery;
    clearFilters();
    offset.value = 0;
    emit('counted', recovery.records.length + recovery.omitted);
  } catch (err) {
    if (request !== requestSeq) return;
    result.value = null;
    error.value = String(err).replace(/^SQLITE_LOCKED:\s*/, '');
  } finally {
    if (request === requestSeq) loading.value = false;
  }
}

watch([tableFilter, originFilter], () => {
  offset.value = 0;
});

watch(
  () => [props.imageId, props.nodeId, props.parentId, props.sqliteKey],
  load,
  { immediate: true },
);
</script>

<style scoped src="../../../../styles/data-table.css"></style>
<style scoped src="../../../../styles/db-browser.css"></style>
<style scoped>
.deleted-pane {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.deleted-toolbar {
  flex: 0 0 auto;
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  border-bottom: 1px solid var(--border-color);
}
.toolbar-spacer { flex: 1; }
.deleted-facts {
  font-size: 11px;
  color: var(--text-secondary);
}

.load-alert {
  flex: 0 0 auto;
  align-items: center;
  margin: 12px;
}
.load-alert .alert-text { flex: 1; }

.deleted-extras {
  flex: 0 0 auto;
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 8px 12px;
  border-bottom: 1px solid var(--border-color);
}

.schema-box { font-size: 12px; }
.schema-summary {
  color: var(--text-secondary);
  padding: 2px 0;
}
.schema-summary:hover { color: var(--primary-color); }
.schema-list {
  list-style: none;
  margin: 6px 0 0;
  padding: 0;
  max-height: 200px;
  overflow: auto;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.schema-head {
  display: flex;
  align-items: center;
  gap: 6px;
}
.schema-name { color: var(--text-primary); }
.schema-sql {
  margin: 4px 0 0;
  padding: 6px 8px;
  font-size: 11px;
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-word;
  color: var(--text-secondary);
  background: var(--surface-tertiary);
  border-radius: var(--radius-sm);
}

.db-table { width: max-content; min-width: 100%; }
.db-table td { max-width: 360px; }
.col-text { display: inline-flex; flex-direction: column; }
.numeric { text-align: right; font-variant-numeric: tabular-nums; }
.table-cell .link-badge { margin-left: 6px; }

.cell-null { color: var(--text-secondary); opacity: 0.65; }
.cell-blob { color: var(--sev-info-fg); }
.cell-changed {
  color: var(--sev-warn-fg);
  background: var(--sev-warn-bg);
}

.dim { color: var(--text-secondary); font-size: 11px; }
</style>
