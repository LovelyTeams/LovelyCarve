<template>
  <div class="sqlite-viewer">
    <PreviewState v-if="loading" tone="loading" :title="tr('正在解析 SQLite 数据库…')" />

    <!-- 加密库：密钥输入表单 -->
    <div v-else-if="locked" class="lock-wrap">
      <form class="lock-card" @submit.prevent="submitKey">
        <div class="lock-head">
          <Lock :size="15" />
          <span>{{ tr("数据库已加密（SQLCipher）") }}</span>
        </div>

        <div class="ui-seg seg">
          <button type="button" class="ui-seg-item" :class="{ active: keyMode === 'passphrase' }" @click="keyMode = 'passphrase'">{{ tr("口令") }}</button>
          <button type="button" class="ui-seg-item" :class="{ active: keyMode === 'raw' }" @click="keyMode = 'raw'">Raw Key (hex)</button>
        </div>

        <input
          v-if="keyMode === 'passphrase'"
          ref="firstInput"
          v-model="passphrase"
          type="password"
          class="ui-input mono lock-input"
          :placeholder="tr('数据库口令')"
          autocomplete="off"
        />
        <input
          v-else
          ref="firstInput"
          v-model="rawHexKey"
          type="text"
          class="ui-input mono lock-input"
          :placeholder="tr('64 个十六进制字符（32 字节），可选再带 32 字符 salt')"
          autocomplete="off"
          spellcheck="false"
        />

        <details v-disclosure-motion class="ui-disclosure lock-adv" :open="showAdvanced">
          <summary><ChevronRight :size="13" class="ui-chevron" />{{ tr("高级参数") }}</summary>
          <div class="adv-grid">
            <label class="adv-field">
              <span class="adv-k">{{ tr("兼容版本") }}</span>
              <UiSelect v-model="compatibility" class="sm mono lock-input" :options="COMPATIBILITY_OPTIONS" />
            </label>
            <label class="adv-field">
              <span class="adv-k">{{ tr("页大小") }}</span>
              <input v-model.number="pageSize" type="number" min="512" step="512" class="ui-input sm mono lock-input" :placeholder="tr('默认')" />
            </label>
            <label class="adv-field">
              <span class="adv-k">kdf_iter</span>
              <input v-model.number="kdfIter" type="number" min="1" class="ui-input sm mono lock-input" :placeholder="tr('默认')" />
            </label>
          </div>
          <div class="adv-presets">
            <span class="adv-k">{{ tr("预设：") }}</span>
            <button type="button" class="ui-btn sm" @click="applyWechatPreset">{{ tr("微信 EnMicroMsg") }}</button>
          </div>
        </details>

        <p v-if="keyError" class="ui-field-error" role="alert"><CircleAlert :size="12" /><span>{{ keyError }}</span></p>

        <button type="submit" class="ui-btn primary lock-submit" :disabled="!canSubmit || unlocking">
          <Loader2 v-if="unlocking" :size="14" class="spin" />
          <KeyRound v-else :size="14" />
          {{ unlocking ? tr("解密中…") : tr("解密") }}
        </button>
      </form>
    </div>

    <template v-else-if="digest || error">
      <!-- 面板较窄时表清单收起，改由视图栏里的下拉框选表、SQL 查询或已删除记录。 -->
      <PreviewChromeSlot area="tools">
        <UiSelect :model-value="sourceValue" class="sm mono source-picker" :options="sourceOptions" :aria-label="tr('数据表')"
          @update:model-value="selectSource" />
      </PreviewChromeSlot>
      <!-- 库的状态（已解密、已回放 WAL）放进状态栏。 -->
      <PreviewChromeSlot area="statusStart">
        <span v-if="digest?.encrypted" class="ui-badge sm success"><LockOpen :size="11" />{{ tr("已解密（SQLCipher）") }}</span>
        <span v-if="digest?.sidecars.length" class="ui-badge sm info" :title="tr('预览是 SQLite 回放 WAL 中已提交事务后的状态，比主库文件更新')">
          <History :size="11" />{{ tr("已回放") }} {{ joinList(digest.sidecars, tr) }}
        </span>
      </PreviewChromeSlot>

      <!-- 左：表清单，末尾是「已删除记录」 -->
      <aside class="table-list">
        <button class="table-item" :class="{ active: showSql }" :aria-pressed="showSql" @click="openSql">
          <CodeXml :size="13" class="table-icon" />
          <span class="table-name">{{ tr("SQL 查询") }}</span>
        </button>
        <div class="list-sep" aria-hidden="true" />
        <button
          v-for="t in digest?.tables"
          :key="t.name"
          class="table-item"
          :class="{ active: !showDeleted && !showSql && t.name === activeName }"
          :title="t.name"
          @click="selectTable(t.name)"
        >
          <Table2 v-if="t.kind === 'table'" :size="13" class="table-icon" />
          <Eye v-else :size="13" class="table-icon" />
          <span class="table-name mono">{{ t.name }}</span>
          <span class="row-pill mono">{{ t.error ? '!' : t.row_count ?? '—' }}</span>
        </button>
        <div v-if="digest && !digest.tables.length" class="list-empty">{{ tr("没有用户表") }}</div>
        <div v-if="digest" class="list-sep" aria-hidden="true" />
        <button
          v-if="digest"
          class="table-item"
          :class="{ active: showDeleted }"
          :title="tr('从空闲页、页内空闲区、WAL 与回滚日志中恢复已删除的行')"
          @click="openDeleted"
        >
          <ArchiveRestore :size="13" class="table-icon" />
          <span class="table-name">{{ tr("已删除记录") }}</span>
          <span v-if="deletedCount !== null" class="row-pill mono">{{ deletedCount.toLocaleString() }}</span>
        </button>
      </aside>

      <!-- 右：所选表的数据，或已删除记录 -->
      <div class="grid-pane">
        <SqliteDeletedRecords
          v-if="digest && deletedOpened"
          v-show="showDeleted"
          :active="showDeleted"
          :image-id="imageId"
          :node-id="nodeId"
          :parent-id="parentId ?? null"
          :sqlite-key="activeKey"
          :tables="digest.tables"
          @counted="deletedCount = $event"
        />
        <SqliteQueryPanel
          v-if="sqlOpened"
          v-show="showSql"
          :active="showSql"
          :image-id="imageId"
          :node-id="nodeId"
          :parent-id="parentId ?? null"
          :sqlite-key="activeKey"
          :initial-table="sqlTable"
        />
        <template v-if="!showDeleted && !showSql">
          <PreviewState v-if="error" tone="error" :icon="CircleAlert" :title="tr('SQLite 解析失败')" :description="error" mono-description>
            <button type="button" class="ui-btn sm" @click="openSql"><CodeXml :size="13" />{{ tr("SQL 查询") }}</button>
          </PreviewState>
          <PreviewState v-else-if="!digest?.tables.length" :icon="Table2" :title="tr('没有用户表')">
            <button type="button" class="ui-btn sm" @click="openDeleted"><ArchiveRestore :size="13" />{{ tr("已删除记录") }}</button>
          </PreviewState>
          <PreviewState v-else-if="!activeName" :icon="Table2" :title="tr('选择一张表查看数据。')" />
          <PreviewState v-else-if="rowsLoading && !rows.length" tone="loading" :title="tr('加载中…')" />
          <PreviewState v-else-if="rowsError" tone="error" :icon="CircleAlert" :title="tr('读取表失败')" :description="rowsError" mono-description />
          <template v-else>
            <div ref="gridEl" class="grid-scroll">
              <table class="data-table db-table">
                <thead>
                  <tr>
                    <th class="rownum-th mono">{{ tr("序号") }}</th>
                    <th
                      v-for="c in columns"
                      :key="c.name"
                      class="sortable"
                      :class="{ sorted: orderBy === c.name }"
                      :title="tr('按 {0} 排序', { 0: c.name })"
                      @click="toggleSort(c.name)"
                    >
                      <span class="col-head">
                        <span class="col-text">
                          <span class="col-name mono">{{ c.name }}</span>
                          <span class="col-meta mono">
                            {{ c.type_name || '?' }}{{ c.pk ? ' · PK' : '' }}{{ c.notnull ? ' · NOT NULL' : '' }}
                          </span>
                        </span>
                        <ChevronUp v-if="orderBy === c.name && !descending" :size="12" class="sort-icon" />
                        <ChevronDown v-else-if="orderBy === c.name && descending" :size="12" class="sort-icon" />
                      </span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="(row, ri) in rows" :key="ri">
                    <td class="rownum mono">{{ offset + ri + 1 }}</td>
                    <td v-for="(cell, ci) in row" :key="ci" class="mono" :class="sqliteCellClass(cell)">
                      <span v-if="highlightedRows" v-html="highlightedRows.cells[ri][ci]"></span>
                      <template v-else>{{ sqliteCellText(cell) }}</template>
                    </td>
                  </tr>
                  <tr v-if="!rows.length">
                    <td class="mono dim empty-cell" :colspan="columns.length + 1">{{ tr("（无数据）") }}</td>
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
        </template>
      </div>
    </template>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../../composables/useLocale';
