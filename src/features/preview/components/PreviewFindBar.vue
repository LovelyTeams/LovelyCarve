<template>
  <!-- 查找栏：按查找开关或 Ctrl+F 打开，有查找词时一直显示（含拆分 / 停靠恢复的查找词）。
       Enter / Shift+Enter 逐项定位，Esc 清空并收起。 -->
  <div :id="id" class="preview-findbar" role="search" :aria-label="tr('查找')">
    <div class="ui-search find-field">
      <Search :size="13" aria-hidden="true" />
      <input
        ref="input"
        v-model="query"
        class="mono"
        type="text"
        spellcheck="false"
        autocomplete="off"
        :placeholder="pageOnly ? tr('搜索当前页') : tr('在内容中查找')"
        :aria-label="pageOnly ? tr('搜索当前页') : tr('搜索预览内容')"
        @keydown="onKeydown"
      />
      <span v-if="query" class="match-count mono" role="status">{{ countText }}</span>
    </div>
    <button type="button" class="ui-icon-btn sm" :title="tr('上一个 (Shift+Enter)')" :aria-label="tr('上一个匹配')" :disabled="matchCount === 0" @click="emit('goto', -1)">
      <ChevronUp :size="14" />
    </button>
    <button type="button" class="ui-icon-btn sm" :title="tr('下一个 (Enter)')" :aria-label="tr('下一个匹配')" :disabled="matchCount === 0" @click="emit('goto', 1)">
      <ChevronDown :size="14" />
    </button>
    <button type="button" class="ui-icon-btn sm" :title="tr('关闭查找 (Esc)')" :aria-label="tr('关闭查找')" @click="emit('close')">
      <X :size="14" />
    </button>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, ref } from 'vue';
import { ChevronDown, ChevronUp, Search, X } from 'lucide-vue-next';

const props = defineProps<{
  id: string;
  matchCount: number;
  currentMatch: number;
  /** SQLite tables search only the page on screen. */
  pageOnly: boolean;
}>();
const query = defineModel<string>('query', { required: true });
const emit = defineEmits<{ goto: [delta: number]; close: []; keydown: [event: KeyboardEvent] }>();

const input = ref<HTMLInputElement | null>(null);
defineExpose({ focus: () => { input.value?.focus(); input.value?.select(); } });

const countText = computed(() => {
  if (props.matchCount) return `${props.currentMatch >= 0 ? props.currentMatch + 1 : 0}/${props.matchCount}`;
  return props.pageOnly ? tr('本页无匹配') : tr('无匹配');
});

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape' && !event.isComposing) {
    // 文档级的 Esc 依次关闭面板与清空勾选，这里只收起查找。
    event.stopPropagation();
    event.preventDefault();
    emit('close');
    return;
  }
  emit('keydown', event);
}
</script>

<style scoped>
.preview-findbar {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
  padding: 6px 12px;
  border-bottom: 1px solid var(--border-color);
  background: var(--surface-secondary);
}

.find-field {
  flex: 1;
  min-width: 0;
  height: 28px;
  margin-right: 4px;
}

.find-field > input {
  font-family: var(--font-mono);
}

.match-count {
  flex-shrink: 0;
  padding-right: 4px;
  color: var(--text-secondary);
  font-size: 11px;
  white-space: nowrap;
}
</style>
