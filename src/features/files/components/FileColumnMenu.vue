<template>
  <UiContextMenu
    v-model:open="open"
    :x="x"
    :y="y"
    :entries="entries"
    :label="tr('列操作')"
    :heading="column ? tr(column.label) : undefined"
    :subheading="sortedLabel"
  />
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed } from 'vue';
import { ArrowUpDown, Columns3, EyeOff, Filter, RotateCcw } from 'lucide-vue-next';
import UiContextMenu, { type UiContextMenuEntry } from '../../../components/ui/UiContextMenu.vue';
import { compactMenuEntries } from '../../../utils/menuPlacement';
import type { ColumnDef } from '../model/model';

/** The menu of a column header: sort, filter or hide that column, and the column set. */
const props = defineProps<{
  x: number;
  y: number;
  /** The column the menu opened on; null on the check column. */
  column: ColumnDef | null;
  sortKey: string | null;
  sortDir: 'asc' | 'desc';
  sortable: boolean;
  filterable: boolean;
  filtered: boolean;
  /** The last visible column cannot be hidden. */
  visibleCount: number;
}>();
const open = defineModel<boolean>('open', { required: true });
const emit = defineEmits<{
  sort: [direction: 'asc' | 'desc' | null];
  filter: [];
  hide: [];
  'show-columns': [];
  'reset-columns': [];
}>();

const sortedDirection = computed(() => (props.column && props.sortKey === props.column.key ? props.sortDir : null));
const sortedLabel = computed(() => {
  if (sortedDirection.value === 'asc') return tr('升序');
  if (sortedDirection.value === 'desc') return tr('降序');
  return undefined;
});

const entries = computed<UiContextMenuEntry[]>(() => {
  const column = props.column;
  const list: UiContextMenuEntry[] = [];
  if (column) {
    const direction = sortedDirection.value;
    const sortTitle = props.sortable ? undefined : tr('列出所有子目录时不能按该列排序');
    list.push(
      { id: 'asc', label: tr('升序排列'), checked: direction === 'asc', disabled: !props.sortable, title: sortTitle, action: () => emit('sort', direction === 'asc' ? null : 'asc') },
      { id: 'desc', label: tr('降序排列'), checked: direction === 'desc', disabled: !props.sortable, title: sortTitle, action: () => emit('sort', direction === 'desc' ? null : 'desc') },
    );
    if (direction) list.push({ id: 'unsort', label: tr('取消排序'), icon: ArrowUpDown, action: () => emit('sort', null) });
    list.push({ kind: 'separator' });
    if (column.filterMode !== 'none') {
      list.push({
        id: 'filter', icon: Filter, disabled: !props.filterable,
        label: props.filtered ? tr('修改「{0}」的过滤条件…', { 0: tr(column.label) }) : tr('过滤「{0}」…', { 0: tr(column.label) }),
        title: props.filterable ? undefined : tr('「{0}」的数据来自独立存档，文件全显的分页查询无法按它过滤', { 0: tr(column.label) }),
        action: () => emit('filter'),
      });
    }
    list.push(
      { id: 'hide', label: tr('隐藏此列'), icon: EyeOff, disabled: props.visibleCount <= 1, action: () => emit('hide') },
      { kind: 'separator' },
    );
  }
  list.push(
    { id: 'columns', label: tr('显示列…'), icon: Columns3, action: () => emit('show-columns') },
    { id: 'reset', label: tr('恢复默认列'), icon: RotateCcw, action: () => emit('reset-columns') },
  );
  return compactMenuEntries(list);
});
</script>
