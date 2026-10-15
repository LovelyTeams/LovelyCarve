<template>
  <div class="sqlite-viewer">
    <div v-if="loading" class="state-row mono"><Loader2 :size="14" class="spin" /> {{ tr("正在解析 SQLite 数据库…") }}</div>
    <div v-else-if="error" class="state-row mono error">{{ tr("SQLite 解析失败：") }}{{ error }}</div>

    <!-- 加密库：密钥输入表单 -->
    <div v-else-if="locked" class="lock-wrap">
      <form class="lock-card" @submit.prevent="submitKey">
        <div class="lock-head">
          <Lock :size="15" />
          <span>{{ tr("数据库已加密（SQLCipher）") }}</span>
        </div>
        <p class="lock-hint mono">{{ tr("输入密钥以解密预览。多数 App 用文本口令；部分 IM 用 32 字节 raw key（hex）。旧版/微信库见下方高级参数。") }}</p>

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

        <details v-disclosure-motion class="lock-adv" :open="showAdvanced">
          <summary class="mono">{{ tr("高级参数（旧版库 / 微信）") }}</summary>
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

        <div v-if="keyError" class="lock-err">{{ keyError }}</div>

        <button type="submit" class="ui-btn primary lock-submit" :disabled="!canSubmit || unlocking">
          <Loader2 v-if="unlocking" :size="14" class="spin" />
          <KeyRound v-else :size="14" />
          {{ unlocking ? tr("解密中…") : tr("解密") }}
        </button>
      </form>
    </div>

    <template v-else-if="digest">
      <!-- 左：表清单，末尾是「已删除记录」 -->
      <aside class="table-list">
        <button
          v-for="t in digest.tables"
          :key="t.name"
          class="table-item"
          :class="{ active: !showDeleted && t.name === activeName }"
          :title="t.name"
          @click="selectTable(t.name)"
        >
          <Table2 v-if="t.kind === 'table'" :size="13" class="table-icon" />
          <Eye v-else :size="13" class="table-icon" />
          <span class="table-name mono">{{ t.name }}</span>
          <span class="row-pill mono">{{ t.error ? '!' : t.row_count ?? '—' }}</span>
        </button>
        <div v-if="!digest.tables.length" class="list-empty">{{ tr("没有用户表") }}</div>
        <div class="list-sep" aria-hidden="true" />
        <button
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
        <div v-if="digest.encrypted" class="decrypted-bar"><LockOpen :size="12" /> {{ tr("已解密（SQLCipher）") }}</div>
        <div v-if="digest.sidecars.length" class="wal-bar" :title="tr('预览是 SQLite 回放 WAL 中已提交事务后的状态，比主库文件更新')">
          <History :size="12" /> {{ tr("已回放") }} {{ digest.sidecars.join('、') }}
        </div>
        <SqliteDeletedRecords
          v-if="deletedOpened"
          v-show="showDeleted"
          :image-id="imageId"
          :node-id="nodeId"
          :parent-id="parentId ?? null"
          :sqlite-key="activeKey"
          :tables="digest.tables"
          @counted="deletedCount = $event"
        />
        <template v-if="!showDeleted">
          <div v-if="!digest.tables.length" class="state-row mono">{{ tr("库内没有用户表。已删除的表可能留有数据，可在左侧打开「已删除记录」查找。") }}</div>
          <div v-else-if="!activeName" class="state-row mono">{{ tr("在左侧选择一张表。") }}</div>
          <div v-else-if="rowsLoading && !rows.length" class="state-row mono"><Loader2 :size="14" class="spin" /> {{ tr("加载中…") }}</div>
          <div v-else-if="rowsError" class="state-row mono error">{{ rowsError }}</div>
          <template v-else>
            <div class="grid-scroll">
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
                    <td v-for="(cell, ci) in row" :key="ci" class="mono" :class="sqliteCellClass(cell)">{{ sqliteCellText(cell) }}</td>
                  </tr>
                  <tr v-if="!rows.length">
                    <td class="mono dim empty-cell" :colspan="columns.length + 1">{{ tr("（无数据）") }}</td>
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
        </template>
      </div>
    </template>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../../composables/useLocale';

