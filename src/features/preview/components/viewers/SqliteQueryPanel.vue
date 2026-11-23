<template>
  <section class="query-pane" :aria-label="tr('SQL 查询')" :aria-busy="busy">
    <form class="query-form" @submit.prevent="run">
      <label class="ui-field">
        <span class="ui-field-label">{{ tr("只读 SQL") }}</span>
        <textarea
          v-model="sql"
          class="ui-textarea mono"
          rows="4"
          spellcheck="false"
          autocomplete="off"
          :disabled="busy"
        />
      </label>
      <label class="ui-field">
        <span class="ui-field-label">{{ tr("绑定参数（JSON 数组）") }}</span>
        <input
          ref="parameterInput"
          v-model="parameterText"
          class="ui-input sm mono"
          placeholder="[]"
          spellcheck="false"
          autocomplete="off"
          :disabled="busy"
          :aria-invalid="!!parameterError || undefined"
          @input="parameterError = null"
        />
      </label>
      <div v-if="parameterError" class="ui-field-error" role="alert">{{ parameterError }}</div>
      <div class="query-actions">
        <button type="submit" class="ui-btn sm primary" :disabled="busy || !sql.trim()">
          <Loader2 v-if="busy" :size="14" class="spin" />
          <Play v-else :size="14" />
          {{ tr("运行查询") }}
        </button>
      </div>
    </form>
    <div v-if="error" class="ui-alert danger query-feedback" role="alert">{{ error }}</div>
    <template v-if="page">
      <div v-if="page.sidecars.length" class="ui-alert info query-feedback">
        <History :size="12" /> {{ tr("已回放") }} {{ joinList(page.sidecars, tr) }}
      </div>
      <div v-if="page.shortenedCells" class="ui-alert warn query-feedback">
        {{ tr("部分文本已缩短或包含替换字符") }}
      </div>
      <div class="grid-scroll">
        <table class="data-table query-table">
          <thead>
            <tr>
              <th class="rownum-th mono" scope="col">#</th>
              <th v-for="(column, index) in page.columns" :key="index" class="mono" scope="col">{{ column }}</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="(row, index) in page.rows" :key="index">
              <td class="rownum mono">{{ page.offset + index + 1 }}</td>
              <td v-for="(cell, column) in row" :key="column" class="mono" :class="sqliteCellClass(cell)">
                {{ sqliteCellText(cell) }}
              </td>
            </tr>
            <tr v-if="!page.rows.length">
              <td :colspan="page.columns.length + 1">{{ tr("（无数据）") }}</td>
            </tr>
          </tbody>
        </table>
      </div>
      <PreviewChromeSlot area="statusEnd" :active="active">
        <span class="ui-footbar-stat pager-info" role="status">
          <strong>{{ (page.rows.length ? page.offset + 1 : 0).toLocaleString() }}–{{ (page.offset + page.rows.length).toLocaleString() }}</strong>
        </span>
        <div class="pager">
          <button type="button" class="ui-icon-btn sm" :title="tr('上一页')" :aria-label="tr('上一页')"
            :disabled="busy || page.offset === 0" @click="load(Math.max(0, page.offset - PAGE_SIZE))">
            <ChevronLeft :size="14" />
          </button>
          <button type="button" class="ui-icon-btn sm" :title="tr('下一页')" :aria-label="tr('下一页')"
            :disabled="busy || page.nextOffset === null" @click="load(page.nextOffset)">
            <ChevronRight :size="14" />
          </button>
        </div>
      </PreviewChromeSlot>
    </template>
  </section>
</template>

<script setup lang="ts">
import { ref, shallowRef } from 'vue';
import { ChevronLeft, ChevronRight, History, Loader2, Play } from 'lucide-vue-next';
import { tr } from '../../../../composables/useLocale';
import { joinList } from '../../../../i18n/translate';
import { fsSqliteQuery, type SqliteKey, type SqliteQueryPage, type SqliteQueryParameter } from '../../../../lib/ipc/sqlite';
import { sqliteCellClass, sqliteCellText } from '../../model/sqliteCell';
import PreviewChromeSlot from '../PreviewChromeSlot.vue';
import type { ImageId, NodeId } from '../../../../lib/ipc/ids';

const props = withDefaults(defineProps<{
  imageId: ImageId;
  nodeId: NodeId;
  parentId: NodeId | null;
  sqliteKey: SqliteKey | null;
  initialTable: string | null;
  /** 本视图正在显示：只有这时它的翻页才放进预览的状态栏。 */
  active?: boolean;
}>(), { active: true });

const PAGE_SIZE = 50;
const sql = ref(props.initialTable
  ? `SELECT * FROM "${props.initialTable.replaceAll('"', '""')}" LIMIT 50`
  : 'SELECT name, type FROM sqlite_schema ORDER BY name');
const parameterText = ref('');
const parameterInput = ref<HTMLInputElement>();
const parameterError = ref<string | null>(null);
const error = ref<string | null>(null);
const busy = ref(false);
const page = shallowRef<SqliteQueryPage | null>(null);
let submitted: { sql: string; parameters: SqliteQueryParameter[] } | null = null;

function isParameter(value: unknown): value is SqliteQueryParameter {
  return value === null || typeof value === 'string' || typeof value === 'boolean'
    || (typeof value === 'number' && Number.isFinite(value) && (!Number.isInteger(value) || Number.isSafeInteger(value)));
}

function run() {
  if (busy.value || !sql.value.trim()) return;
  parameterError.value = null;
  let parameters: unknown = null;
  try {
    parameters = JSON.parse(parameterText.value.trim() || '[]');
  } catch {
    // Invalid JSON and unsupported parameter types use the same field feedback.
  }
  if (!Array.isArray(parameters) || !parameters.every(isParameter)) {
    parameterError.value = tr("请输入由文本、布尔值、null 或可精确表示的数字组成的 JSON 数组。");
    parameterInput.value?.focus();
    return;
  }
  submitted = { sql: sql.value, parameters };
  page.value = null;
  void load(0);
}

async function load(offset: number | null) {
  if (offset === null || busy.value || !submitted) return;
  busy.value = true;
  error.value = null;
  try {
    page.value = await fsSqliteQuery({
      ...submitted,
      imageId: props.imageId,
      nodeId: props.nodeId,
      parentId: props.parentId,
      key: props.sqliteKey ?? undefined,
      offset,
      limit: PAGE_SIZE,
    });
  } catch (cause) {
    error.value = tr("SQL 查询失败：{0}", { 0: String(cause) });
  } finally {
    busy.value = false;
  }
}
</script>

<style scoped src="../../../../styles/db-browser.css"></style>
<style scoped>
.query-pane {
  display: flex;
  flex: 1;
  flex-direction: column;
  min-height: 0;
  overflow: auto;
}
.query-form {
  display: grid;
  flex: 0 0 auto;
  gap: 8px;
  padding: 12px;
  border-bottom: 1px solid var(--border-color);
}
.query-actions { display: flex; justify-content: flex-end; }
.query-feedback { margin: 8px 12px; overflow-wrap: anywhere; }
.query-pane .grid-scroll { min-height: 96px; }
.query-table { width: max-content; min-width: 100%; }
.query-table td { max-width: 360px; overflow-wrap: anywhere; white-space: pre-wrap; }
.query-table .cell-null { color: var(--text-secondary); opacity: 0.65; }
.query-table .cell-blob { color: var(--sev-info-fg); }
</style>
