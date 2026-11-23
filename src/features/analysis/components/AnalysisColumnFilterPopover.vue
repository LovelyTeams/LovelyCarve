<template>
<Teleport to="body">
  <UiTransition kind="pop"><div
    v-if="open"
    ref="root"
    class="ui-menu record-filter-popover"
    :style="{ left: `${x}px`, top: `${y}px`, width: `${width}px` }"
    role="dialog"
    :aria-label="tr('筛选「{0}」', { 0: label })"
  >
    <div class="record-filter-head">
      <span class="record-filter-title">{{ tr("筛选「{0}」", { 0: label }) }}</span>
      <button class="ui-icon-btn sm" type="button" :title="tr('关闭')" :aria-label="tr('关闭筛选')" @click="emit('close')"><X :size="14" /></button>
    </div>

    <template v-if="error">
      <p class="record-filter-error" role="alert">{{ tr("值读取失败：{0}", { 0: error }) }}</p>
      <div class="record-filter-foot">
        <button class="ui-btn sm" type="button" @click="emit('retry')">{{ tr("重试") }}</button>
      </div>
    </template>

    <template v-else>
      <label v-if="values && values.length > 8" class="ui-search record-filter-search">
        <Search :size="13" aria-hidden="true" />
        <input v-model="search" type="text" :placeholder="tr('搜索值…')" :aria-label="tr('搜索值')" spellcheck="false" />
      </label>
      <div class="record-filter-list" :aria-busy="loading">
        <label v-for="item in visibleValues" :key="rawOf(item.value)" class="ui-check record-filter-item" :title="valueText(item)">
          <input
            type="checkbox"
            :checked="selected.includes(rawOf(item.value))"
            :aria-label="valueText(item)"
            @change="emit('toggle', rawOf(item.value), ($event.target as HTMLInputElement).checked)"
          />
          <span class="record-filter-text">{{ valueText(item) }}</span>
          <span class="record-filter-count">{{ item.count.toLocaleString() }}</span>
        </label>
        <div v-if="loading" class="record-filter-state"><Loader2 :size="14" class="spin" /><span>{{ tr("正在读取…") }}</span></div>
        <p v-else-if="!values?.length" class="record-filter-state">{{ tr("没有可筛选的值") }}</p>
        <p v-else-if="!visibleValues.length" class="record-filter-state">{{ tr("没有匹配的值") }}</p>
      </div>
      <div class="record-filter-foot">
        <span class="record-filter-note">
          <template v-if="truncated">{{ tr("值过多，仅列出前 {0} 项", { 0: values?.length ?? 0 }) }}</template>
          <template v-else>{{ tr("{0} 条匹配", { 0: total.toLocaleString() }) }}</template>
        </span>
        <span class="record-filter-spacer"></span>
        <button class="ui-btn sm" type="button" :disabled="!filtered" @click="emit('clear')">{{ tr("清除") }}</button>
      </div>
    </template>
  </div></UiTransition>
</Teleport>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { Loader2, Search, X } from 'lucide-vue-next';
import UiTransition from '../../../components/ui/UiTransition.vue';
import type { AnalysisRecordValueCount, RecordColumn } from '../../../lib/ipc/analysisResults';
import { recordValueText } from '../model/analysisValue';

// 解析结果表格的列筛选气泡:列出该列的去重值与计数,勾选即服务端过滤。
// 值按记录单元格的同一口径显示(枚举取声明名、布尔取是/否);null/空串都
// 是「(空)」,发回后端的筛选值是它的字符串形式(null → 空串)。
const props = defineProps<{
  open: boolean;
  x: number;
  y: number;
  width: number;
  column: RecordColumn | null;
  label: string;
  /** 已勾选的原始筛选值(null/空 = 空单元格)。 */
  selected: string[];
  values: AnalysisRecordValueCount[] | null;
  /** 命中当前关键词与其它列筛选的记录数。 */
  total: number;
  loading: boolean;
  error: string | null;
  truncated: boolean;
  filtered: boolean;
  choiceLabel: (choice: string) => string;
}>();
const emit = defineEmits<{ close: []; toggle: [value: string, checked: boolean]; clear: []; retry: [] }>();

/** JSON 值 → 发回后端的筛选字符串(null → 空串)。 */
function rawOf(value: unknown): string {
  if (value === null || value === undefined) return '';
  if (typeof value === 'boolean') return value ? 'true' : 'false';
  return String(value);
}

function valueText(item: AnalysisRecordValueCount): string {
  const raw = rawOf(item.value);
  if (!raw) return tr("(空)");
  const value = props.column?.kind === 'bool' ? raw === 'true' : item.value;
  return recordValueText(value, props.column?.kind ?? null, { choice: props.choiceLabel, tr });
}

const search = ref('');
watch(() => props.open, open => {
  search.value = '';
  if (open) {
    document.addEventListener('pointerdown', onOutsidePointerDown, true);
    document.addEventListener('keydown', onEscape, true);
  } else {
    stopListeners();
  }
});
function stopListeners() {
  document.removeEventListener('pointerdown', onOutsidePointerDown, true);
  document.removeEventListener('keydown', onEscape, true);
}
onBeforeUnmount(stopListeners);
const root = ref<HTMLElement | null>(null);
function onOutsidePointerDown(event: PointerEvent) {
  if (root.value?.contains(event.target as Node)) return;
  emit('close');
}
function onEscape(event: KeyboardEvent) {
  if (event.key === 'Escape') emit('close');
}

const visibleValues = computed(() => {
  const list = props.values ?? [];
  const needle = search.value.trim().toLowerCase();
  if (!needle) return list;
  return list.filter(item => valueText(item).toLowerCase().includes(needle));
});
</script>

<style scoped>
/* ── 列筛选气泡(Teleport 到 body;外观来自 ui-menu)── */
.record-filter-popover {
  position: fixed;
  z-index: 9000;
  transform-origin: top center;
  padding: 10px 12px 12px;
  display: flex;
  flex-direction: column;
  gap: 8px;
  font-family: var(--font-ui);
  border-radius: var(--radius-lg);
}
.record-filter-head { display: flex; align-items: center; justify-content: space-between; margin-right: -4px; }
.record-filter-title { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 12px; font-weight: 600; color: var(--text-primary); }
.record-filter-list { max-height: 264px; overflow-y: auto; display: flex; flex-direction: column; gap: 1px; padding-right: 2px; }
.record-filter-item { display: flex; min-height: 26px; padding: 0 6px; border-radius: var(--radius-sm); cursor: pointer; }
.record-filter-item:hover { background: var(--hover-bg); }
.record-filter-text { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 12px; color: var(--text-primary); }
.record-filter-count { flex-shrink: 0; font-family: var(--font-mono); font-size: 11px; color: var(--text-secondary); font-variant-numeric: tabular-nums; }
.record-filter-state { display: flex; align-items: center; justify-content: center; gap: 6px; min-height: 48px; margin: 0; color: var(--text-secondary); font-size: 12px; }
.record-filter-error { margin: 0; color: var(--sev-critical-fg); font-size: 12px; overflow-wrap: anywhere; }
.record-filter-foot { display: flex; align-items: center; gap: 6px; }
.record-filter-note { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-secondary); font-size: 11px; }
.record-filter-spacer { flex: 1; }
</style>
