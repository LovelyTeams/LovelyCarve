<template>
<div class="ui-toolbar treemap-bar">
  <div class="treemap-controls">
    <div class="ui-seg" role="group" :aria-label="tr('树图度量')">
      <button class="ui-seg-item" type="button" :aria-pressed="mode === 'bytes'" :title="tr('面积按文件逻辑大小')" @click="mode = 'bytes'">
        <HardDrive :size="13" aria-hidden="true" />{{ tr("大小") }}
      </button>
      <button class="ui-seg-item" type="button" :aria-pressed="mode === 'files'" :title="tr('面积按文件数')" @click="mode = 'files'">
        <Files :size="13" aria-hidden="true" />{{ tr("文件数") }}
      </button>
      <button class="ui-seg-item" type="button" :aria-pressed="mode === 'age'" :title="tr('面积按大小，颜色按距卷内最新写入的时间')" @click="mode = 'age'">
        <Clock3 :size="13" aria-hidden="true" />{{ tr("时间") }}
      </button>
    </div>
    <label class="ui-check" :title="tr('计入已删除的文件，以斜线标出')">
      <input v-model="includeDeleted" type="checkbox" />{{ tr("含已删除") }}
    </label>
    <div class="depth-stepper" role="group" :aria-label="tr('显示层数')">
      <button class="ui-icon-btn sm" type="button" :disabled="depth <= MIN_DEPTH" :title="tr('少显示一层 ([)')" :aria-label="tr('少显示一层')" @click="depth = Math.max(MIN_DEPTH, depth - 1)">
        <Minus :size="13" />
      </button>
      <span class="depth-value" aria-live="polite">{{ tr("{0} 层", { 0: depth }) }}</span>
      <button class="ui-icon-btn sm" type="button" :disabled="depth >= MAX_DEPTH" :title="tr('多显示一层 (])')" :aria-label="tr('多显示一层')" @click="depth = Math.min(MAX_DEPTH, depth + 1)">
        <Plus :size="13" />
      </button>
    </div>
    <template v-if="mode === 'age'">
      <span class="ui-divider-v" aria-hidden="true"></span>
      <ul class="treemap-legend" :aria-label="tr('图例')">
        <li v-for="item in legend" :key="item.key" class="legend-item">
          <span class="legend-swatch" :style="{ background: item.swatch }" aria-hidden="true"></span>{{ item.label }}
        </li>
      </ul>
    </template>
  </div>
  <div class="ui-toolbar-actions">
    <FileViewSwitch v-model="view" :treemap-available="true" />
  </div>
</div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed } from 'vue';
import { Clock3, Files, HardDrive, Minus, Plus } from 'lucide-vue-next';
import FileViewSwitch from './FileViewSwitch.vue';
import type { TreemapMode } from '../composables/useSizeTree';
import { TREEMAP_MAX_DEPTH as MAX_DEPTH, TREEMAP_MIN_DEPTH as MIN_DEPTH, type FileViewMode } from '../model/treemap';
import { AGE_BUCKETS, ageBucketLabel } from '../model/treemapAge';
import { ageSwatch } from '../model/treemapPalette';

const mode = defineModel<TreemapMode>('mode', { required: true });
const depth = defineModel<number>('depth', { required: true });
const includeDeleted = defineModel<boolean>('includeDeleted', { required: true });
const view = defineModel<FileViewMode>('view', { required: true });

/** 只有「时间」着色需要图例：由新到旧。 */
const legend = computed(() => [
  ...AGE_BUCKETS.map((_limit, bucket) => ({ key: `age-${bucket}`, swatch: ageSwatch(bucket), label: ageBucketLabel(bucket, tr) })),
  { key: 'age-none', swatch: ageSwatch(null), label: tr('时间未知') },
]);
</script>

<style scoped>
/* ── 树图工具条 ──
   与列表的筛选条同一位置、同高：左侧是度量、已删除、层数与图例，右侧是视图切换。
   宽度不够时左侧换行。 */
.treemap-bar {
  flex-wrap: nowrap;
  gap: 12px;
}

.treemap-controls {
  display: flex;
  flex: 1;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px 12px;
  min-width: 0;
}

.treemap-controls .ui-seg-item {
  gap: 6px;
}

.depth-stepper {
  display: inline-flex;
  align-items: center;
  gap: 2px;
}

.depth-value {
  min-width: 36px;
  color: var(--text-primary);
  font-size: 12px;
  font-variant-numeric: tabular-nums;
  text-align: center;
}

.treemap-legend {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 4px 12px;
  min-width: 0;
  margin: 0;
  padding: 0;
  list-style: none;
}

.legend-item {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: var(--text-secondary);
  font-size: 12px;
  white-space: nowrap;
}

.legend-swatch {
  flex-shrink: 0;
  width: 10px;
  height: 10px;
  border-radius: 3px;
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--text-primary) 12%, transparent);
}

.treemap-bar .ui-toolbar-actions {
  align-self: flex-start;
  flex-wrap: nowrap;
}
</style>
