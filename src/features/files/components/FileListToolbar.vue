<template>
<div class="ui-toolbar location-bar">
  <button class="ui-icon-btn" :aria-pressed="treeVisible" :aria-label="tr('切换目录树')" :title="tr('显示 / 隐藏目录树')" @click="treeVisible = !treeVisible"><PanelLeft :size="16" /></button>
  <button
    class="ui-icon-btn"
    :aria-label="tr('返回上一级')"
    :disabled="stack.length <= 1 || loading"
    :title="tr('返回上一级 (Backspace)')"
    @click="emit('goUp')"
  ><ArrowUp :size="16" /></button>
  <nav ref="trail" class="breadcrumb" :class="{ clipped }" :aria-label="tr('当前目录')" @scroll.passive="updateClipped">
    <template v-for="(frame, i) in stack" :key="`${i}-${frame.id}`">
      <ChevronRight v-if="i > 0" :size="14" class="crumb-sep" aria-hidden="true" />
      <button
        class="crumb"
        :class="{ current: i === stack.length - 1, root: i === 0 }"
        :aria-current="i === stack.length - 1 ? 'location' : undefined"
        :disabled="loading && i !== stack.length - 1"
        :title="i === stack.length - 1
          ? tr('点击复制完整路径：{0}', { 0: currentDirectoryPath })
          : tr('转到 {0}', { 0: frame.name })"
        @click="i === stack.length - 1 ? emit('copyPath') : emit('goTo', i)"
      >
        <HardDrive v-if="i === 0" :size="14" class="crumb-root-icon" aria-hidden="true" />
        <span v-if="i > 0 || frame.name !== '/'" class="crumb-label">{{ frame.name }}</span>
        <span v-else-if="stack.length === 1" class="crumb-label">{{ tr("根目录") }}</span>
        <span v-else class="sr-only">{{ frame.name }}</span>
        <Copy v-if="i === stack.length - 1" :size="12" class="crumb-copy" aria-hidden="true" />
      </button>
    </template>
  </nav>
  <div class="ui-toolbar-actions">
    <button class="ui-icon-btn" :disabled="loading" :title="tr('刷新当前目录')" :aria-label="tr('刷新当前目录')" @click="emit('refresh')">
      <RefreshCw :size="16" :class="{ spin: loading }" />
    </button>
    <label class="ui-search location-search" :class="{ applied: searchKeyword.trim() !== '' }" :title="tr('搜索当前目录及子目录中的文件名或路径')">
      <Search :size="14" />
      <input
        type="text"
        :placeholder="tr('搜索文件名或路径…')"
        :aria-label="tr('搜索文件名或路径')"
        v-model="searchKeyword"
        @input="emit('searchInput')"
        @keydown.esc.stop="emit('clearSearch')"
      />
      <button v-if="searchKeyword" class="ui-icon-btn sm" :title="tr('清空关键词')" :aria-label="tr('清空关键词')" @click="emit('clearSearch')">
        <X :size="13" />
      </button>
    </label>
    <span class="ui-divider-v" aria-hidden="true"></span>
    <button class="ui-icon-btn" :aria-pressed="dockedPreviewVisible"
      :title="previewDetached ? tr('显示独立预览') : tr('右侧预览')" :aria-label="previewDetached ? tr('显示独立预览') : tr('右侧预览')" @click="emit('togglePreview')">
      <PanelRight :size="16" />
    </button>
  </div>
</div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { PanelLeft, PanelRight, ArrowUp, ChevronRight, HardDrive, Copy, Search, X, RefreshCw } from 'lucide-vue-next';
import type { FileNavigationFrame } from '../model/navigation';

const props = defineProps<{
  stack: FileNavigationFrame[]; currentDirectoryPath: string; loading: boolean;
  dockedPreviewVisible: boolean; previewDetached: boolean;
}>();
const treeVisible = defineModel<boolean>('treeVisible', { required: true });
const searchKeyword = defineModel<string>('searchKeyword', { required: true });
const emit = defineEmits<{
  goUp: []; goTo: [index: number]; copyPath: []; searchInput: []; clearSearch: [];
  refresh: []; togglePreview: [];
}>();