import { joinList } from '../../../../i18n/translate';

import { vDisclosureMotion } from '../../../../composables/uiMotionDirectives';
// SQLite 库的「原类型视图」：左侧表清单（名称 + 行数），右侧按需分页取
// 所选表的行（点列头排序，底部翻页）。加密库（SQLCipher）先弹密钥表单，
// 解密后同样浏览；后端复用预览 VFS 会话，翻页/排序不重新读取整库。表清单
// 末尾的「已删除记录」第一次打开时才扫描，之后在切换表时保留结果。
import { computed, nextTick, ref, watch } from 'vue';
import {
  ArchiveRestore, ChevronDown, ChevronLeft, ChevronRight, ChevronUp, CircleAlert, CodeXml,
  Eye, History, KeyRound, Loader2, Lock, LockOpen, Table2,
} from 'lucide-vue-next';
import PreviewChromeSlot from '../PreviewChromeSlot.vue';
import PreviewState from '../PreviewState.vue';
import { fsSqlitePreview, fsSqliteTableRows, SQLITE_LOCKED, type SqliteCell, type SqliteColumn, type SqliteDigest, type SqliteKey } from '../../../../lib/ipc/sqlite';
import { highlightHtml } from '../../../../utils/highlight';
import { sqliteCellClass, sqliteCellText } from '../../model/sqliteCell';
import SqliteDeletedRecords from './SqliteDeletedRecords.vue';
import SqliteQueryPanel from './SqliteQueryPanel.vue';
import UiSelect, { type UiSelectOption } from '../../../../components/ui/UiSelect.vue';
import type { ImageId, NodeId } from '../../../../lib/ipc/ids';

