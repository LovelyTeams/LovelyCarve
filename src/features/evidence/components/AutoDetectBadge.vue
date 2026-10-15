<template>
  <div class="platform">
    <span v-if="report" class="platform-chip" :class="{ unknown: !report.platform }">
      <component :is="conclusionIcon" :size="14" aria-hidden="true" />
      <span class="platform-name">{{ report.platform ? report.display_platform_name || platformLabel(report.platform, tr) : tr("平台未识别") }}</span>
      <span class="platform-type">· {{ evidenceTypeLabel(report.evidence_type, tr) }}<template v-if="manufactor"> · {{ manufactor }}</template></span>
    </span>
    <span v-else-if="pending" class="platform-state"><Loader2 :size="13" class="spin" />{{ tr("正在识别平台…") }}</span>
    <span v-else-if="error" class="platform-state failed" :title="error"><TriangleAlert :size="13" />{{ tr("平台识别失败") }}</span>
    <span v-else class="platform-state">{{ tr("尚未识别平台") }}</span>
    <span v-if="report?.is_backup" class="ui-badge sm info">{{ tr("备份") }}</span>
    <span v-for="extra in extraPlatforms" :key="extra" class="ui-badge sm">{{ platformLabel(extra, tr) }}</span>
    <slot />
    <button
      class="ui-icon-btn sm"
      type="button"
      :disabled="pending"
      :title="pending ? tr('正在识别平台') : report ? tr('重新识别平台并回写检材') : tr('开始识别平台')"
      :aria-label="report ? tr('重新识别平台') : tr('开始识别平台')"
      @click="rerun"
    >
      <Loader2 v-if="pending" :size="13" class="spin" />
      <RefreshCw v-else :size="13" />
    </button>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, watch } from 'vue';
import { Archive, Loader2, Monitor, RefreshCw, Smartphone, TriangleAlert } from 'lucide-vue-next';
import { evidenceTypeLabel, manufactorLabel, platformLabel } from '../model/autodetectLabels';
import { runAutoDetect, useAutoDetectError, useAutoDetectPending, useAutoDetectReport } from '../composables/useAutoDetect';
import { useBuiltinTasks } from '../../tasks/api';
import { autoDetectTaskId } from '../../tasks/model/autodetectTasks';

// 镜像信息页头部的平台结论（平台 · 检材类型 · 厂商）与重新识别按钮。
// 逐分区判定与告警随回写存入案件，页面只显示结论。
const props = defineProps<{ imageId: string }>();

const report = useAutoDetectReport(() => props.imageId);
const manualPending = useAutoDetectPending(() => props.imageId);
const error = useAutoDetectError(() => props.imageId);
const { tasks } = useBuiltinTasks();
// The first task that runs when evidence joins a case counts as well, so the
// page neither starts a second run nor offers one while it is in progress.
const pending = computed(() => manualPending.value || tasks.value.some(task =>
  task.id === autoDetectTaskId(props.imageId) && (task.status === 'queued' || task.status === 'running')));

const manufactor = computed(() => manufactorLabel(report.value?.manufactor));

// 主平台之外的其它命中平台。
const extraPlatforms = computed(() =>
  (report.value?.platforms ?? []).filter(platform => platform !== report.value?.platform),
);

const conclusionIcon = computed(() => {
  if (report.value?.is_backup) return Archive;
  switch (report.value?.platform) {
    case 'android':
    case 'ios':
    case 'harmony':
    case 'huaweihmos':
    case 'othermobile':
      return Smartphone;
    default:
      return Monitor;
  }
});

function rerun() {
  if (!props.imageId || pending.value) return;
  void runAutoDetect(props.imageId);
}

// 首次打开镜像信息页且没有缓存结论、也没有进行中的首任务时自动跑一次
// (识别只读、回写幂等,与首任务偶尔重叠一次也无害)。
watch(
  () => props.imageId,
  imageId => {
    if (imageId && !report.value && !pending.value) {
      void runAutoDetect(imageId);
    }
  },
  { immediate: true },
);
</script>

<style scoped>
.platform { display: flex; flex-wrap: wrap; align-items: center; justify-content: flex-end; gap: 6px; min-width: 0; }
.platform-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  height: 28px;
  padding: 0 12px;
  border: 1px solid color-mix(in srgb, var(--sev-info-border) 60%, transparent);
  border-radius: 999px;
  background: var(--sev-info-bg);
  color: var(--sev-info-fg);
  font-size: 12px;
  white-space: nowrap;
}
.platform-chip.unknown { border-color: var(--border-color); background: var(--surface-tertiary); color: var(--text-secondary); }
.platform-chip > svg { flex-shrink: 0; }
.platform-name { font-weight: 600; }
.platform-type { overflow: hidden; text-overflow: ellipsis; }
.platform-state { display: inline-flex; align-items: center; gap: 6px; color: var(--text-secondary); font-size: 12px; }
.platform-state.failed { color: var(--sev-critical-fg); }
</style>
