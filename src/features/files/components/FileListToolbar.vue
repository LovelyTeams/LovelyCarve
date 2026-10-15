<template>
<div class="path-bar">
  <button class="ui-icon-btn" :class="{ active: treeVisible }" :aria-label="tr('切换目录树')" :title="tr('显示 / 隐藏目录树')" @click="treeVisible = !treeVisible"><PanelLeft :size="16" /></button>
  <button
    class="ui-icon-btn"
    :aria-label="tr('返回上一级')"
    :disabled="stack.length <= 1 || loading"
    @click="emit('goUp')"
    :title="tr('返回上一级')"
  ><ArrowLeft :size="15" /></button>
  <div class="breadcrumb mono" :class="{ deepsearch }">
    <template v-for="(frame, i) in stack" :key="`${i}-${frame.id}`">
      <ChevronRight v-if="i > 0" :size="12" class="crumb-sep" aria-hidden="true" />
      <button
        class="crumb"
        :class="{ active: i === stack.length - 1, root: i === 0 }"
        :disabled="loading"
        @click="i === stack.length - 1 ? emit('copyPath') : emit('goTo', i)"
        :title="i === stack.length - 1
          ? tr('点击复制完整路径：{0}', { 0: currentDirectoryPath })
          : tr('转到 {0}', { 0: frame.name })"
      >
        <HardDrive v-if="i === 0" :size="13" class="crumb-root-icon" />
        <span v-if="i > 0 || frame.name !== '/'">{{ frame.name }}</span>
        <span v-else class="sr-only">{{ frame.name }}</span>
        <Copy v-if="i === stack.length - 1" :size="11" class="crumb-copy-icon" />
      </button>
    </template>
    <Layers3
      v-if="deepsearch"
      :size="12"
      class="crumb-flat-icon"
      :title="tr('文件全显：当前节点下所有文件已递归平铺')"
    />
  </div>
  <label
    class="ui-search flat-search"
    :class="{ active: deepsearch }"
    :title="tr('搜索当前目录及子目录中的文件名或路径')"
  >
    <Search :size="14" />
    <input
      class="flat-search-input"
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
  <div class="path-bar-actions">
    <button class="ui-btn" :class="{ active: selectedCategoryCount > 0 || categoryFilterOpen }"
      :aria-expanded="categoryFilterOpen" :disabled="loading && selectedCategoryCount === 0"
      :title="tr('按文件类型筛选')" @click="categoryFilterOpen = !categoryFilterOpen">
      <Filter :size="14" /><span>{{ tr("分类") }}{{ selectedCategoryCount ? ` · ${selectedCategoryCount}` : '' }}</span>
    </button>
    <button
      class="ui-btn"
      :class="{ active: deepsearch, 'flat-active': deepsearch }"
      :aria-pressed="deepsearch"
      @click="emit('toggleDeepsearch')"
      :title="deepsearch
        ? tr('文件全显（已开启）：点击退出，恢复逐级浏览')
        : tr('文件全显：以当前节点为根递归平铺全部文件，按需分页加载')"
    >
      <component :is="deepsearch ? Layers3 : Layers" :size="13" /><span>{{ tr("子目录") }}</span>
    </button>
    <div class="toolbar-utilities">
      <button class="ui-icon-btn" :disabled="loading" :title="tr('刷新当前目录')" :aria-label="tr('刷新当前目录')" @click="emit('refresh')">
        <RefreshCw :size="15" :class="{ spin: loading }" />
      </button>
      <button class="ui-icon-btn" :class="{ active: !settingsCollapsed }" :aria-expanded="!settingsCollapsed"
        :title="tr('显示列')" :aria-label="tr('显示列')" @click="settingsCollapsed = !settingsCollapsed">
        <Columns3 :size="15" />
      </button>
      <button class="ui-icon-btn" :class="{ active: dockedPreviewVisible }" :aria-pressed="dockedPreviewVisible"
        :title="previewDetached ? tr('显示独立预览') : tr('右侧预览')" :aria-label="previewDetached ? tr('显示独立预览') : tr('右侧预览')" @click="emit('togglePreview')">
        <PanelRightOpen :size="15" />
      </button>
    </div>
  </div>
</div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { PanelLeft, ArrowLeft, ChevronRight, HardDrive, Copy, Layers3, Search, X, Filter, Layers, RefreshCw, Columns3, PanelRightOpen } from 'lucide-vue-next';
import type { FileNavigationFrame } from '../model/navigation';

