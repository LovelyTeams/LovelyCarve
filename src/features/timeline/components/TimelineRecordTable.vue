<template>
          <section class="ui-card tl-table-card" :aria-label="tr('事件记录')">
            <div ref="tableWrapRef" class="tl-table-wrap">
              <table class="data-table tl-table">
                <colgroup><col class="col-time" /><col class="col-type" /><col /><col class="col-source" /><col class="col-action" /></colgroup>
                <thead><tr>
                  <th :aria-sort="order === 'desc' ? 'descending' : 'ascending'"><button class="sort-button" @click="emit('sort')">{{ tr("时间") }}<component :is="order === 'desc' ? ArrowDown : ArrowUp" :size="12" /></button></th>
                  <th>{{ tr("事件类型") }}</th><th>{{ tr("内容") }}</th><th>{{ tr("来源") }}</th><th>{{ tr("操作") }}</th>
                </tr></thead>
                <tbody>
                  <tr v-if="paddingTop > 0" class="virtual-spacer" aria-hidden="true"><td colspan="5" :style="{ height: `${paddingTop}px` }"></td></tr>
                  <tr v-for="{ item } in visibleItems" :key="item.id" :class="{ selected: selectedRowId === item.id, deleted: item.deleted }" :aria-selected="selectedRowId === item.id" tabindex="0" @click="emit('select', item.id)" @dblclick="emit('open', item)" @keydown.enter="emit('open', item)" @contextmenu="onRowContextMenu($event, item)">
                    <td class="mono tl-time"><span class="tl-date">{{ formatEpoch(item.tl_time, tzHours).slice(0, 10) }}</span> {{ formatEpoch(item.tl_time, tzHours).slice(11) }}</td>
                    <td :title="`${dataTypeLabel(item.data_type, tr)} · ${item.tl_flag || dataTypeLabel(item.data_type, tr)}`"><span class="ui-badge tone tl-flag" :style="{ '--badge-tone': flagTone(item.tl_flag) }"><component :is="dataTypeIcon(item.data_type)" :size="12" />{{ item.tl_flag || dataTypeLabel(item.data_type, tr) }}</span><span v-if="item.deleted" class="ui-badge sm danger deleted-label">{{ tr("已删除") }}</span></td>
                    <td class="tl-content" :title="item.content"><span>{{ item.content || tr("（无摘要）") }}</span></td>
                    <td class="source-cell" :title="item.from_app || tr('文件系统')">{{ item.from_app || tr("文件系统") }}</td>
                    <td><button v-if="item.data_type === 1" class="ui-btn link" :disabled="locating" :title="tr('在来源检材的文件列表中定位')" @click.stop="emit('open', item)" @keydown.enter.stop><ArrowUpRight :size="13" />{{ tr("定位") }}</button><button v-else class="ui-btn link" :title="tr('在右侧查看事件详情')" @click.stop="emit('select', item.id)" @keydown.enter.stop><PanelRightOpen :size="13" />{{ tr("详情") }}</button></td>
                  </tr>
                  <tr v-if="paddingBottom > 0" class="virtual-spacer" aria-hidden="true"><td colspan="5" :style="{ height: `${paddingBottom}px` }"></td></tr>
                </tbody>
              </table>
              <div v-if="rows.length === 0" class="ui-empty tl-table-empty">
                <span class="ui-empty-icon" :class="{ accent: loadingRows }"><Loader2 v-if="loadingRows" :size="24" class="spin" /><CalendarDays v-else :size="26" /></span>
                <p class="ui-empty-title">{{ loadingRows ? tr("正在读取事件…") : rowsError || tr("当前条件下没有事件") }}</p>
                <p v-if="!loadingRows && !rowsError" class="ui-empty-desc">{{ tr("可切换年份、选择其他日期或重置筛选。") }}</p>
                <div class="ui-empty-actions">
                  <button v-if="rowsError" class="ui-btn" @click="emit('retry')">{{ tr("重试") }}</button>
                  <button v-else-if="hasFilters && !loadingRows" class="ui-btn" @click="emit('reset-filters')">{{ tr("重置筛选") }}</button>
                </div>
              </div>
            </div>
            <UiContextMenu
              v-model:open="rowMenu.open"
              :x="rowMenu.x"
              :y="rowMenu.y"
              :entries="rowMenuEntries"
              :label="tr('事件操作')"
              :heading="rowMenu.target ? rowMenu.target.tl_flag || dataTypeLabel(rowMenu.target.data_type, tr) : undefined"
              :subheading="rowMenu.target ? formatEpoch(rowMenu.target.tl_time, tzHours) : undefined"
            />
          </section>
