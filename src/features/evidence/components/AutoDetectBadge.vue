<template>
  <div class="platform">
    <span v-if="report || manualPlatform" class="platform-chip" :class="{ unknown: !platform }" :title="chipTitle">
      <component :is="conclusionIcon" :size="14" aria-hidden="true" />
      <span class="platform-name">{{ platform ? platformLabel(platform, tr) : tr("平台未识别") }}</span>
      <span v-if="conclusionType" class="platform-type">{{ conclusionType }}</span>
    </span>
    <span v-else-if="pending" class="platform-state"><Loader2 :size="13" class="spin" />{{ tr("正在识别平台…") }}</span>
    <span v-else-if="error" class="platform-state failed" :title="error"><TriangleAlert :size="13" />{{ tr("平台识别失败") }}</span>
    <span v-else class="platform-state">{{ tr("尚未识别平台") }}</span>
    <span v-for="extra in extraPlatforms" :key="extra" class="ui-badge sm">{{ platformLabel(extra, tr) }}</span>
    <span v-if="caseEvidence" class="platform-choice" :title="choiceTitle">
      <UiSelect v-model="choice" class="sm" :options="choiceOptions" :disabled="saving || caseLocked" :aria-label="tr('手动指定平台')" />
    </span>
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

import { computed, ref, watch } from 'vue';
import { Archive, Drone, Loader2, Monitor, RefreshCw, Smartphone, TriangleAlert } from 'lucide-vue-next';
import UiSelect, { type UiSelectOption } from '../../../components/ui/UiSelect.vue';
import type { EvidencePlatform } from '../../../lib/ipc/autodetect';
import { caseEvidencePlatformSet } from '../../../lib/ipc/cases';
import { notifyError } from '../../../composables/useToast';
import { EVIDENCE_PLATFORMS, evidenceTypeLabel, manufactorLabel, platformLabel } from '../model/autodetectLabels';
import { runAutoDetect, useAutoDetectError, useAutoDetectPending, useAutoDetectReport } from '../composables/useAutoDetect';
import { useTaskEntries } from '../../tasks/api';
import { useCaseSession } from '../../cases/api';

// 镜像信息页头部的平台结论（平台、厂商、检材类型）、手动指定与重新识别按钮。
// 逐分区判定与告警随回写存入案件，页面只显示结论。手动指定的平台存在案件里，
// 优先于自动识别，重新识别也不覆盖；选「自动识别」恢复自动结论。
type PlatformChoice = EvidencePlatform | 'auto';

const props = defineProps<{ imageId: string }>();

const report = useAutoDetectReport(() => props.imageId);
const manualPending = useAutoDetectPending(() => props.imageId);
const error = useAutoDetectError(() => props.imageId);
const { entries } = useTaskEntries();
const session = useCaseSession();
// The first task that runs when evidence joins a case counts as well, so the
// page neither starts a second run nor offers one while it is in progress.
// Detection is registered as `autodetect:<image id>` (one task per evidence).
const pending = computed(() => manualPending.value || entries.value.some(entry =>
  entry.id === `autodetect:${props.imageId}` && (entry.status === 'queued' || entry.status === 'running')));

const caseEvidence = computed(() =>
  session.activeCase.value?.evidence.find(evidence => evidence.image_id === props.imageId) ?? null);
const caseLocked = computed(() => !!session.activeCase.value?.locked);
const manualPlatform = computed(() => caseEvidence.value?.manual_platform ?? null);
/** 生效的平台：手动指定优先，否则取自动识别结论。 */
const platform = computed(() => manualPlatform.value ?? report.value?.platform ?? null);

const manufactor = computed(() => manufactorLabel(report.value?.manufactor));
const conclusionType = computed(() => {
  if (manualPlatform.value) return tr("手动指定");
  if (!report.value) return '';
  return [manufactor.value, report.value.is_backup ? tr("备份") : evidenceTypeLabel(report.value.evidence_type, tr)]
    .filter(Boolean).join(' ');
});
const chipTitle = computed(() => {
  const detected = report.value
    ? report.value.display_platform_name || platformLabel(report.value.platform, tr)
    : '';
  if (manualPlatform.value) return detected ? tr("自动识别：{0}", { 0: detected }) : undefined;
  return detected || undefined;
});

// 主平台之外的其它命中平台；手动指定时自动结论只留在悬停提示里。
const extraPlatforms = computed(() => manualPlatform.value
  ? []
  : (report.value?.platforms ?? []).filter(item => item !== report.value?.platform));

const conclusionIcon = computed(() => {
  if (!manualPlatform.value && report.value?.is_backup) return Archive;
  switch (platform.value) {
    case 'drone':
      return Drone;
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

const saving = ref(false);
/** 选中后、案件刷新前先显示新选择。 */
const pendingChoice = ref<PlatformChoice | null>(null);
const choiceOptions = computed<UiSelectOption<PlatformChoice>[]>(() => [
  { value: 'auto', label: tr("自动识别") },
  ...EVIDENCE_PLATFORMS.map(item => ({ value: item, label: platformLabel(item, tr) })),
]);
const choice = computed<PlatformChoice>({
  get: () => pendingChoice.value ?? manualPlatform.value ?? 'auto',
  set: value => { void applyChoice(value); },
});
const choiceTitle = computed(() => caseLocked.value
  ? tr("案件已锁定，解锁后才能修改平台")
  : tr("自动识别有误时手动指定平台；重新识别不会覆盖"));

async function applyChoice(value: PlatformChoice) {
  if (saving.value) return;
  saving.value = true;
  pendingChoice.value = value;
  try {
    await caseEvidencePlatformSet({ imageId: props.imageId, platform: value });
    await session.refreshActive();
  } catch (failure) {
    notifyError(tr("手动指定平台失败"), String(failure));
  } finally {
    pendingChoice.value = null;
    saving.value = false;
  }
}

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
.platform-choice { display: inline-flex; }
</style>
