<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, onMounted, onScopeDispose, ref, watch } from 'vue';
import { CalendarRange, TriangleAlert, CircleAlert, Download, Loader2, RefreshCw, SlidersHorizontal, Undo2, X, Search, BarChart3, Plus } from 'lucide-vue-next';
import UiTransition from '../../../components/ui/UiTransition.vue';
import WorkspaceStartState from '../../../components/layout/WorkspaceStartState.vue';
import TimelineOverview from './TimelineOverview.vue';
import TimelineRecordTable from './TimelineRecordTable.vue';
import TimelineFilterDialog from './TimelineFilterDialog.vue';
import { dataTypeIcon } from './timelineIcons';
import TimelineEventDetail from './TimelineEventDetail.vue';
import { useTimelineFilters } from '../composables/useTimelineFilters';
import { useTimelineAggregation } from '../composables/useTimelineAggregation';
import { useTimelineRows } from '../composables/useTimelineRows';
import { useTimelineBuild } from '../composables/useTimelineBuild';
import { useTimelineExport } from '../composables/useTimelineExport';
import { formatEpoch, formatRange, localTimeZoneHours, statusLabel, flagTone } from '../model/timelineView';
import type { TimelineRow } from '../../../lib/ipc/timeline';
import type { FileLocationTarget } from '../../../lib/models/fileLocation';
const props = defineProps<{ imageId: string; locating?: boolean }>();
const emit = defineEmits<{ 'open-file': [target: FileLocationTarget] }>();
const tzHours = localTimeZoneHours();
const chartMode = ref<'heatmap' | 'trend'>('heatmap');
const chartVisible = ref(true);
const filters = useTimelineFilters({ imageId: () => props.imageId, tzHours });
const { unit, heatmapYear, drillStack, activeDrill, showDelete, filterTextDraft, filterText,
  selectedPairs, typeGroups, typeFilterActive, hasFilters, dialog, buildQuery, drillInto, undoDrill,
  selectDay, changeHeatmapYear, inputHeatmapYear, resetFilters, openConfigDialog,
  toggleDialogPair, toggleDialogGroup, applyConfigDialog, applyFilters } = filters;
const aggregation = useTimelineAggregation({ imageId: () => props.imageId, buildQuery, heatmapYear: () => heatmapYear.value, tzHours });
const { buckets, bucketsTruncated, loadingBuckets, bucketsError, heatmapBuckets, loadingHeatmap, heatmapError, loadBuckets, loadHeatmap } = aggregation;
const query = useTimelineRows({ imageId: () => props.imageId, buildQuery });
const { rows, rowsTotal, loadingRows, order, selectedRowId, selectedRow, rowsError, loadRowsFirstPage, ensureRowsThrough, toggleOrder } = query;
const build = useTimelineBuild({ imageId: () => props.imageId, onBuilt: () => { loadAll(); void loadHeatmap(); } });
const { statusInfo, statusError, building, progress, statusChipClass, buildButtonLabel, refreshStatus, buildButtonClicked } = build;
const spanLabel = computed(() => {
  const info = statusInfo.value;
  if (!info) return tr("正在读取时间线状态…");
  if (info.status === 'buildRequired') return tr("尚未创建 · 汇总文件时间与分析事件");
  const zone = `UTC${tzHours >= 0 ? '+' : ''}${tzHours}`;
  if (info.start_time == null || info.end_time == null) return tr("暂无时间范围 · {0}", { 0: zone });
  return `${formatEpoch(info.start_time, tzHours).slice(0, 10)} → ${formatEpoch(info.end_time, tzHours).slice(0, 10)} · ${zone}`;
});

function loadAll() { void loadBuckets(); void loadRowsFirstPage(); }
watch([unit, showDelete, filterText, selectedPairs, drillStack], () => { if (statusInfo.value && statusInfo.value.status !== 'buildRequired') loadAll(); }, { deep: true });
watch([heatmapYear, showDelete, filterText, selectedPairs], () => { if (statusInfo.value && statusInfo.value.status !== 'buildRequired') void loadHeatmap(); }, { deep: true });
const { exporting, exportRows } = useTimelineExport({ imageId: () => props.imageId, buildQuery, total: () => rowsTotal.value });
function openRow(row: TimelineRow) {
  if (props.locating) return;
  if (row.data_type !== 1) { selectedRowId.value = row.id; return; }
  emit('open-file', { imageId: row.eid, nodeId: row.oid });
}
let initGeneration = 0;
async function initPanel() {
  const captured = ++initGeneration;
  filters.resetContext(); query.reset(); aggregation.reset();
  const info = await refreshStatus();
  if (!info || captured !== initGeneration) return;
  if (info.end_time != null) heatmapYear.value = Number(formatEpoch(info.end_time, tzHours).slice(0, 4));
  if (info.status !== 'buildRequired') { loadAll(); void loadHeatmap(); }
}
onMounted(() => { void initPanel(); });
watch(() => props.imageId, () => { void initPanel(); }, { flush: 'sync' });
onScopeDispose(() => { initGeneration++; });
</script>

