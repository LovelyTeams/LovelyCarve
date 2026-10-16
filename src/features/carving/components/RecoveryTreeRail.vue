<template>
  <aside class="tree-rail" :aria-label="tr('恢复目录')">
    <div class="tree-header"><FolderTree :size="14" class="tree-header-icon" /><span class="tree-header-label">{{ tr("目录树") }}</span></div>
    <div class="tree-list">
      <button v-for="row in rows" :key="row.entry.key" type="button" class="tree-row" :class="{ active: row.entry.key === currentKey }"
        :style="{ paddingLeft: `${8 + row.depth * 14}px` }" :title="row.entry.name" :aria-current="row.entry.key === currentKey ? 'location' : undefined" @click="emit('navigate', row.entry.key)">
        <span class="tree-chevron-wrap" :aria-label="row.expandable ? tr('展开或收起 {0}', { 0: row.entry.name }) : undefined" @click.stop="row.expandable && emit('toggle', row.entry.key)">
          <ChevronRight v-if="row.expandable" :size="12" class="tree-chevron" :class="{ open: expanded.has(row.entry.key) }" />
        </span>
        <ScanSearch v-if="row.entry.kind === 'signatures'" :size="13" class="tree-node-icon signature-icon" />
        <FileNodeIcon v-else :id="row.entry.kind === 'volume' ? 'part:0' : row.entry.key" kind="dir" :name="row.entry.name" :size="13" class="tree-node-icon" />
        <span class="tree-name" :class="{ deleted: row.entry.node?.deleted }">{{ row.entry.name }}</span>
        <span v-if="row.entry.fileCount" class="tree-count mono">{{ row.entry.fileCount.toLocaleString() }}</span>
      </button>
      <div v-if="!rows.length" class="tree-empty">{{ loading ? tr("正在读取恢复结果…") : tr("没有可显示的目录") }}</div>
    </div>
  </aside>
</template>
<script setup lang="ts">
import { tr } from '../../../composables/useLocale';
import { ChevronRight, FolderTree, ScanSearch } from 'lucide-vue-next';
import FileNodeIcon from '../../../components/display/FileNodeIcon.vue';
import type { RecoveryTreeRow } from '../composables/useRecoveryBrowser';
defineProps<{ rows: RecoveryTreeRow[]; currentKey: string; expanded: ReadonlySet<string>; loading: boolean }>();
const emit = defineEmits<{ navigate: [key: string]; toggle: [key: string] }>();
</script>
<style scoped>
/* 与文件列表的目录树同一套尺寸与配色：表头与右侧表格表头同高同色。 */
.tree-rail { display: flex; flex-direction: column; width: 240px; min-width: 180px; max-width: 34%; flex-shrink: 0; border-right: 1px solid var(--border-color); background: var(--surface-secondary); }
.tree-header { display: flex; align-items: center; gap: 8px; box-sizing: border-box; height: 34px; padding: 0 6px 0 12px; border-bottom: 1px solid var(--border-color); background: color-mix(in srgb, var(--surface-tertiary) 60%, var(--surface-secondary)); flex-shrink: 0; }
.tree-header-icon { color: var(--text-secondary); }
.tree-header-label { color: var(--text-secondary); font-size: 12px; font-weight: 500; user-select: none; }
.tree-list { flex: 1; min-height: 0; overflow-y: auto; padding: 4px; }
.tree-row { display: flex; align-items: center; gap: 4px; width: 100%; min-height: 26px; padding: 2px 8px 2px 0; border: none; border-radius: var(--radius-sm); background: transparent; color: var(--text-primary); font-family: var(--font-ui); font-size: 12px; text-align: left; white-space: nowrap; overflow: hidden; cursor: pointer; transition: background var(--transition-fast), color var(--transition-fast); }
.tree-row:hover { background: var(--hover-bg); }
.tree-row.active { background: var(--sidebar-active-bg); color: var(--primary-color); font-weight: 500; }
.tree-row:focus-visible { outline: 2px solid var(--primary-color); outline-offset: -2px; }
.tree-chevron-wrap { display: inline-flex; align-items: center; justify-content: center; width: 14px; height: 14px; flex-shrink: 0; border-radius: var(--radius-sm); color: var(--text-secondary); }
.tree-chevron-wrap:hover { background: var(--active-bg); color: var(--text-primary); }
.tree-chevron { transition: rotate var(--transition-fast); }
.tree-chevron.open { rotate: 90deg; }
.tree-node-icon { flex-shrink: 0; }
.signature-icon { color: var(--primary-color); }
.tree-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; }
.tree-name.deleted { color: var(--sev-critical-fg); text-decoration: line-through; }
.tree-count { flex-shrink: 0; min-width: 20px; height: 18px; margin-left: auto; padding: 0 6px; border-radius: 999px; background: var(--chip-count-bg); color: var(--text-secondary); font-size: 11px; line-height: 18px; text-align: center; }
.tree-row.active .tree-count { background: color-mix(in srgb, var(--primary-color) 16%, transparent); color: var(--primary-color); }
.tree-empty { padding: 16px; color: var(--text-secondary); font-size: 11px; text-align: center; }
.mono { font-family: var(--font-mono); }
@media (max-width: 760px) { .tree-rail { width: 180px; } }
</style>