const props = defineProps<{
  imageId: ImageId;
  nodeId: NodeId;
  /** 库所在目录的节点，用来查找同目录的 -wal / -journal。 */
  parentId?: NodeId | null;
  query: string;
}>();
const emit = defineEmits<{ resetMatch: [] }>();

const PAGE_SIZE = 100;

const loading = ref(false);
const error = ref<string | null>(null);
const digest = ref<SqliteDigest | null>(null);
const activeName = ref<string | null>(null);
const showSql = ref(false);
const sqlOpened = ref(false);
const sqlTable = ref<string | null>(null);
let digestSequence = 0;

// 解密表单状态。
const locked = ref(false);
const unlocking = ref(false);
const keyError = ref<string | null>(null);
const keyMode = ref<'passphrase' | 'raw'>('passphrase');
const passphrase = ref('');
const rawHexKey = ref('');
const compatibility = ref<number | ''>('');
const COMPATIBILITY_OPTIONS: UiSelectOption<number | ''>[] = [
  { value: '', get label() { return tr("默认（4）"); } },
  ...[4, 3, 2, 1].map(version => ({ value: version, label: `SQLCipher ${version}` })),
];
const pageSize = ref<number | ''>('');
const kdfIter = ref<number | ''>('');
const showAdvanced = ref(false);
const firstInput = ref<HTMLInputElement | null>(null);
// 解密成功的密钥：分页取数沿用同一把，避免每页重新弹窗。
const activeKey = ref<SqliteKey | null>(null);