import { vDisclosureMotion } from '../../../../composables/uiMotionDirectives';
// SQLite 库的「原类型视图」：左侧表清单（名称 + 行数），右侧按需分页取
// 所选表的行（点列头排序，底部翻页）。加密库（SQLCipher）先弹密钥表单，
// 解密后同样浏览；后端复用预览 VFS 会话，翻页/排序不重新读取整库。表清单
// 末尾的「已删除记录」第一次打开时才扫描，之后在切换表时保留结果。
import { computed, nextTick, ref, watch } from 'vue';
import {
  ArchiveRestore, ChevronDown, ChevronLeft, ChevronRight, ChevronUp,
  Eye, History, KeyRound, Loader2, Lock, LockOpen, Table2,
} from 'lucide-vue-next';
import { fsSqlitePreview, fsSqliteTableRows, SQLITE_LOCKED, type SqliteCell, type SqliteColumn, type SqliteDigest, type SqliteKey } from '../../../../lib/ipc/sqlite';
import { sqliteCellClass, sqliteCellText } from '../../model/sqliteCell';
import SqliteDeletedRecords from './SqliteDeletedRecords.vue';
import UiSelect, { type UiSelectOption } from '../../../../components/ui/UiSelect.vue';

const props = defineProps<{
  imageId: string;
  nodeId: string;
  /** 库所在目录的节点，用来查找同目录的 -wal / -journal。 */
  parentId?: string | null;
}>();

const PAGE_SIZE = 100;

const loading = ref(false);
const error = ref<string | null>(null);
const digest = ref<SqliteDigest | null>(null);
const activeName = ref<string | null>(null);

// 解密表单状态。
const locked = ref(false);
const unlocking = ref(false);
const keyError = ref<string | null>(null);
const keyMode = ref<'passphrase' | 'raw'>('passphrase');
const passphrase = ref('');
const rawHexKey = ref('');
const compatibility = ref<number | ''>('');
const COMPATIBILITY_OPTIONS: UiSelectOption<number | ''>[] = [
  { value: '', get label() { return tr("默认 (4)"); } },
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
// 单调请求序号：快速切表/翻页时丢弃过期响应，避免乱序覆盖。
let reqSeq = 0;

// 「已删除记录」：打开过一次后保持挂载，切回表格只是隐藏。
const showDeleted = ref(false);
const deletedOpened = ref(false);
const deletedCount = ref<number | null>(null);

const activeMeta = computed(() => digest.value?.tables.find((t) => t.name === activeName.value) ?? null);
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
  showDeleted.value = true;
  deletedOpened.value = true;
}

function selectTable(name: string) {
  showDeleted.value = false;
  if (name === activeName.value) return;
  activeName.value = name;
  orderBy.value = null;
  descending.value = false;
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
  const meta = activeMeta.value;
  // digest 已标记该表解析失败（坏页/虚表）——直接展示错误，不再取数。
  if (meta?.error) {
    rowsError.value = meta.error;
    columns.value = meta.columns;
    rows.value = [];
    total.value = 0;
    offset.value = 0;
    return;
  }
  const my = ++reqSeq;
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
  try {
    const d = await fsSqlitePreview({
      imageId: props.imageId,
      nodeId: props.nodeId,
      parentId: props.parentId ?? null,
      key,
    });
    digest.value = d;
    activeKey.value = key ?? null;
    const first = d.tables[0]?.name ?? null;
    if (first) {
      activeName.value = first;
      loadRows(0);
    }
  } catch (err) {
    const msg = String(err);
    if (msg.includes(SQLITE_LOCKED)) {
      // 加密库：弹密钥表单。带 key 仍失败 = 密钥/参数不对。
      locked.value = true;
      keyError.value = key ? tr("密钥或兼容参数不正确，请检查后重试。") : null;
      nextTick(() => firstInput.value?.focus());
    } else {
      error.value = msg;
    }
  } finally {
    loading.value = false;
    unlocking.value = false;
  }
}

// 切换文件：重置表单与密钥，重新尝试（不带 key，触发加密检测）。
watch(
  () => [props.imageId, props.nodeId],
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
.lock-hint {
  margin: 0;
  font-size: 12px;
  color: var(--text-secondary);
  line-height: 1.55;
}

.seg { align-self: flex-start; }

.lock-adv { font-size: 12px; }
.lock-adv summary {
  cursor: pointer;
  color: var(--text-secondary);
  font-size: 12px;
  padding: 2px 0;
  list-style: none;
}
.lock-adv summary:hover { color: var(--primary-color); }
.lock-adv summary::-webkit-details-marker { display: none; }
.lock-adv summary::before {
  content: '▸';
  display: inline-block;
  margin-right: 4px;
  transition: transform var(--transition-fast);
}
.lock-adv[open] summary::before { transform: rotate(90deg); }
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

.lock-err {
  font-size: 12px;
  color: var(--sev-critical-fg);
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
.decrypted-bar,
.wal-bar {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 4px 12px;
  font-size: 11px;
  border-bottom: 1px solid var(--border-color);
}
.decrypted-bar {
  color: var(--sev-success-fg);
  background: var(--sev-success-bg);
}
.wal-bar {
  color: var(--sev-info-fg);
  background: var(--sev-info-bg);
}

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
