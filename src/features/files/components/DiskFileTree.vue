<template>
  <aside class="tree-rail">
    <div class="tree-header">
      <FolderTree :size="14" class="tree-header-icon" aria-hidden="true" />
      <span class="tree-header-label">{{ tr("目录树") }}</span>
      <button
        class="ui-icon-btn tree-header-toggle"
        :title="hideEmptyDirs ? tr('显示空目录') : tr('隐藏空目录（不含任何文件的目录，包括只含空子目录的目录）')"
        :aria-label="tr('隐藏空目录')"
        :aria-pressed="hideEmptyDirs"
        @click="hideEmptyDirs = !hideEmptyDirs"
      >
        <FolderX :size="16" />
      </button>
    </div>
    <div
      v-if="dockerFilesystemLoading"
      class="tree-docker-loading"
      role="status"
      aria-live="polite"
    >
      <div class="tree-docker-loading-copy">
        <Loader2 :size="12" class="spin" />
        <span>{{ tr("正在合并容器文件系统…") }}</span>
      </div>
      <div class="tree-docker-loading-track" aria-hidden="true"><span></span></div>
    </div>
    <div class="tree-panel">
      <div class="tree-list">
        <button
          v-for="row in visibleRows"
          :key="row.key"
          class="tree-row"
          :class="{ active: row.node.id === activeId || row.node.id === selectedLeafId, loading: row.node.loading }"
          :style="{ paddingLeft: 8 + row.depth * 14 + 'px' }"
          @click="onRowClick(row.node)"
        >
          <span class="tree-chevron-wrap" @click.stop="toggleExpand(row.node)">
            <ChevronRight
              v-if="row.node.kind === 'dir'"
              :size="12"
              class="tree-chevron"
              :class="{ open: row.node.expanded }"
            />
            <span v-else class="tree-chevron-spacer"></span>
          </span>
          <FileNodeIcon
            :id="row.node.id"
            :kind="row.node.kind"
            :name="row.node.name"
            :size="13"
            class="tree-node-icon"
          />
          <span
            v-if="row.depth === 0 && row.node.driveLetter"
            class="tree-drive-letter mono"
            :title="tr('源系统盘符 {0}', { 0: row.node.driveLetter })"
          >{{ row.node.driveLetter }}</span>
          <span class="tree-name" :title="row.node.name">{{ row.node.name }}</span>
          <span v-if="row.node.childCount != null && row.node.childCount > 0" class="tree-count">{{ row.node.childCount.toLocaleString() }}</span>
        </button>
        <div v-if="rootNodes.length === 0 && rootLoaded" class="tree-empty mono">
          {{ tr("没有可显示的目录") }}
        </div>
      </div>
    </div>
  </aside>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { ChevronRight, FolderTree, FolderX, Loader2 } from 'lucide-vue-next';
import FileNodeIcon from '../../../components/display/FileNodeIcon.vue';
import { useCaseContext } from '../../cases/api';
import { useEmptyDirFilter } from '../composables/useEmptyDirFilter';
import { useFileTree } from '../composables/useFileTree';
import type { TreeStackFrame } from '../model/fileTree';
const props = defineProps<{ imageId: string; pathStack: TreeStackFrame[] }>();
const emit = defineEmits<{ navigate: [stack: TreeStackFrame[]]; 'open-file': [stack: TreeStackFrame[], fileId: string] }>();
const { activeCase } = useCaseContext();
const { hideEmptyDirs } = useEmptyDirFilter();
const { activeId, selectedLeafId, visibleRows, rootNodes, rootLoaded, dockerFilesystemLoading, onRowClick, toggleExpand } = useFileTree({
  imageId: () => props.imageId, pathStack: () => props.pathStack, caseId: () => activeCase.value?.id ?? null, hideEmptyDirs,
  navigate: stack => emit('navigate', stack), openFile: (stack, fileId) => emit('open-file', stack, fileId),
});
</script>

<style scoped>
.tree-rail {
  width: 240px;
  display: flex;
  flex-direction: column;
  align-items: stretch;
  border-right: 1px solid var(--border-color);
  background: var(--surface-secondary);
}

