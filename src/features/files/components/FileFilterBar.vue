<template>
<div class="ui-toolbar filter-bar">
  <div class="filter-group">
    <div class="ui-seg scope-seg" role="group" :aria-label="tr('列表范围')">
      <button class="ui-seg-item" type="button" :aria-pressed="!deepsearch" :title="tr('只列出当前目录中的条目')" @click="emit('scope', 'directory')">
        <Folder :size="13" aria-hidden="true" /><span class="compact-text">{{ tr("当前目录") }}</span>
      </button>
      <button class="ui-seg-item" type="button" :aria-pressed="deepsearch" :title="tr('列出当前目录及所有子目录中的文件，按需分页加载')" @click="emit('scope', 'recursive')">
        <FolderTree :size="13" aria-hidden="true" /><span class="compact-text">{{ tr("所有子目录") }}</span>
      </button>
    </div>
    <FileCategoryMenu
      :counts="categoryCounts"
      :counts-complete="countsComplete"
      :total="categoryTotal"
      :selected="selectedCategories"
      :disabled="loading && selectedCategories.size === 0"
      @toggle="emit('toggleCategory', $event)"
      @clear="emit('clearCategories')"
    />
    <template v-if="chips.length || hideEmptyDirs">
      <span class="ui-divider-v" aria-hidden="true"></span>
      <ul class="filter-chips" :aria-label="tr('生效的筛选条件')">
        <li v-for="chip in chips" :key="chip.key" class="filter-chip">
          <button
            class="chip-body"
            type="button"
            data-filter-anchor
            :title="tr('修改条件：{0} {1}', { 0: chip.label, 1: chip.value })"
            @click="emit('editFilter', $event, chip.key)"
          ><span class="chip-label">{{ chip.label }}</span><span class="chip-value">{{ chip.value }}</span></button>
          <button class="chip-remove" type="button" :aria-label="tr('移除条件「{0}」', { 0: chip.label })" :title="tr('移除条件「{0}」', { 0: chip.label })" @click="emit('removeFilter', chip.key)"><X :size="12" /></button>
        </li>
        <li v-if="hideEmptyDirs" class="filter-chip">
          <span class="chip-body static" :title="tr('不含任何文件的目录（包括只含空子目录的目录）已隐藏')"><span class="chip-label">{{ tr("已隐藏空目录") }}</span></span>
          <button class="chip-remove" type="button" :aria-label="tr('显示空目录')" :title="tr('显示空目录')" @click="emit('showEmptyDirs')"><X :size="12" /></button>
        </li>
      </ul>
      <button v-if="conditionCount > 1" class="ui-btn link sm clear-all" type="button" @click="emit('clearAll')">{{ tr("清除全部") }}</button>
    </template>
  </div>
  <div class="ui-toolbar-actions">
    <template v-if="selection.files || selection.dirs">
      <button v-if="selection.files" class="ui-btn" type="button" :disabled="indexOnly" :title="indexOnly ? tr('导出需恢复源镜像') : undefined" @click="emit('exportFile')">
        <FileDown :size="14" /><span class="compact-text">{{ tr("导出文件（{0}）", { 0: selection.files.toLocaleString() }) }}</span>
      </button>
      <button v-if="selection.dirs" class="ui-btn" type="button" :disabled="indexOnly" :title="indexOnly ? tr('导出需恢复源镜像') : undefined" @click="emit('exportDirectory')">
        <FolderDown :size="14" /><span class="compact-text">{{ tr("导出文件夹（{0}）", { 0: selection.dirs.toLocaleString() }) }}</span>
      </button>
    </template>
    <button v-else class="ui-btn" type="button" :disabled="indexOnly || !selection.cursorKind"
      :title="indexOnly ? tr('导出需恢复源镜像') : selection.cursorKind ? tr('导出「{0}」', { 0: selection.cursorName }) : tr('选中或勾选要导出的文件与文件夹')"
      @click="selection.cursorKind === 'dir' ? emit('exportDirectory') : emit('exportFile')">
      <Download :size="14" /><span class="compact-text">{{ tr("导出") }}</span>
    </button>
    <button class="ui-icon-btn" type="button" :aria-expanded="columnsOpen" :title="tr('显示列')" :aria-label="tr('显示列')" @click="columnsOpen = !columnsOpen">
      <Columns3 :size="16" />
    </button>
  </div>