<template>
  <section class="timeline-panel" :aria-label="tr('时间线')">
    <WorkspaceStartState v-if="!building && (!statusInfo || statusInfo.status === 'buildRequired')"
      :icon="BarChart3" :loading="!statusInfo && !statusError"
      :title="statusError ? tr('读取时间线失败') : statusInfo ? tr('还没有时间线') : tr('正在读取时间线…')"
      :description="statusError || tr('汇总文件时间与分析事件，查看事件发生顺序。')"
      :action-label="statusError ? tr('重试') : tr('创建时间线')" :action-icon="statusError ? RefreshCw : Plus"
      @action="statusError ? initPanel() : buildButtonClicked()" />
    <template v-else>
    <!-- 页面不设标题栏：工具条左侧为检索与筛选，右侧为构建状态与操作；
         覆盖时间段与计数放在底部状态栏。 -->
    <form class="ui-toolbar tl-filter-row" @submit.prevent="applyFilters">
      <div class="ui-search tl-search"><Search :size="14" /><input v-model="filterTextDraft" type="text" :placeholder="tr('搜索事件内容或文件路径')" :aria-label="tr('搜索时间线')" /><button class="ui-btn sm ghost tl-search-submit" type="submit">{{ tr("搜索") }}</button></div>
      <button type="button" class="ui-btn" :class="{ active: typeFilterActive }" @click="openConfigDialog"><SlidersHorizontal :size="14" />{{ typeFilterActive ? tr("{0} 类事件", { 0: selectedPairs.length }) : tr("全部类型") }}</button>
      <label class="ui-check"><input v-model="showDelete" type="checkbox" />{{ tr("包含已删除") }}</label>
      <span v-if="activeDrill" class="tl-drill-chip" :title="tr('仅显示 {0} 的事件', { 0: formatRange(activeDrill.start, activeDrill.end, tzHours) })">
        <CalendarRange :size="13" /><span class="mono">{{ formatRange(activeDrill.start, activeDrill.end, tzHours) }}</span>
        <button v-if="drillStack.length > 1" class="ui-icon-btn sm" type="button" :title="tr('返回上一级')" :aria-label="tr('返回上一级')" @click="undoDrill"><Undo2 :size="12" /></button>
        <button class="ui-icon-btn sm" type="button" :title="tr('显示全部时间')" :aria-label="tr('显示全部时间')" @click="drillStack = []; unit = 'year'"><X :size="12" /></button>
      </span>
      <button v-if="hasFilters || filterTextDraft" class="ui-btn link" type="button" @click="resetFilters"><Undo2 :size="13" />{{ tr("重置筛选") }}</button>
      <div class="ui-toolbar-actions">
        <span class="ui-badge sm tl-status" :class="statusChipClass" :title="spanLabel">{{ statusInfo ? statusLabel(statusInfo.status, tr) : tr("读取中…") }}</span>
        <button type="button" class="ui-btn" :class="{ primary: statusInfo?.status === 'buildRequired' || statusInfo?.status === 'stale' }" :disabled="statusInfo === null" @click="buildButtonClicked">
          <Loader2 v-if="building || statusInfo?.status === 'running'" :size="14" class="spin" /><RefreshCw v-else :size="14" />{{ buildButtonLabel }}
        </button>
        <button type="button" class="ui-btn" :disabled="exporting || rowsTotal === 0 || loadingRows" @click="exportRows"><Download :size="14" />{{ exporting ? tr("导出中…") : tr("导出结果") }}</button>
      </div>
    </form>

    <!-- 概览、事件表与状态栏在左，单击事件后详情从右侧展开。 -->
    <div class="tl-workspace">
      <div class="tl-column">
        <div class="tl-body">
          <div v-if="building || statusInfo?.status === 'running'" class="ui-alert info tl-notice" role="status">
            <Loader2 :size="14" class="spin" /><span>{{ tr("正在构建，当前结果可能不完整") }}</span>
            <span v-if="progress">{{ tr("已处理") }} {{ progress.done.toLocaleString() }}<template v-if="progress.total > 0"> / {{ progress.total.toLocaleString() }}</template> {{ tr("项 · 已生成") }} {{ progress.rows.toLocaleString() }} {{ tr("条事件") }}</span>
          </div>
          <div v-else-if="statusInfo?.status === 'stale'" class="ui-alert warn tl-notice"><TriangleAlert :size="14" /><span>{{ tr("有新数据尚未加入时间线，点击「更新时间线」即可同步。") }}</span></div>
          <div v-if="statusInfo?.meta?.error" class="ui-alert danger tl-notice" role="alert"><CircleAlert :size="14" /><span>{{ statusInfo.meta.error }}</span></div>

          <TimelineOverview v-if="statusInfo && statusInfo.status !== 'buildRequired'" v-model:chart-mode="chartMode" v-model:chart-visible="chartVisible" v-model:unit="unit"
            :heatmap-year="heatmapYear" :tz-hours="tzHours" :heatmap-buckets="heatmapBuckets" :buckets="buckets"
            :loading-heatmap="loadingHeatmap" :loading-buckets="loadingBuckets" :heatmap-error="heatmapError" :buckets-error="bucketsError"
            :buckets-truncated="bucketsTruncated" :active-drill="activeDrill" @change-year="changeHeatmapYear" @year-input="inputHeatmapYear"
            @select-day="selectDay" @drill="drillInto" @retry-heatmap="loadHeatmap" @retry-buckets="loadBuckets" />

          <TimelineRecordTable :rows="rows" :rows-total="rowsTotal" :selected-row-id="selectedRowId" :order="order" :tz-hours="tzHours"
            :locating="locating" :loading-rows="loadingRows" :rows-error="rowsError" :has-filters="hasFilters" :revision="query.revision.value"
            @select="selectedRowId = $event" @open="openRow" @sort="toggleOrder" @retry="loadRowsFirstPage" @reset-filters="resetFilters" @load-through="ensureRowsThrough" />
        </div>
        <footer class="ui-footbar tl-footer">
          <span class="tl-footer-span"><CalendarRange :size="12" />{{ spanLabel }}</span>
          <template v-if="statusInfo && statusInfo.status !== 'buildRequired'">
            <span class="ui-footbar-stat">{{ tr("事件") }} <strong :key="statusInfo.record_count" class="ui-bump">{{ statusInfo.record_count.toLocaleString() }}</strong></span>
            <span v-if="statusInfo.meta" class="ui-footbar-stat tl-footer-detail">{{ tr("文件记录") }} <strong>{{ statusInfo.meta.file_rows.toLocaleString() }}</strong></span>
            <span v-if="statusInfo.meta" class="ui-footbar-stat tl-footer-detail">{{ tr("分析事件") }} <strong>{{ statusInfo.meta.event_rows.toLocaleString() }}</strong></span>
          </template>
          <span class="ui-footbar-spacer"></span>
          <span v-if="loadingRows" class="tl-footer-loading"><Loader2 :size="12" class="spin" /> {{ tr("加载中…") }}</span>
          <span v-else class="mono">{{ tr("已加载") }} {{ rows.length.toLocaleString() }} / <strong :key="rowsTotal" class="tl-footer-total ui-bump">{{ rowsTotal.toLocaleString() }}</strong> {{ tr("条") }}{{ hasFilters ? tr("（已筛选）") : '' }}</span>
        </footer>
      </div>
      <UiTransition kind="dock"><TimelineEventDetail v-if="selectedRow" :row="selectedRow" :time="formatEpoch(selectedRow.tl_time, tzHours)" :tone="flagTone(selectedRow.tl_flag)" :icon="dataTypeIcon(selectedRow.data_type)" :locating="locating" @open="openRow" @close="selectedRowId = null" /></UiTransition>
    </div>
    </template>
    <TimelineFilterDialog :dialog="dialog" :type-groups="typeGroups" @close="dialog.open = false"
      @apply="applyConfigDialog" @toggle-group="toggleDialogGroup" @toggle-pair="toggleDialogPair" />
  </section>
</template>


<style scoped src="../styles/timeline-workspace.css"></style>