const rowsLoading = ref(false);
const rowsError = ref<string | null>(null);
const columns = ref<SqliteColumn[]>([]);
const rows = ref<SqliteCell[][]>([]);
const total = ref(0);
const offset = ref(0);
const orderBy = ref<string | null>(null);
const descending = ref(false);
const gridEl = ref<HTMLDivElement | null>(null);
// 单调请求序号：快速切表/翻页时丢弃过期响应，避免乱序覆盖。
let reqSeq = 0;

// 「已删除记录」：打开过一次后保持挂载，切回表格只是隐藏。
const showDeleted = ref(false);
const deletedOpened = ref(false);
const deletedCount = ref<number | null>(null);

// 查找范围为当前页的单元格预览值。按表格渲染顺序编号，翻页后重新计数。
const highlightedRows = computed(() => {
  const query = props.query;
  if (!query || showDeleted.value) return null;
  let count = 0;
  const cells = rows.value.map(row => row.map(cell => {
    const highlighted = highlightHtml(sqliteCellText(cell), query);
    count += highlighted.count;
    return highlighted.html;
  }));
  return { cells, count };
});
const matchCount = computed(() => gridEl.value ? highlightedRows.value?.count ?? 0 : null);

function focusMatch(index: number) {
  const grid = gridEl.value;
  grid?.querySelector('mark.current')?.classList.remove('current');
  if (index < 0 || !grid) return;
  const target = grid.querySelectorAll('mark')[index];
  target?.classList.add('current');
  target?.scrollIntoView({ block: 'center', inline: 'nearest' });
}

watch([rows, showDeleted, showSql], () => emit('resetMatch'));
defineExpose({ matchCount, focusMatch });

const activeMeta = computed(() => digest.value?.tables.find((t) => t.name === activeName.value) ?? null);

// 窄面板里代替表清单的下拉框：SQL 查询、各表（附行数）与已删除记录。表名不会含 NUL。
const SQL_SOURCE = '\u0000sql';
const DELETED_SOURCE = '\u0000deleted';
const sourceValue = computed(() => (showSql.value ? SQL_SOURCE : showDeleted.value ? DELETED_SOURCE : activeName.value ?? ''));
const sourceOptions = computed<UiSelectOption<string>[]>(() => [
  ...(sourceValue.value === '' ? [{ value: '', label: tr('选择数据表'), disabled: true }] : []),
  { value: SQL_SOURCE, label: tr('SQL 查询') },
  ...(digest.value?.tables ?? []).map((table) => ({
    value: table.name,
    label: table.error ? tr('{0}（解析失败）', { 0: table.name })
      : table.row_count === null ? tr('{0}（视图）', { 0: table.name })
        : tr('{0}（{1} 行）', { 0: table.name, 1: table.row_count.toLocaleString() }),
  })),
  ...(digest.value ? [{
    value: DELETED_SOURCE,
    label: deletedCount.value === null ? tr('已删除记录') : tr('已删除记录（{0}）', { 0: deletedCount.value.toLocaleString() }),
  }] : []),
]);

function selectSource(value: string) {
  if (value === SQL_SOURCE) openSql();
  else if (value === DELETED_SOURCE) openDeleted();
  else if (value) selectTable(value);
}
const pageIndex = computed(() => Math.floor(offset.value / PAGE_SIZE));
const pageCount = computed(() => Math.max(1, Math.ceil(total.value / PAGE_SIZE)));
const rangeStart = computed(() => (total.value === 0 ? 0 : offset.value + 1));
const rangeEnd = computed(() => offset.value + rows.value.length);
const canSubmit = computed(() =>
  keyMode.value === 'passphrase' ? passphrase.value.length > 0 : rawHexKey.value.trim().length > 0,
);