/* 与中间的筛选条、右侧预览栏标题条同高（48px），三栏的分隔线连成一线。 */
.tree-header {
  display: flex;
  align-items: center;
  gap: 8px;
  box-sizing: border-box;
  height: 48px;
  padding: 0 10px 0 14px;
  border-bottom: 1px solid var(--border-color);
  background: var(--surface-primary);
  flex-shrink: 0;
}

.tree-header-icon {
  color: var(--text-secondary);
}

.tree-header-label {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
  user-select: none;
}

.tree-header-toggle {
  margin-left: auto;
}

.tree-docker-loading {
  flex-shrink: 0;
  padding: 7px 10px 8px;
  border-bottom: 1px solid var(--border-color);
  background: var(--surface-secondary);
}

.tree-docker-loading-copy {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--text-secondary);
  font-size: 11px;
}

.tree-docker-loading-copy svg {
  flex-shrink: 0;
  color: var(--primary-color);
}

.tree-docker-loading-track {
  position: relative;
  height: 3px;
  margin-top: 6px;
  overflow: hidden;
  border-radius: 999px;
  background: var(--primary-glow);
}

.tree-docker-loading-track span {
  position: absolute;
  inset: 0 auto 0 0;
  width: 42%;
  border-radius: inherit;
  background: var(--primary-color);
  animation: tree-docker-loading-slide 1.15s ease-in-out infinite;
}

@keyframes tree-docker-loading-slide {
  from { transform: translateX(-110%); }
  to { transform: translateX(250%); }
}

:root[data-motion='reduced'] .tree-docker-loading-track span {
  inset: 0;
  width: 100%;
  opacity: 0.45;
  animation: none;
}

.tree-panel {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
  overflow: hidden;
}

.tree-list {
  flex: 1;
  overflow-y: auto;
  padding: 4px;
}

.tree-row {
  display: flex;
  align-items: center;
  gap: 4px;
  width: 100%;
  min-height: 26px;
  padding: 2px 8px 2px 0;
  background: transparent;
  border: none;
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  cursor: pointer;
  text-align: left;
  font-family: var(--font-ui);
  font-size: 12px;
  white-space: nowrap;
  overflow: hidden;
  transition: background var(--transition-fast), color var(--transition-fast);
}

.tree-row:hover {
  background: var(--hover-bg);
}

.tree-row.active {
  background: var(--sidebar-active-bg);
  color: var(--primary-color);
  font-weight: 500;
}

.tree-row:focus-visible {
  outline: 2px solid var(--primary-color);
  outline-offset: -2px;
}

.tree-chevron-wrap {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 14px;
  height: 14px;
  flex-shrink: 0;
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
}

.tree-chevron-wrap:hover {
  background: var(--active-bg);
  color: var(--text-primary);
}

.tree-chevron {
  transition: transform 120ms ease-out;
}

.tree-chevron.open {
  transform: rotate(90deg);
}

.tree-chevron-spacer {
  display: inline-block;
  width: 12px;
}

.tree-node-icon {
  flex-shrink: 0;
}

.tree-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 子项数：右对齐的等宽数字，不再画成徽章，长树里读起来更安静。 */
.tree-count {
  flex-shrink: 0;
  margin-left: auto;
  padding-left: 8px;
  color: var(--text-secondary);
  font-family: var(--font-mono);
  font-size: 11px;
  font-variant-numeric: tabular-nums;
}

.tree-row.active .tree-count {
  color: var(--primary-color);
}

/* 源 Windows 系统的原始盘符徽标（前置主色调小标），仅分区根节点显示。 */
.tree-drive-letter {
  flex-shrink: 0;
  font-size: 11px;
  font-weight: 600;
  color: var(--primary-color);
  background: color-mix(in srgb, var(--primary-color) 14%, transparent);
  border-radius: var(--radius-sm);
  padding: 0 4px;
  margin-right: 2px;
}

.tree-row.loading {
  /* Subtle "in-flight" cue without a spinner — pulses the row's bg
   *  using the same 150ms transition family the rest of the UI uses. */
  background: color-mix(in srgb, var(--surface-tertiary) 60%, transparent);
  animation: tree-row-pulse 1.2s ease-in-out infinite;
}

@keyframes tree-row-pulse {
  0%, 100% {
    opacity: 0.65;
  }
  50% {
    opacity: 1;
  }
}

.tree-empty {
  padding: 16px;
  text-align: center;
  font-size: 11px;
  color: var(--text-secondary);
}
</style>