</template>
<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, nextTick, onScopeDispose, ref, watch } from 'vue';
import { ArrowDown, ArrowUp, ArrowUpRight, PanelRightOpen, Loader2, CalendarDays, Clock3, ClipboardList, TextQuote } from 'lucide-vue-next';
import UiContextMenu, { type UiContextMenuEntry } from '../../../components/ui/UiContextMenu.vue';
import { copyText } from '../../../composables/clipboard';
import { useContextMenu } from '../../../composables/useContextMenu';
import { useVirtualList } from '../../../composables/useVirtualList';
import { tsvEscape } from '../../../utils/exportFormat';
import type { TimelineRow } from '../../../lib/ipc/timeline';
import { formatEpoch, flagTone, dataTypeLabel } from '../model/timelineView';
import { dataTypeIcon } from './timelineIcons';
const props = defineProps<{ rows: TimelineRow[]; rowsTotal: number; selectedRowId: number | null;
  order: 'asc' | 'desc'; tzHours: number; locating?: boolean; loadingRows: boolean; rowsError: string;
  hasFilters: boolean; revision: number }>();
const emit = defineEmits<{ select: [id: number]; open: [row: TimelineRow]; sort: []; retry: []; 'reset-filters': []; 'load-through': [index: number] }>();
const tableWrapRef = ref<HTMLElement | null>(null);
const { visibleItems, paddingTop, paddingBottom, range } = useVirtualList({
  items: computed(() => props.rows), itemHeight: 36, containerRef: tableWrapRef, overscan: 10, virtualCount: computed(() => props.rowsTotal),
});
watch(range, range => emit('load-through', range.end + 60));
let generation = 0;
watch(() => props.revision, () => {
  const captured = ++generation;
  void nextTick(() => { if (captured === generation) tableWrapRef.value?.scrollTo({ top: 0 }); });
});
onScopeDispose(() => { generation++; });

// 右键：选中该事件（右侧显示详情），菜单作用于它。
const { menu: rowMenu, openFromEvent } = useContextMenu<TimelineRow>();
function onRowContextMenu(event: MouseEvent, row: TimelineRow) {
  emit('select', row.id);
  openFromEvent(event, row);
}
const rowMenuEntries = computed<UiContextMenuEntry[]>(() => {
  const row = rowMenu.target;
  if (!row) return [];
  const time = formatEpoch(row.tl_time, props.tzHours);
  const type = row.tl_flag || dataTypeLabel(row.data_type, tr);
  const source = row.from_app || tr('文件系统');
  const entries: UiContextMenuEntry[] = [{ id: 'detail', label: tr('查看详情'), icon: PanelRightOpen, action: () => emit('select', row.id) }];
  if (row.data_type === 1) {
    entries.push({ id: 'open', label: tr('定位源文件'), icon: ArrowUpRight, hint: 'Enter', disabled: props.locating, title: tr('在来源检材的文件列表中定位'), action: () => emit('open', row) });
  }
  entries.push(
    { kind: 'separator' },
    { id: 'copy-time', label: tr('复制时间'), icon: Clock3, action: () => void copyText(time, tr('事件时间')) },
    { id: 'copy-content', label: tr('复制内容'), icon: TextQuote, disabled: !row.content, action: () => void copyText(row.content, tr('事件内容')) },
    {
      id: 'copy-row', label: tr('复制整行'), icon: ClipboardList,
      action: () => void copyText([[tr('时间'), tr('事件类型'), tr('内容'), tr('来源')], [time, type, row.content, source]]
        .map(cells => cells.map(tsvEscape).join('\t')).join('\n'), tr('{0} 行事件', { 0: 1 })),
    },
  );
  return entries;
});
</script>
<style scoped src="../../../styles/data-table.css"></style>

<style scoped src="../styles/timeline-workspace.css"></style>