function buildKey(): SqliteKey {
  const k: SqliteKey = {};
  if (keyMode.value === 'raw') k.rawHexKey = rawHexKey.value.trim();
  else k.passphrase = passphrase.value;
  if (compatibility.value !== '') k.compatibility = compatibility.value;
  if (pageSize.value !== '') k.pageSize = pageSize.value;
  if (kdfIter.value !== '') k.kdfIter = kdfIter.value;
  return k;
}

// 微信 Android EnMicroMsg.db 的典型参数：SQLCipher v1（HMAC 关、1024 页、
// kdf_iter=4000，cipher_compatibility=1）。密钥是 MD5(IMEI+UIN) 取前 7 位的
// 文本口令——派生需自行算出后填到「口令」栏，这里只设兼容参数。
function applyWechatPreset() {
  keyMode.value = 'passphrase';
  compatibility.value = 1;
  pageSize.value = 1024;
  kdfIter.value = 4000;
  showAdvanced.value = true;
}

function submitKey() {
  if (!canSubmit.value) return;
  unlocking.value = true;
  loadDigest(buildKey());
}

function openDeleted() {
  showSql.value = false;
  showDeleted.value = true;
  deletedOpened.value = true;
}

function openSql() {
  showDeleted.value = false;
  if (!sqlOpened.value) sqlTable.value = activeName.value;
  sqlOpened.value = true;
  showSql.value = true;
}

function selectTable(name: string) {
  showSql.value = false;
  showDeleted.value = false;
  if (name === activeName.value) return;
  activeName.value = name;
  orderBy.value = null;
  descending.value = false;
  rows.value = [];
  loadRows(0);
}

function toggleSort(col: string) {
  if (orderBy.value === col) {
    descending.value = !descending.value;
  } else {
    orderBy.value = col;
    descending.value = false;
  }
  loadRows(0);
}

function prevPage() {
  if (pageIndex.value > 0) loadRows(offset.value - PAGE_SIZE);
}
function nextPage() {
  if (pageIndex.value < pageCount.value - 1) loadRows(offset.value + PAGE_SIZE);
}

async function loadRows(nextOffset: number) {
  const name = activeName.value;
  if (!name) return;
  const my = ++reqSeq;
  const meta = activeMeta.value;
  // digest 已标记该表解析失败（坏页/虚表）——直接展示错误，不再取数。
  if (meta?.error) {
    rowsLoading.value = false;
    rowsError.value = meta.error;
    columns.value = meta.columns;
    rows.value = [];
    total.value = 0;
    offset.value = 0;
    return;
  }
  rowsLoading.value = true;
  rowsError.value = null;
  try {
    const res = await fsSqliteTableRows({
      imageId: props.imageId,
      nodeId: props.nodeId,
      parentId: props.parentId ?? null,
      table: name,
      offset: nextOffset,
      limit: PAGE_SIZE,
      orderBy: orderBy.value,
      descending: descending.value,
      key: activeKey.value ?? undefined,
    });
    if (my !== reqSeq) return;
    columns.value = res.columns;
    rows.value = res.rows;
    total.value = res.total;
    offset.value = res.offset;
  } catch (err) {
    if (my !== reqSeq) return;
    rowsError.value = String(err);
  } finally {
    if (my === reqSeq) rowsLoading.value = false;
  }
}

