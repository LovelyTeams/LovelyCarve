<template>
          <section class="ui-card tl-overview" :aria-label="tr('活动概览')">
            <header class="tl-overview-header">
              <div class="ui-seg tl-view-tabs" role="group" :aria-label="tr('活动视图')">
                <button class="ui-seg-item" :aria-pressed="chartMode === 'heatmap'" @click="chartMode = 'heatmap'; chartVisible = true"><CalendarDays :size="14" />{{ tr("活动热力图") }}</button>
                <button class="ui-seg-item" :aria-pressed="chartMode === 'trend'" @click="chartMode = 'trend'; chartVisible = true"><BarChart3 :size="14" />{{ tr("事件趋势") }}</button>
              </div>
              <div v-if="chartVisible && chartMode === 'heatmap'" class="tl-year-controls">
                <button class="ui-icon-btn sm" :title="tr('上一年')" :aria-label="tr('上一年')" :disabled="heatmapYear <= 100" @click="emit('change-year', heatmapYear - 1)"><ChevronLeft :size="15" /></button>
                <label><input class="ui-input sm mono tl-year-input" :value="heatmapYear" type="number" min="100" max="9998" :aria-label="tr('热力图年份')" @change="emit('year-input', $event)" /> {{ tr("年") }}</label>
                <button class="ui-icon-btn sm" :title="tr('下一年')" :aria-label="tr('下一年')" :disabled="heatmapYear >= 9998" @click="emit('change-year', heatmapYear + 1)"><ChevronRight :size="15" /></button>
              </div>
              <UiSelect v-if="chartVisible && chartMode === 'trend'" v-model="unit" class="sm tl-granularity" :options="UNIT_OPTIONS" :aria-label="tr('趋势时间粒度')" />
              <button class="ui-btn ghost sm collapse-chart" :aria-expanded="chartVisible" @click="chartVisible = !chartVisible"><component :is="chartVisible ? ChevronUp : ChevronDown" :size="14" />{{ chartVisible ? tr("收起") : tr("展开") }}</button>
            </header>
            <UiTransition><div v-if="chartVisible" class="tl-chart-body" v-content-motion="chartMode">
              <div v-if="chartMode === 'heatmap' ? loadingHeatmap : loadingBuckets" class="tl-chart-state" role="status"><Loader2 :size="18" class="spin" />{{ tr("正在读取活动分布…") }}</div>
              <div v-else-if="chartMode === 'heatmap' ? heatmapError : bucketsError" class="tl-chart-state" role="alert">{{ chartMode === 'heatmap' ? heatmapError : bucketsError }}<button class="ui-btn sm" @click="chartMode === 'heatmap' ? emit('retry-heatmap') : emit('retry-buckets')">{{ tr("重试") }}</button></div>
              <TimelineHeatmap v-else-if="chartMode === 'heatmap'" :year="heatmapYear" :buckets="heatmapBuckets" :tz-hours="tzHours" :selection="activeDrill" @select="emit('select-day', $event)" />
              <TimelineHistogram v-else :buckets="buckets" :unit="unit" :tz-hours="tzHours" :height="CHART_HEIGHT" :truncated="bucketsTruncated" @select="emit('drill', $event)" />
            </div></UiTransition>
          </section>

</template>
<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { CalendarDays, BarChart3, ChevronLeft, ChevronRight, ChevronUp, ChevronDown, Loader2 } from 'lucide-vue-next';
import UiSelect from '../../../components/ui/UiSelect.vue';
import UiTransition from '../../../components/ui/UiTransition.vue';
import { vContentMotion } from '../../../composables/uiMotionDirectives';
import TimelineHeatmap from './TimelineHeatmap.vue';
import TimelineHistogram from './TimelineHistogram.vue';
import type { TimelineBucket } from '../../../lib/ipc/timeline';
import { TIMELINE_UNITS, type TimelineUnitValue } from '../model/timelineView';
const chartMode = defineModel<'heatmap' | 'trend'>('chartMode', { required: true });
const chartVisible = defineModel<boolean>('chartVisible', { required: true });
const unit = defineModel<TimelineUnitValue>('unit', { required: true });
const UNIT_OPTIONS = TIMELINE_UNITS.map(value => ({ value, get label() { return tr("按{0}查看", { 0: tr(`timeUnits.${value}`) }); } }));
const CHART_HEIGHT = 135;
defineProps<{ heatmapYear: number; tzHours: number; heatmapBuckets: TimelineBucket[]; buckets: TimelineBucket[];
  loadingHeatmap: boolean; loadingBuckets: boolean; heatmapError: string; bucketsError: string; bucketsTruncated: boolean;
  activeDrill: { start: number; end: number } | null }>();
const emit = defineEmits<{ 'change-year': [year: number]; 'year-input': [event: Event]; 'select-day': [date: string];
  drill: [bucket: TimelineBucket]; 'retry-heatmap': []; 'retry-buckets': [] }>();
</script>

<style scoped src="../styles/timeline-workspace.css"></style>
