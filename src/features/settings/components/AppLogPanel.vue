<template>
  <section class="log-panel" :aria-label="tr('运行日志')">
    <div class="ui-toolbar log-toolbar">
      <label class="ui-search log-search">
        <Search :size="14" aria-hidden="true" />
        <input v-model="searchText" type="search" :aria-label="tr('搜索日志')" :placeholder="tr('搜索日志…')" spellcheck="false" />
      </label>
      <div class="ui-toolbar-actions">
        <button class="ui-btn sm" type="button" :disabled="!entries.length || copying" :title="tr('复制全部会话日志')" @click="copyLogs">
          <Loader2 v-if="copying" :size="14" class="spin" /><Copy v-else :size="14" aria-hidden="true" />{{ tr("复制全部") }}
        </button>
        <button class="ui-btn sm" type="button" :disabled="!entries.length || copying" @click="clearLogs"><Eraser :size="14" aria-hidden="true" />{{ tr("清空") }}</button>
      </div>
    </div>
    <div class="log-levels" role="group" :aria-label="tr('日志级别')">
      <button v-for="level in levelOptions" :key="level.key" class="ui-btn sm" type="button"
        :class="{ active: enabledLevels[level.key] }" :aria-pressed="enabledLevels[level.key]"
        @click="enabledLevels[level.key] = !enabledLevels[level.key]">
        {{ level.label }}<span class="ui-count">{{ levelCounts[level.key].toLocaleString() }}</span>
      </button>
    </div>
    <p v-if="feedback" class="ui-alert" :class="copyFailed ? 'danger' : 'success'" :role="copyFailed ? 'alert' : 'status'">{{ feedback }}</p>
    <div ref="bodyEl" class="log-body" tabindex="0" :aria-label="tr('日志内容')" @scroll="onScroll">
      <p v-if="!filteredEntries.length" class="log-empty" role="status">{{ entries.length ? tr("没有符合筛选条件的日志。") : tr("暂无日志，运行期间的输出会显示在这里。") }}</p>
      <div v-for="entry in filteredEntries" :key="entry.id" class="log-entry">
        <time class="log-time" :datetime="new Date(entry.timestamp).toISOString()" :title="formatEpochSeconds(entry.timestamp / 1000)">{{ formatTime(entry.timestamp) }}</time>
        <span class="ui-badge sm log-level" :class="levelOptions.find(level => level.key === entry.level)?.tone">{{ levelLabels[entry.level] }}</span>
        <span class="log-text">{{ entry.text }}</span>
      </div>
    </div>
    <div class="log-footer">
      <span>{{ tr("显示") }} {{ filteredEntries.length.toLocaleString() }} / {{ entries.length.toLocaleString() }} {{ tr("条 · 本地时间") }}</span>
      <label class="ui-check"><input v-model="autoScroll" type="checkbox" />{{ tr("自动滚动") }}</label>
    </div>
    <p class="ui-field-hint">{{ tr("仅保留当前会话最近 5,000 条输出。") }}</p>
  </section>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { Copy, Eraser, Loader2, Search } from 'lucide-vue-next';
import { useConsoleCapture, type ConsoleLevel } from '../../../composables/useConsoleCapture';
import { formatEpochSeconds } from '../../../utils/format';

const { entries, clear, copyAll } = useConsoleCapture();
const levelOptions: { key: ConsoleLevel; label: string; tone: string }[] = [
  { key: 'log', get label() { return tr("普通"); }, tone: '' },
  { key: 'info', get label() { return tr("信息"); }, tone: 'info' },
  { key: 'warn', get label() { return tr("警告"); }, tone: 'warn' },
  { key: 'error', get label() { return tr("错误"); }, tone: 'danger' },
  { key: 'debug', get label() { return tr("调试"); }, tone: '' },
];
const levelLabels = Object.fromEntries(levelOptions.map(level => [level.key, level.label])) as Record<ConsoleLevel, string>;
const enabledLevels = ref<Record<ConsoleLevel, boolean>>({ log: true, info: true, warn: true, error: true, debug: true });
const searchText = ref('');
const autoScroll = ref(true);
const bodyEl = ref<HTMLElement | null>(null);
const stuckToBottom = ref(true);
const copying = ref(false);
const feedback = ref('');
const copyFailed = ref(false);
let disposed = false;

const levelCounts = computed(() => {
  const counts: Record<ConsoleLevel, number> = { log: 0, info: 0, warn: 0, error: 0, debug: 0 };
  for (const entry of entries.value) counts[entry.level]++;
  return counts;
});
const filteredEntries = computed(() => {
  const query = searchText.value.trim().toLowerCase();
  return entries.value.filter(entry => enabledLevels.value[entry.level] && (!query || entry.text.toLowerCase().includes(query)));
});
function formatTime(timestamp: number) {
  const date = new Date(timestamp);
  const time = [date.getHours(), date.getMinutes(), date.getSeconds()].map(value => String(value).padStart(2, '0')).join(':');
  return `${time}.${String(date.getMilliseconds()).padStart(3, '0')}`;
}
function onScroll() {
  const element = bodyEl.value;
  if (element) stuckToBottom.value = element.scrollHeight - element.scrollTop - element.clientHeight < 24;
}
function scrollToBottom() {
  if (bodyEl.value) {
    bodyEl.value.scrollTop = bodyEl.value.scrollHeight;
    stuckToBottom.value = true;
  }
}
watch(filteredEntries, () => { if (autoScroll.value && stuckToBottom.value) scrollToBottom(); }, { flush: 'post' });
watch(autoScroll, enabled => { if (enabled) scrollToBottom(); });
onMounted(() => { void nextTick(scrollToBottom); });
onBeforeUnmount(() => { disposed = true; });

async function copyLogs() {
  if (copying.value) return;
  copying.value = true;
  feedback.value = '';
  try {
    await copyAll();
    if (!disposed) { copyFailed.value = false; feedback.value = tr("已复制全部日志。"); }
  } catch (cause) {
    if (!disposed) { copyFailed.value = true; feedback.value = tr("复制日志失败：{0}。请重试。", { 0: String(cause) }); }
  } finally {
    if (!disposed) copying.value = false;
  }
}
function clearLogs() { clear(); feedback.value = ''; }
</script>

<style scoped>
.log-panel { display: flex; flex-direction: column; gap: 12px; min-width: 0; }
.log-toolbar { flex-wrap: wrap; padding: 0; }
.log-search { flex: 1; min-width: 160px; }
.log-levels { display: flex; flex-wrap: wrap; gap: 4px; }
.log-body { height: clamp(160px, 32vh, 320px); overflow: auto; padding: 8px 12px; border: 1px solid var(--border-color); border-radius: var(--radius-md); background: var(--surface-primary); }
.log-body:focus-visible { outline: 2px solid var(--primary-color); outline-offset: 2px; }
.log-entry { display: grid; grid-template-columns: 88px 40px minmax(0, 1fr); align-items: start; gap: 8px; padding: 4px 0; font: 12px/1.5 var(--font-mono); }
.log-time { color: var(--text-secondary); font-size: 11px; }
.log-level { justify-content: center; justify-self: start; min-width: 40px; }
.log-text { white-space: pre-wrap; overflow-wrap: anywhere; }
.log-empty { color: var(--text-secondary); font-size: 12px; }
.log-footer { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 8px; color: var(--text-secondary); font-size: 11px; }
.log-panel > .ui-field-hint { margin: 0; }
@media (max-width: 480px) {
  .log-entry { grid-template-columns: 88px minmax(0, 1fr); }
  .log-text { grid-column: 1 / -1; }
}
</style>
