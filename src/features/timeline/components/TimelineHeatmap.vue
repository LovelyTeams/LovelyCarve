<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, ref, watch } from 'vue';
import type { TimelineBucket } from '../../../lib/ipc/timeline';
import { buildHeatmapDays, heatmapLevel } from '../model/timelineHeatmap';
import { bucketRange } from '../model/timelineView';

const props = defineProps<{
  year: number;
  buckets: TimelineBucket[];
  tzHours: number;
  selection: { start: number; end: number } | null;
}>();
const emit = defineEmits<{ select: [date: string] }>();
const days = computed(() => buildHeatmapDays(props.year, props.buckets));
const weeks = computed(() => (days.value.at(-1)?.week ?? 52) + 1);
const months = computed(() => days.value.filter(day => day.day === 1));
const max = computed(() => Math.max(0, ...props.buckets.map(bucket => bucket.count)));
const total = computed(() => props.buckets.reduce((sum, bucket) => sum + bucket.count, 0));
const activeDays = computed(() => days.value.filter(day => day.count > 0).length);
const weekdays = computed(() => [tr("一"), tr("二"), tr("三"), tr("四"), tr("五"), tr("六"), tr("日")]);
const focusedDay = ref(0);
watch(() => props.year, () => { focusedDay.value = 0; });
function onDayKeydown(event: KeyboardEvent, index: number) {
  const steps: Record<string, number> = { ArrowLeft: -7, ArrowRight: 7, ArrowUp: -1, ArrowDown: 1 };
  const step = steps[event.key];
  if (step == null && event.key !== 'Home' && event.key !== 'End') return;
  event.preventDefault();
  const target = event.key === 'Home' ? 0 : event.key === 'End' ? days.value.length - 1 : Math.max(0, Math.min(days.value.length - 1, index + step));
  focusedDay.value = target;
  (event.currentTarget as HTMLElement).parentElement?.querySelector<HTMLButtonElement>(`[data-day-index="${target}"]`)?.focus();
}
function selected(date: string) {
  if (!props.selection) return false;
  const range = bucketRange(date, 'day', props.tzHours);
  return props.selection.start <= range.end && props.selection.end >= range.start;
}
</script>

<template>
  <div class="heatmap" :aria-label="tr('年度活动热力图')">
    <div class="calendar-scroll">
      <div class="calendar" :style="{ '--weeks': weeks }">
        <span v-for="month in months" :key="month.month" class="month" :style="{ gridColumn: `${month.week + 2} / span 3`, gridRow: 1 }">{{ month.month }}{{ tr("月") }}</span>
        <span v-for="(day, index) in weekdays" :key="day" class="weekday" :style="{ gridColumn: 1, gridRow: index + 2 }">{{ day }}</span>
        <button v-for="(day, index) in days" :key="day.date" type="button" class="day" :data-day-index="index" :tabindex="focusedDay === index ? 0 : -1"
          :class="[`level-${heatmapLevel(day.count, max)}`, { selected: selected(day.date) }]"
          :style="{ gridColumn: day.week + 2, gridRow: day.weekday + 2 }"
          :title="tr('{0} · {1} 条事件，点击查看当天记录', { 0: day.date, 1: day.count.toLocaleString() })"
          :aria-label="tr('{0}，{1} 条事件', { 0: day.date, 1: day.count })" :aria-pressed="selected(day.date)"
          @focus="focusedDay = index" @keydown="onDayKeydown($event, index)"
          @click="emit('select', day.date)"></button>
      </div>
    </div>
    <div class="heatmap-footer">
      <span>{{ year }} {{ tr("年 ·") }} {{ activeDays }} {{ tr("个活跃日 ·") }} {{ total.toLocaleString() }} {{ tr("条事件") }}<span v-if="max"> {{ tr("· 单日峰值") }} {{ max.toLocaleString() }}</span></span>
      <span class="legend">{{ tr("少") }} <i v-for="level in [0, 1, 2, 3, 4]" :key="level" :class="`level-${level}`"></i> {{ tr("多") }}</span>
    </div>
  </div>
</template>

<style scoped>
.heatmap { min-width: 0; }
.calendar-scroll { overflow-x: auto; padding: 4px; }
.calendar { display: grid; grid-template-columns: 24px repeat(var(--weeks), minmax(24px, 1fr)); grid-template-rows: 20px repeat(7, 24px); gap: 4px; min-width: max-content; }
.month, .weekday { font-size: 11px; color: var(--text-secondary); align-self: center; }
.weekday { text-align: center; }
.day { min-width: 0; padding: 0; border: 1px solid transparent; border-radius: 3px; cursor: pointer; transition: outline-color var(--transition-fast); }
.day:hover, .day:focus-visible, .day.selected { outline: 2px solid var(--text-primary); outline-offset: 1px; }
.level-0 { background: var(--surface-tertiary); border: 1px solid var(--border-color); }
.level-1 { background: color-mix(in srgb, var(--primary-color) 22%, var(--surface-secondary)); }
.level-2 { background: color-mix(in srgb, var(--primary-color) 45%, var(--surface-secondary)); }
.level-3 { background: color-mix(in srgb, var(--primary-color) 70%, var(--surface-secondary)); }
.level-4 { background: var(--primary-color); }
.heatmap-footer { display: flex; justify-content: space-between; gap: 12px; flex-wrap: wrap; margin-top: 10px; font-size: 11px; color: var(--text-secondary); }
.legend { display: inline-flex; gap: 4px; align-items: center; }
.legend i { width: 11px; height: 11px; border-radius: 3px; }
</style>