async function loadDigest(key?: SqliteKey) {
  const sequence = ++digestSequence;
  reqSeq++;
  rowsLoading.value = false;
  loading.value = true;
  error.value = null;
  locked.value = false;
  digest.value = null;
  activeName.value = null;
  rows.value = [];
  columns.value = [];
  total.value = 0;
  offset.value = 0;
  orderBy.value = null;
  descending.value = false;
  rowsError.value = null;
  showDeleted.value = false;
  deletedOpened.value = false;
  deletedCount.value = null;
  showSql.value = false;
  sqlOpened.value = false;
  try {
    const d = await fsSqlitePreview({
      imageId: props.imageId,
      nodeId: props.nodeId,
      parentId: props.parentId ?? null,
      key,
    });
    if (sequence !== digestSequence) return;
    digest.value = d;
    activeKey.value = key ?? null;
    const first = d.tables[0]?.name ?? null;
    if (first) {
      activeName.value = first;
      loadRows(0);
    }
  } catch (err) {
    if (sequence !== digestSequence) return;
    const msg = String(err);
    if (msg.includes(SQLITE_LOCKED)) {
      // 加密库：弹密钥表单。带 key 仍失败 = 密钥/参数不对。
      locked.value = true;
      keyError.value = key ? tr("密钥或兼容参数不正确，请检查后重试。") : null;
      nextTick(() => firstInput.value?.focus());
    } else {
      // A complex schema or view can exhaust the preview budget. A targeted
      // SQL query still uses its own connection and execution budget.
      activeKey.value = key ?? null;
      error.value = msg;
    }
  } finally {
    if (sequence === digestSequence) {
      loading.value = false;
      unlocking.value = false;
    }
  }
}

// 切换文件：重置表单与密钥，重新尝试（不带 key，触发加密检测）。
watch(
  () => [props.imageId, props.nodeId, props.parentId],
  () => {
    passphrase.value = '';
    rawHexKey.value = '';
    compatibility.value = '';
    pageSize.value = '';
    kdfIter.value = '';
    keyError.value = null;
    activeKey.value = null;
    loadDigest();
  },
  { immediate: true },
);
</script>

<style scoped src="../../../../styles/data-table.css"></style>

<style scoped src="../../../../styles/db-browser.css"></style>
<style scoped>
.db-table { width: max-content; min-width: 100%; }
.db-table td { max-width: 360px; }
.db-table td:has(mark.current) { white-space: normal; overflow-wrap: anywhere; }

.sqlite-viewer {
  display: flex;
  align-items: stretch;
  min-height: 0;
  overflow: hidden;
  box-sizing: border-box;
}

/* ── 解密表单 ── */
.lock-wrap {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 20px;
  overflow: auto;
}
.lock-card {
  width: 100%;
  max-width: 420px;
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 18px 20px;
  background: var(--surface-secondary);
  border: 1px solid var(--border-color);
  border-radius: var(--radius-lg);
  box-shadow: var(--card-shadow);
}
.lock-head {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
}
.seg { align-self: flex-start; }

.lock-adv { font-size: 12px; }
.lock-adv summary {
  color: var(--text-secondary);
  font-size: 12px;
  padding: 2px 0;
}
.lock-adv summary:hover { color: var(--text-primary); }
.adv-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(110px, 1fr));
  gap: 8px;
  margin-top: 8px;
}
.adv-field { display: flex; flex-direction: column; gap: 4px; }
.adv-field .ui-select { width: 100%; }
.adv-k { font-size: 11px; color: var(--text-secondary); }
.adv-presets {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-top: 8px;
}

.lock-submit { align-self: stretch; }

/* ── 左侧清单 ── */
.list-empty {
  padding: 6px 8px;
  font-size: 11px;
  color: var(--text-secondary);
}
.list-sep {
  flex: 0 0 auto;
  height: 1px;
  margin: 4px 2px;
  background: var(--border-color);
}

/* ── 右侧数据网格 ── */
.db-table th.sortable { cursor: pointer; user-select: none; }
.db-table th.sortable:hover { background: var(--hover-bg); }
.db-table th.sorted { color: var(--primary-color); }

.col-head {
  display: inline-flex;
  align-items: flex-end;
  gap: 4px;
}
.sort-icon { color: var(--primary-color); margin-bottom: 1px; }

.col-text { display: inline-flex; flex-direction: column; }
.db-table th.sorted .col-name { color: var(--primary-color); }

.cell-null { color: var(--text-secondary); opacity: 0.65; }
.cell-blob { color: var(--sev-info-fg); }
.empty-cell { padding: 10px 14px; }

.dim { color: var(--text-secondary); font-size: 11px; }
</style>