</div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed } from 'vue';
import { Columns3, Download, FileDown, Folder, FolderDown, FolderTree, X } from 'lucide-vue-next';
import FileCategoryMenu from './FileCategoryMenu.vue';
import type { FilterChip } from '../model/filterChips';
import type { IconCategory } from '../../../utils/fileCategory';

const props = defineProps<{
  deepsearch: boolean;
  loading: boolean;
  indexOnly?: boolean;
  categoryCounts: Record<string, number>;
  countsComplete: boolean;
  categoryTotal: number;
  selectedCategories: Set<IconCategory>;
  chips: FilterChip[];
  hideEmptyDirs: boolean;
  /** Export targets: checked files and folders, or the cursor row when nothing is checked. */
  selection: { files: number; dirs: number; cursorKind: 'file' | 'dir' | null; cursorName: string };
}>();
const columnsOpen = defineModel<boolean>('columnsOpen', { required: true });
const emit = defineEmits<{
  scope: [mode: 'directory' | 'recursive'];
  toggleCategory: [key: IconCategory];
  clearCategories: [];
  editFilter: [event: MouseEvent, key: string];
  removeFilter: [key: string];
  showEmptyDirs: [];
  clearAll: [];
  exportFile: [];
  exportDirectory: [];
}>();

const conditionCount = computed(() => props.chips.length + (props.hideEmptyDirs ? 1 : 0) + (props.selectedCategories.size ? 1 : 0));
</script>

<style scoped>
/* ── 筛选条 ──
   表格上方的第二条工具栏：左侧是列表范围、文件类型与生效的列条件（每个条件一枚
   标签，点开修改、× 移除），右侧是导出与显示列。与目录树、预览栏的标题条同高，
   三栏的分隔线连成一线；条件多时左侧换行，整条随之增高。 */
.filter-bar {
  flex-wrap: nowrap;
  gap: 12px;
  container: filter-bar / inline-size;
}

.filter-group {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px;
  flex: 1;
  min-width: 0;
}

.scope-seg .ui-seg-item {
  gap: 6px;
}

.filter-chips {
  display: contents;
  margin: 0;
  padding: 0;
  list-style: none;
}

.filter-chip {
  display: inline-flex;
  align-items: stretch;
  max-width: 320px;
  height: 26px;
  border: 1px solid color-mix(in srgb, var(--primary-color) 35%, var(--border-color));
  border-radius: var(--radius-sm);
  background: var(--sidebar-active-bg);
  overflow: hidden;
}

:root[data-motion='full'] .filter-chip {
  animation: ui-pop-in var(--motion-fast) var(--ease-out) backwards;
}

.chip-body {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  padding: 0 4px 0 8px;
  border: 0;
  background: transparent;
  color: var(--text-primary);
  font-family: var(--font-ui);
  font-size: 12px;
  white-space: nowrap;
  cursor: pointer;
  transition: background var(--transition-fast);
}

button.chip-body:hover {
  background: color-mix(in srgb, var(--primary-color) 8%, transparent);
}

.chip-body:focus-visible,
.chip-remove:focus-visible {
  outline: 2px solid var(--primary-color);
  outline-offset: -2px;
}

.chip-body.static {
  padding-right: 2px;
  cursor: default;
}

.chip-label {
  flex-shrink: 0;
  color: var(--text-secondary);
}

.chip-body.static .chip-label {
  color: var(--text-primary);
}

.chip-value {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  font-family: var(--font-mono);
}

.chip-remove {
  display: grid;
  place-items: center;
  flex-shrink: 0;
  width: 24px;
  padding: 0;
  border: 0;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  transition: background var(--transition-fast), color var(--transition-fast);
}

.chip-remove:hover {
  background: color-mix(in srgb, var(--primary-color) 12%, transparent);
  color: var(--text-primary);
}

.clear-all {
  flex-shrink: 0;
}

.filter-bar .ui-toolbar-actions {
  flex-wrap: nowrap;
  align-self: flex-start;
}

/* 窄面板：范围与导出只留图标（文字仍供读屏），宽度让给条件标签。 */
@container filter-bar (max-width: 520px) {
  .compact-text {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }
  .scope-seg .ui-seg-item,
  .filter-bar .ui-toolbar-actions .ui-btn {
    padding: 0 8px;
  }
}
</style>