defineProps<{
  stack: FileNavigationFrame[]; currentDirectoryPath: string; loading: boolean;
  deepsearch: boolean; selectedCategoryCount: number; dockedPreviewVisible: boolean; previewDetached: boolean;
}>();
const treeVisible = defineModel<boolean>('treeVisible', { required: true });
const categoryFilterOpen = defineModel<boolean>('categoryFilterOpen', { required: true });
const settingsCollapsed = defineModel<boolean>('settingsCollapsed', { required: true });
const searchKeyword = defineModel<string>('searchKeyword', { required: true });
const emit = defineEmits<{
  goUp: []; goTo: [index: number]; copyPath: []; searchInput: []; clearSearch: [];
  toggleDeepsearch: []; refresh: []; togglePreview: [];
}>();
</script>

<style scoped>
/* ── Address bar ──
   一行承载导航与检索：目录树开关、返回、地址栏式面包屑、搜索框、分类 / 子目录
   开关与刷新 / 列 / 预览。窄窗时搜索与按钮组整体折到第二行。 */
.path-bar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  min-height: 48px;
  padding: 8px 12px;
  flex-shrink: 0;
  border-bottom: 1px solid var(--border-color);
  background: var(--surface-secondary);
}

.breadcrumb {
  flex: 1 1 180px;
  min-width: 0;
  height: 30px;
  display: flex;
  align-items: center;
  gap: 0;
  padding: 0 4px;
  overflow: hidden;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  background: var(--surface-primary);
  color: var(--text-secondary);
  font-size: 12px;
  white-space: nowrap;
  transition: border-color var(--transition-fast), box-shadow var(--transition-fast);
}

.breadcrumb:hover {
  border-color: color-mix(in srgb, var(--text-secondary) 40%, var(--border-color));
}

.crumb {
  min-width: 28px;
  max-width: 180px;
  height: 24px;
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 0 6px;
  border: none;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-secondary);
  font-family: var(--font-mono);
  font-size: 12px;
  white-space: nowrap;
  cursor: pointer;
  transition: background var(--transition-fast), color var(--transition-fast);
}

.crumb:hover:not(:disabled) {
  background: var(--hover-bg);
  color: var(--text-primary);
}

.crumb span { overflow: hidden; text-overflow: ellipsis; }
.crumb.root { flex-shrink: 0; min-width: 0; }
.crumb-root-icon { flex-shrink: 0; color: var(--primary-color); }

.crumb.active {
  max-width: none;
  flex-shrink: 1;
  color: var(--text-primary);
  font-weight: 600;
  cursor: copy;
}

.crumb:disabled {
  cursor: default;
}

.crumb-copy-icon {
  flex-shrink: 0;
  color: var(--text-secondary);
  opacity: 0;
  transition: opacity var(--transition-fast), color var(--transition-fast);
}

.crumb.active:hover .crumb-copy-icon,
.crumb.active:focus-visible .crumb-copy-icon {
  color: var(--primary-color);
  opacity: 1;
}

/* 全显中：地址栏描上全显色、路径斜体，节点后缀全显小图标。 */
.breadcrumb.deepsearch {
  border-color: color-mix(in srgb, var(--showall-accent) 55%, var(--border-color));
}

.breadcrumb.deepsearch .crumb span {
  font-style: italic;
}

.crumb-flat-icon {
  flex-shrink: 0;
  margin: 0 4px 0 2px;
  color: var(--showall-accent);
}

.crumb-sep {
  flex-shrink: 0;
  color: var(--text-secondary);
  opacity: 0.55;
}

.path-bar-actions {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
}

.path-bar-actions > .ui-btn { flex-shrink: 0; }
.toolbar-utilities { display: flex; gap: 2px; flex-shrink: 0; margin-left: 2px; padding-left: 8px; border-left: 1px solid var(--border-color); }

/* 搜索框：输入关键词即自动进入全显。 */
.flat-search {
  flex: 0 1 220px;
  min-width: 150px;
  padding-right: 3px;
}

.flat-search.active {
  border-color: var(--showall-accent);
  color: var(--showall-accent);
}

.flat-search-input {
  min-width: 100px;
}

/* 文件全显开启态：与面包屑标记同一色。 */
.ui-btn.flat-active {
  border-color: color-mix(in srgb, var(--showall-accent) 60%, var(--border-color));
  background: color-mix(in srgb, var(--showall-accent) 10%, var(--surface-secondary));
  color: var(--showall-accent);
}

</style>
