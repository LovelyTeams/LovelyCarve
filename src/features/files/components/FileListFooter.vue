<template>
<div class="list-footer ui-footbar">
  <span v-if="deepsearch" class="footer-stat mono">
    {{ tr("共") }} {{ flatTotal.toLocaleString() }} {{ tr("项") }}<template
      v-if="loadedCount < flatTotal"
    > {{ tr("· 已加载") }} {{ loadedCount.toLocaleString() }} {{ tr("项") }}</template>
  </span>
  <span v-else class="footer-stat mono">
    {{ tr("共") }} {{ directoryTotal.toLocaleString() }} {{ tr("项") }}<template v-if="directoryCursor"> {{ tr("· 已加载") }} {{ loadedCount.toLocaleString() }} {{ tr("项") }}</template><template
      v-if="displayedCount !== loadedCount"
    > {{ tr("· 显示") }} {{ displayedCount.toLocaleString() }} {{ tr("项") }}</template>
  </span>
  <button
    v-if="activeFilterCount > 0"
    class="snapshot-chip filter-chip"
    :title="tr('清除全部列过滤和分类筛选')"
    @click="emit('clearFilters')"
  >
    <Filter :size="12" />
    <span>{{ tr("清除筛选") }}</span>
    <X :size="12" />
  </button>
  <span v-if="deepsearch && flatTruncated" class="footer-note mono">{{ indexOnly ? tr("已有索引覆盖不完整，仅显示已索引文件") : tr("实时遍历达安全上限，结果不完整") }}</span>
  <span class="ui-footbar-spacer"></span>
  <span
    v-if="snapshotStatus"
    class="snapshot-chip"
    :class="`snap-${snapshotStatus.state}`"
    :title="snapshotStatus.state === 'building'
      ? tr('正在后台建立卷快照，目录加载将从实时遍历切换为快照')
      : snapshotStatus.state === 'stale'
        ? tr('源卷已变化，快照可能与当前镜像不一致，需重建')
        : tr('目录列表由卷快照直接返回，无需重新遍历文件系统')"
  >
    <Loader2 v-if="snapshotStatus.state === 'building'" :size="12" class="spin" />
    <Camera v-else :size="12" />
    <span>{{ snapshotStatus.label }}</span>
  </span>
  <div v-if="checkedStats.count || activeRow" class="selection-well" :class="{ checked: checkedStats.count > 0 }">
    <span class="selection-label">
      <template v-if="checkedStats.count">{{ tr("已勾选") }} <strong class="mono">{{ checkedStats.count.toLocaleString() }}</strong> {{ tr("项 ·") }} <span class="mono">{{ formatBytes(checkedStats.bytes) }}</span></template>
      <template v-else-if="activeRow">
        <FileNodeIcon :id="activeRow.id" :kind="activeRow.kind" :name="activeRow.name" :size="13" class="selection-icon" />
        <span class="selection-name" :title="activeRow.name">{{ activeRow.name }}</span>
      </template>
    </span>
    <button v-if="checkedFiles.length || (!checkedStats.count && activeRow?.kind === 'file')" class="ui-btn sm primary" :disabled="indexOnly" :title="indexOnly ? tr('导出需恢复原镜像') : undefined" @click="emit('exportFile', activeRow, checkedFiles)"><Download :size="14" />{{ tr("导出") }}{{ checkedFiles.length ? tr("文件 ({0})", { 0: checkedFiles.length }) : tr("文件") }}</button>
    <button v-if="checkedDirs.length || (!checkedStats.count && activeRow?.kind === 'dir')" class="ui-btn sm" :disabled="indexOnly" :title="indexOnly ? tr('导出需恢复原镜像') : undefined" @click="emit('exportDirectory', activeRow, checkedDirs)"><Download :size="14" />{{ tr("导出") }}{{ checkedDirs.length ? tr("目录 ({0})", { 0: checkedDirs.length }) : tr("目录") }}</button>
    <button v-if="activeRow" class="ui-btn sm" @click="emit('preview')"><Eye :size="14" />{{ tr("预览") }}</button>
    <button v-if="checkedStats.count" class="ui-btn sm ghost" @click="emit('clearChecked')">{{ tr("取消勾选") }}</button>
  </div>