// 路径放不下时整体横向滚动到末端：当前目录与最近的上级保持完整可读，较早的
// 各级移出左缘并淡出，可横向滚动查看。
const trail = ref<HTMLElement | null>(null);
const clipped = ref(false);
let resizeObserver: ResizeObserver | null = null;
function updateClipped() {
  clipped.value = (trail.value?.scrollLeft ?? 0) > 0;
}
function revealCurrent() {
  if (trail.value) trail.value.scrollLeft = trail.value.scrollWidth;
  updateClipped();
}
watch(() => props.stack, () => { void nextTick(revealCurrent); }, { deep: true });
onMounted(() => {
  revealCurrent();
  if (typeof ResizeObserver !== 'undefined' && trail.value) {
    resizeObserver = new ResizeObserver(revealCurrent);
    resizeObserver.observe(trail.value);
  }
});
onBeforeUnmount(() => resizeObserver?.disconnect());
</script>

<style scoped>
/* ── 地址栏 ──
   第一行只回答「在哪里、找什么」：目录树开关、上一级、面包屑、刷新、搜索与预览开关。
   范围、类型与列条件放在表格上方的筛选条。窄窗时右侧组整体折到第二行。 */
.location-bar {
  gap: 4px;
}

.location-bar > .ui-icon-btn + .breadcrumb {
  margin-left: 4px;
}

.breadcrumb {
  flex: 1 1 240px;
  min-width: 120px;
  height: 30px;
  display: flex;
  align-items: center;
  gap: 2px;
  overflow-x: auto;
  overflow-y: hidden;
  scrollbar-width: none;
  white-space: nowrap;
}

.breadcrumb::-webkit-scrollbar {
  display: none;
}

/* 左缘淡出提示前面还有上级；遮罩只取透明度，渐变终点用任意不透明色。 */
.breadcrumb.clipped {
  mask-image: linear-gradient(to right, transparent 0, var(--text-primary) 40px);
}

.crumb {
  flex: 0 0 auto;
  max-width: 200px;
  height: 28px;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 0 8px;
  border: 0;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-secondary);
  font-family: var(--font-ui);
  font-size: 13px;
  white-space: nowrap;
  cursor: pointer;
  transition: background var(--transition-fast), color var(--transition-fast);
}

.crumb:hover:not(:disabled) {
  background: var(--hover-bg);
  color: var(--text-primary);
}

.crumb:focus-visible {
  outline: 2px solid var(--primary-color);
  outline-offset: -2px;
}

.crumb:disabled {
  cursor: default;
}

.crumb-label {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
}

.crumb.root {
  padding: 0 6px;
}

.crumb-root-icon {
  flex-shrink: 0;
  color: var(--primary-color);
}

/* 当前目录：超长名称才在地址栏宽度内省略，点击复制完整路径。 */
.crumb.current {
  max-width: 100%;
  color: var(--text-primary);
  font-weight: 600;
  cursor: copy;
}

.crumb-copy {
  flex-shrink: 0;
  color: var(--text-secondary);
  opacity: 0;
  transition: opacity var(--transition-fast), color var(--transition-fast);
}

.crumb.current:hover .crumb-copy,
.crumb.current:focus-visible .crumb-copy {
  color: var(--primary-color);
  opacity: 1;
}

.crumb-sep {
  flex-shrink: 0;
  color: var(--text-secondary);
  opacity: 0.6;
}

/* 搜索框：输入关键词即在当前目录及所有子目录中查找。 */
.location-search {
  flex: 0 1 280px;
  min-width: 168px;
  padding-right: 4px;
}

.location-search.applied {
  border-color: color-mix(in srgb, var(--primary-color) 55%, var(--border-color));
  background: color-mix(in srgb, var(--primary-color) 5%, var(--surface-secondary));
}

.location-search.applied > svg {
  color: var(--primary-color);
  opacity: 1;
}

.location-bar .ui-toolbar-actions {
  flex-wrap: nowrap;
  gap: 4px;
}

.location-bar .ui-divider-v {
  margin: 0 4px;
}
</style>
