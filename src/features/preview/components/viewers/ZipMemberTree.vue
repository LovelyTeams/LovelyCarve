<template>
        <div class="zip-tree-pane">
          <div class="tree-toolbar">
            <div class="ui-search filter">
              <Search :size="13" />
              <input v-model="filter" type="text" :placeholder="tr('过滤路径')" spellcheck="false" />
            </div>
            <button v-if="!filterQuery" class="ui-icon-btn sm bordered" :title="allExpanded ? tr('全部折叠') : tr('全部展开')" :aria-label="allExpanded ? tr('全部折叠') : tr('全部展开')" @click="toggleAll">
              <component :is="allExpanded ? FoldVertical : UnfoldVertical" :size="14" />
            </button>
          </div>

          <!-- 过滤态：匹配项扁平列表；否则可折叠目录树。两种行都虚拟化渲染——
               中央目录最多列出 5 万个成员，全部铺成 DOM 会卡住预览面板。 -->
          <div ref="treeScrollEl" class="tree-scroll">
            <div v-if="filterQuery && listRows.length === 0" class="empty-row mono">{{ tr("无匹配项") }}</div>
            <div v-if="paddingTop > 0" :style="{ height: `${paddingTop}px` }" aria-hidden="true"></div>
            <template v-for="{ item: row } in visibleListRows" :key="row.key">
              <button
                v-if="row.mode === 'match'"
                class="tree-row"
                :class="{ selected: selected?.path === row.entry.path }"
                :style="{ paddingLeft: '10px' }"
                @click="onEntryClick(row.entry)"
              >
                <component :is="iconFor(row.entry)" :size="14" class="row-icon" :class="iconClass(row.entry)" />
                <span class="row-name mono">{{ row.entry.path }}</span>
                <Lock v-if="row.entry.encrypted" :size="12" class="row-lock" />
                <span class="row-meta mono">{{ row.entry.kind === 'dir' ? '' : formatBytes(row.entry.size) }}</span>
              </button>
              <button
                v-else
                class="tree-row"
                :class="{ selected: selected?.path === row.node.path }"
                :style="{ paddingLeft: `${10 + row.depth * 16}px` }"
                @click="onRowClick(row.node)"
              >
                <span class="row-chevron" :class="{ hidden: !row.node.isDir }">
                  <ChevronRight v-if="!isOpen(row.node)" :size="13" />
                  <ChevronDown v-else :size="13" />
                </span>
                <component :is="rowIcon(row.node)" :size="14" class="row-icon" :class="rowIconClass(row.node)" />
                <span class="row-name mono">{{ row.node.name }}</span>
                <Lock v-if="!row.node.isDir && row.node.entry?.encrypted" :size="12" class="row-lock" />
                <span class="row-meta mono">
                  <template v-if="!row.node.isDir && row.node.entry && row.node.entry.kind !== 'dir'">
                    {{ formatBytes(row.node.entry.size) }}<template v-if="row.node.entry.method"> · {{ row.node.entry.method }}</template>
                  </template>
                </span>
              </button>
            </template>
            <div v-if="paddingBottom > 0" :style="{ height: `${paddingBottom}px` }" aria-hidden="true"></div>
          </div>
        </div>

</template>
<script setup lang="ts">
import { tr } from '../../../../composables/useLocale';

import { ref, watch } from 'vue';
import { Search, FoldVertical, UnfoldVertical, ChevronRight, ChevronDown, Lock, Folder, FolderOpen, File, Link2 } from 'lucide-vue-next';
import { useVirtualList } from '../../../../composables/useVirtualList';
import { useZipTree } from '../../composables/useZipTree';
import { formatBytes } from '../../../../utils/format';
import type { ZipPreview, ZipPreviewEntry } from '../../../../lib/ipc/zip';
import type { ZipTreeNode as TreeNode } from '../../model/zipTree';
const props = defineProps<{ report: ZipPreview; selected: ZipPreviewEntry | null }>();
const emit = defineEmits<{ select: [entry: ZipPreviewEntry] }>();
const { filter, filterQuery, listRows, allExpanded, toggleAll, onRowClick, onEntryClick, isOpen } = useZipTree({ report: () => props.report, onSelect: entry => emit('select', entry) });
const treeScrollEl = ref<HTMLElement | null>(null);
const { visibleItems: visibleListRows, paddingTop, paddingBottom, scrollToTop } = useVirtualList({ items: listRows, itemHeight: 24, containerRef: treeScrollEl, overscan: 12 });
watch(filterQuery, scrollToTop);
function rowIcon(node: TreeNode) {
  if (node.isDir) return isOpen(node) ? FolderOpen : Folder;
  if (node.entry?.kind === 'symlink') return Link2;
  return File;
}
function rowIconClass(node: TreeNode) {
  if (node.isDir) return 'i-dir';
  if (node.entry?.kind === 'other') return 'i-locked';
  return 'i-file';
}
function iconFor(e: ZipPreviewEntry) {
  if (e.kind === 'dir') return Folder;
  if (e.kind === 'symlink') return Link2;
  return File;
}
function iconClass(e: ZipPreviewEntry) {
  if (e.kind === 'dir') return 'i-dir';
  if (e.kind === 'other') return 'i-locked';
  return 'i-file';
}

</script>
<style scoped src="../../styles/zip-viewer.css"></style>