</div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { Filter, X, Loader2, Camera, Download, Eye } from 'lucide-vue-next';
import FileNodeIcon from '../../../components/display/FileNodeIcon.vue';
import { formatBytes, type DiskFsNode } from '../model/model';

defineProps<{
  indexOnly?: boolean;
  deepsearch: boolean; flatTotal: number; flatTruncated: boolean;
  directoryTotal: number; directoryCursor: string | null;
  loadedCount: number; displayedCount: number; activeFilterCount: number;
  snapshotStatus: { state: 'building' | 'ready' | 'stale'; label: string } | null;
  checkedStats: { count: number; bytes: number };
  activeRow: DiskFsNode | null; checkedFiles: DiskFsNode[]; checkedDirs: DiskFsNode[];
}>();
const emit = defineEmits<{
  clearFilters: []; clearChecked: []; preview: [];
  exportFile: [node: DiskFsNode | null, checked: DiskFsNode[]];
  exportDirectory: [node: DiskFsNode | null, checked: DiskFsNode[]];
}>();
</script>

<style scoped>
/* Footer status chips — severity-tinted by state. */
.snapshot-chip {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  height: 20px;
  padding: 0 7px;
  border-radius: var(--radius-sm);
  font-size: 11px;
  white-space: nowrap;
  border: 1px solid transparent;
}

.snapshot-chip.snap-ready {
  color: var(--sev-success-fg);
  background: var(--sev-success-bg);
  border-color: color-mix(in srgb, var(--sev-success-border) 60%, transparent);
}

/* 列过滤生效中的提示芯片：点击清除全部过滤条件。 */
.filter-chip {
  color: var(--sev-info-fg);
  background: var(--sev-info-bg);
  border-color: color-mix(in srgb, var(--sev-info-border) 60%, transparent);
  cursor: pointer;
  font-family: var(--font-ui);
  transition: background var(--transition-fast);
}

.filter-chip:hover {
  background: var(--sev-info-bg-hover);
}

.snapshot-chip.snap-building {
  color: var(--sev-info-fg);
  background: var(--sev-info-bg);
  border-color: color-mix(in srgb, var(--sev-info-border) 60%, transparent);
}

.snapshot-chip.snap-stale {
  color: var(--sev-warn-fg);
  background: var(--sev-warn-bg);
  border-color: color-mix(in srgb, var(--sev-warn-border) 60%, transparent);
}

/* ── Footer：左侧计数与筛选，右侧快照状态与选择操作区 ── */
.list-footer {
  min-height: 38px;
  padding: 4px 8px 4px 16px;
}

.list-footer .mono {
  font-size: 11px;
}

/* 选择操作区：当前行或勾选集合的导出 / 预览，勾选时整块染上主题色。 */
.selection-well {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  max-width: 100%;
  padding: 2px 2px 2px 10px;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  background: var(--surface-secondary);
}

.selection-well.checked {
  border-color: color-mix(in srgb, var(--primary-color) 40%, var(--border-color));
  background: var(--sidebar-active-bg);
}

.selection-label {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  max-width: 260px;
  margin-right: 4px;
  overflow: hidden;
  color: var(--text-primary);
  font-size: 12px;
  white-space: nowrap;
}

.selection-label strong { font-size: 12px; font-weight: 600; color: var(--primary-color); }
.selection-icon { flex-shrink: 0; }
.selection-name { min-width: 0; overflow: hidden; text-overflow: ellipsis; }
:root[data-motion='full'] .selection-well { animation: ui-rise-in var(--motion-normal) var(--ease-out) backwards; }

.footer-note {
  color: var(--sev-warn-fg);
}

</style>
