<template>
  <UiContextMenu
    v-model:open="open"
    :x="x"
    :y="y"
    :entries="entries"
    :label="tr('当前目录操作')"
    :heading="directoryName"
    :subheading="tr('{0} 项', { 0: rowCount.toLocaleString() })"
    :heading-icon="Folder"
  />
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed } from 'vue';
import { ClipboardCopy, Columns3, CornerLeftUp, Folder, FolderDown, LayoutGrid, ListChecks, ListX, RefreshCw, X } from 'lucide-vue-next';
import UiContextMenu, { type UiContextMenuEntry } from '../../../components/ui/UiContextMenu.vue';
import { compactMenuEntries } from '../../../utils/menuPlacement';

/** The menu of the file list's blank area: the current directory and the view. */
const props = defineProps<{
  x: number;
  y: number;
  directoryName: string;
  rowCount: number;
  atRoot: boolean;
  loading: boolean;
  allChecked: boolean;
  checkedCount: number;
  recursive: boolean;
  hideEmptyDirs: boolean;
  treemapAvailable: boolean;
  indexOnly: boolean;
}>();
const open = defineModel<boolean>('open', { required: true });
const emit = defineEmits<{
  'go-up': [];
  refresh: [];
  'toggle-all': [];
  'clear-checked': [];
  'set-recursive': [recursive: boolean];
  'set-hide-empty': [hide: boolean];
  'show-treemap': [];
  'show-columns': [];
  'copy-path': [];
  'export-directory': [];
}>();

const entries = computed<UiContextMenuEntry[]>(() => {
  const list: UiContextMenuEntry[] = [
    { id: 'up', label: tr('返回上级'), icon: CornerLeftUp, hint: 'Backspace', disabled: props.atRoot || props.loading, action: () => emit('go-up') },
    { id: 'refresh', label: tr('刷新'), icon: RefreshCw, action: () => emit('refresh') },
    { kind: 'separator' },
    {
      id: 'toggle-all', label: props.allChecked ? tr('取消全选') : tr('全选'), icon: props.allChecked ? ListX : ListChecks,
      hint: 'Ctrl+A', disabled: props.rowCount === 0, action: () => emit('toggle-all'),
    },
  ];
  if (props.checkedCount > 0) {
    list.push({ id: 'clear-checked', label: tr('清空勾选（{0}）', { 0: props.checkedCount.toLocaleString() }), icon: X, action: () => emit('clear-checked') });
  }
  list.push(
    { kind: 'separator' },
    { id: 'recursive', label: tr('列出所有子目录中的文件'), checked: props.recursive, action: () => emit('set-recursive', !props.recursive) },
    {
      id: 'hide-empty', label: tr('隐藏空目录'), checked: props.hideEmptyDirs, disabled: props.recursive,
      title: props.recursive ? tr('列出所有子目录时只列文件') : undefined, action: () => emit('set-hide-empty', !props.hideEmptyDirs),
    },
    { kind: 'separator' },
  );
  if (props.treemapAvailable) list.push({ id: 'treemap', label: tr('切换到树图'), icon: LayoutGrid, action: () => emit('show-treemap') });
  list.push(
    { id: 'columns', label: tr('显示列…'), icon: Columns3, action: () => emit('show-columns') },
    { kind: 'separator' },
    { id: 'copy-path', label: tr('复制当前目录路径'), icon: ClipboardCopy, action: () => emit('copy-path') },
  );
  if (!props.atRoot) {
    list.push({
      id: 'export', label: tr('导出此文件夹'), icon: FolderDown, disabled: props.indexOnly,
      title: props.indexOnly ? tr('离线索引 · 需恢复源镜像') : undefined, action: () => emit('export-directory'),
    });
  }
  return compactMenuEntries(list);
});
</script>
