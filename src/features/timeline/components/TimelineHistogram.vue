<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

// 时间线柱状图。
// 点击柱子 = 钻取：时间范围收窄到该桶 + 粒度降一档（由父面板处理）。
// 悬停 tooltip 显示桶区间与计数；X 轴标签按桶数抽稀。
import { computed, ref, watch } from 'vue';

import type { TimelineBucket } from '../../../lib/ipc/timeline';
import {
  bucketRange,
  bucketShortLabel,
  formatRange,
  type TimelineUnitValue,
} from '../model/timelineView';

const props = withDefaults(
  defineProps<{
    buckets: TimelineBucket[];
    unit: TimelineUnitValue;
    tzHours: number;
    height?: number;
    truncated?: boolean;
  }>(),
  { height: 160, truncated: false },
);

const emit = defineEmits<{ (e: 'select', bucket: TimelineBucket): void }>();
const focusedBucket = ref(0);
watch(() => props.buckets, () => { focusedBucket.value = 0; });
function onBarKeydown(event: KeyboardEvent, index: number) {
  const steps: Record<string, number> = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -1, ArrowDown: 1 };
  const step = steps[event.key];
  if (step == null && event.key !== 'Home' && event.key !== 'End') return;
  event.preventDefault();
  const target = event.key === 'Home' ? 0 : event.key === 'End' ? props.buckets.length - 1 : Math.max(0, Math.min(props.buckets.length - 1, index + step));
  focusedBucket.value = target;
  (event.currentTarget as HTMLElement).parentElement?.querySelector<HTMLButtonElement>(`[data-bucket-index="${target}"]`)?.focus();
}

const maxCount = computed(() =>
  props.buckets.reduce((max, bucket) => Math.max(max, bucket.count), 0),
);

/** X 轴标签抽稀：最多 ~24 个刻度。 */
const labelEvery = computed(() => Math.max(1, Math.ceil(props.buckets.length / 24)));

function barHeightRatio(bucket: TimelineBucket): number {
  if (maxCount.value === 0) return 0;
  return Math.max(bucket.count / maxCount.value, 0.015);
}

function tooltipOf(bucket: TimelineBucket): string {
  const { start, end } = bucketRange(bucket.date_str, props.unit, props.tzHours);
  return tr("{0}\n{1}\n{2} 条记录", { 0: bucket.date_str, 1: formatRange(start, end, props.tzHours), 2: bucket.count });
}

function shouldLabel(index: number): boolean {
  return index % labelEvery.value === 0 || index === props.buckets.length - 1;
}
</script>

<template>
  <div class="tl-chart" :style="{ height: `${height + 22}px` }">
    <div v-if="buckets.length === 0" class="tl-chart-empty">{{ tr("当前过滤条件下无时间线数据") }}</div>
    <template v-else>
      <div class="tl-chart-meta">
        <span class="tl-chart-max mono">{{ tr("峰值") }} {{ maxCount.toLocaleString() }}</span>
        <span v-if="truncated" class="tl-chart-truncated">{{ tr("桶数已达上限，数据被截断") }}</span>
      </div>
      <div class="tl-bars" :style="{ height: `${height}px` }" role="group" :aria-label="tr('时间线柱状图')">
        <button
          v-for="(bucket, index) in buckets"
          :key="bucket.date_str"
          type="button"
          class="tl-bar"
          :data-bucket-index="index" :tabindex="focusedBucket === index ? 0 : -1"
          :style="{ '--bar-index': index }"
          :title="tooltipOf(bucket)"
          :aria-label="tr('{0}，{1} 条事件', { 0: bucket.date_str, 1: bucket.count })"
          @focus="focusedBucket = index" @keydown="onBarKeydown($event, index)"
          @click="emit('select', bucket)"
        >
          <span class="tl-bar-count mono" :class="{ visible: bucket.count / (maxCount || 1) > 0.55 }">{{
            bucket.count >= 10000 ? `${Math.round(bucket.count / 1000)}k` : bucket.count.toLocaleString()
          }}</span>
          <span
            class="tl-bar-fill"
            :style="{ height: `${Math.round(barHeightRatio(bucket) * 100)}%` }"
          ></span>
          <span class="tl-bar-label mono" :class="{ visible: shouldLabel(index) }">{{
            bucketShortLabel(bucket.date_str, unit, tr)
          }}</span>
        </button>
      </div>
    </template>
  </div>
</template>

<style scoped>
.tl-chart {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
  overflow-x: auto;
}

.tl-chart-empty {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
  color: var(--text-secondary);
  font-size: 12px;
}

.tl-chart-meta {
  display: flex;
  align-items: center;
  gap: 10px;
  font-size: 11px;
  color: var(--text-secondary);
  min-height: 14px;
}

.tl-chart-truncated {
  color: var(--sev-warn-fg);
}

.tl-bars {
  display: flex;
  align-items: stretch;
  gap: 4px;
  min-width: 0;
  border-bottom: 1px solid var(--border-color);
}

.tl-bar {
  flex: 1 1 0;
  min-width: 24px;
  max-width: 42px;
  display: flex;
  flex-direction: column;
  justify-content: flex-end;
  align-items: stretch;
  background: none;
  border: none;
  padding: 0;
  cursor: pointer;
  position: relative;
}

.tl-bar-fill {
  display: block;
  background: var(--primary-color);
  opacity: 0.72;
  border-radius: 2px 2px 0 0;
  transition: opacity var(--transition-fast), height var(--motion-normal) var(--ease-out);
  min-height: 2px;
  transform-origin: bottom center;
}

:root[data-motion='full'] .tl-bar-fill {
  animation: ui-grow-y var(--motion-slow) var(--ease-out) calc(min(var(--bar-index), 40) * 8ms) backwards;
}

.tl-bar:hover .tl-bar-fill,
.tl-bar:focus-visible .tl-bar-fill {
  opacity: 1;
}

.tl-bar-count {
  position: absolute;
  top: -14px;
  left: 50%;
  transform: translateX(-50%);
  font-size: 11px;
  color: var(--text-secondary);
  white-space: nowrap;
  opacity: 0;
  pointer-events: none;
}

.tl-bar-count.visible {
  opacity: 1;
}

.tl-bar-label {
  height: 18px;
  font-size: 11px;
  color: var(--text-secondary);
  text-align: center;
  white-space: nowrap;
  overflow: hidden;
  opacity: 0;
}

.tl-bar-label.visible {
  opacity: 1;
}
</style>
