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
                  <tr v-for="{ item } in visibleItems" :key="item.id" :class="{ selected: selectedRowId === item.id, deleted: item.deleted }" :aria-selected="selectedRowId === item.id" tabindex="0" @click="emit('select', item.id)" @dblclick="emit('open', item)" @keydown.enter="emit('select', item.id)">
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
          </section>
</template>
<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, nextTick, onScopeDispose, ref, watch } from 'vue';
import { ArrowDown, ArrowUp, ArrowUpRight, PanelRightOpen, Loader2, CalendarDays } from 'lucide-vue-next';
import { useVirtualList } from '../../../composables/useVirtualList';
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
</script>
<style scoped src="../../../styles/data-table.css"></style>

<style scoped src="../styles/timeline-workspace.css"></style>
