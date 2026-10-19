<template>
  <Transition name="task-hub-motion" appear>
    <aside v-show="open" id="task-hub" ref="root" class="task-hub" role="dialog" :aria-label="tr('任务中心')" tabindex="-1" @keydown.esc.stop="emit('close')">
      <header class="hub-head">
        <div class="ui-tabs hub-tabs" role="tablist" :aria-label="tr('任务中心')" @keydown="onTabKeydown">
          <button id="task-list-tab" class="ui-tab" role="tab" :aria-selected="tab === 'tasks'" :tabindex="tab === 'tasks' ? 0 : -1" aria-controls="task-list-panel" @click="emit('update:tab', 'tasks')">{{ tr("任务") }}<span v-if="activeCount" :key="activeCount" class="ui-count accent ui-bump" :aria-label="tr('{0} 项进行中', { 0: activeCount })">{{ activeCount }}</span></button>
          <button id="quick-analysis-tab" class="ui-tab" role="tab" :aria-selected="tab === 'quick'" :tabindex="tab === 'quick' ? 0 : -1" aria-controls="quick-analysis-panel" @click="emit('update:tab', 'quick')">{{ tr("快速分析") }}</button>
        </div>
        <span v-if="currentImageId" class="hub-evidence" :title="tr('当前检材：{0}', { 0: currentImagePath })"><HardDrive :size="12" /><span>{{ evidenceName || currentImageName }}</span></span>
        <button class="ui-icon-btn hub-close" :aria-label="tr('收起任务中心')" :title="tr('收起，任务继续运行')" @click="emit('close')"><X :size="16" /></button>
      </header>
      <template v-if="currentImageId || hasCase">
        <div v-show="tab === 'tasks'" id="task-list-panel" v-content-motion.rise="tab" class="hub-body" role="tabpanel" aria-labelledby="task-list-tab">
          <TaskCenterPanel ref="taskCenter" :image-id="currentImageId" @quick-analysis="emit('update:tab', 'quick')" @open-result="(panel, analyzer) => emit('openPanel', panel, analyzer)" />
        </div>
        <div v-show="tab === 'quick'" id="quick-analysis-panel" v-content-motion.rise="tab" class="hub-body" role="tabpanel" aria-labelledby="quick-analysis-tab">
          <QuickAnalysisPanel v-if="evidence" :key="evidence.image_id" :image-id="evidence.image_id" @submitted="onSubmitted" />
          <div v-else class="ui-empty hub-empty">
            <span class="ui-empty-icon accent"><HardDrive :size="24" /></span>
            <p class="ui-empty-title">{{ tr("选择案件检材，开始快速分析") }}</p>
            <p class="ui-empty-desc">{{ tr("当前镜像的后台任务可在「任务」页查看。") }}</p>
            <div class="ui-empty-actions"><button class="ui-btn sm primary" @click="emit('openCaseManager')">{{ tr("打开案件管理") }}</button></div>
          </div>
        </div>
      </template>
      <div v-else class="ui-empty hub-empty">
        <span class="ui-empty-icon accent"><HardDrive :size="24" /></span>
        <p class="ui-empty-title">{{ hasCase ? tr("打开检材，查看分析任务") : tr("打开案件，开始分析") }}</p>
        <p class="ui-empty-desc">{{ tr("任务与快速分析跟随顶部当前检材标签页。") }}</p>
        <div class="ui-empty-actions"><button class="ui-btn sm primary" @click="emit('openCaseManager')">{{ tr("打开案件管理") }}</button></div>
      </div>
    </aside>
  </Transition>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { type TaskHubTab } from '../model/types';

/** 右上角「任务中心」浮动面板：任务列表与快速分析两个页签，均限定为顶部当前检材。 */
import { computed, defineAsyncComponent, nextTick, ref } from 'vue';
import { HardDrive, X } from 'lucide-vue-next';
import AsyncPanelLoading from '../../../components/feedback/AsyncPanelLoading.vue';
import { vContentMotion } from '../../../composables/uiMotionDirectives';
import type { EvidenceRef } from '../../../lib/ipc/cases';
import type { ImageInfo } from '../../../lib/ipc/image';
import type { TaskResultPanel } from '../model/taskCenterView';

const props = defineProps<{
  open: boolean;
  tab: TaskHubTab;
  evidence: EvidenceRef | undefined;
  /** Loaded session images can have background tasks without a case ledger entry. */
  sessionImage?: Pick<ImageInfo, 'id' | 'path'> | null;
  evidenceName: string;
  hasCase: boolean;
  activeCount: number;
}>();
const currentImageId = computed(() => props.evidence?.image_id ?? props.sessionImage?.id ?? null);
const currentImagePath = computed(() => props.evidence?.path ?? props.sessionImage?.path ?? '');
const currentImageName = computed(() => currentImagePath.value.split(/[\\/]/).pop() || currentImagePath.value);
const emit = defineEmits<{
  'update:tab': [tab: TaskHubTab];
  close: [];
  openCaseManager: [];
  /** A task's result page; `analyzer` names the plugin results to open there. */
  openPanel: [panel: TaskResultPanel, analyzer: string | null];
}>();

const TaskCenterPanel = defineAsyncComponent({ loader: () => import('./TaskCenterPanel.vue'), delay: 120, loadingComponent: AsyncPanelLoading });
const QuickAnalysisPanel = defineAsyncComponent({ loader: () => import('./QuickAnalysisPanel.vue'), delay: 120, loadingComponent: AsyncPanelLoading });

const root = ref<HTMLElement | null>(null);
const taskCenter = ref<{ resetFilters: () => void } | null>(null);

function focus() {
  void nextTick(() => root.value?.focus());
}
defineExpose({ focus });

function onSubmitted() {
  taskCenter.value?.resetFilters();
  emit('update:tab', 'tasks');
  if (props.open) focus();
}

/** Arrow keys move between the two tabs, as the WAI-ARIA tabs pattern expects. */
function onTabKeydown(event: KeyboardEvent) {
  if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
  event.preventDefault();
  const next: TaskHubTab = event.key === 'Home' ? 'tasks' : event.key === 'End' ? 'quick' : props.tab === 'tasks' ? 'quick' : 'tasks';
  emit('update:tab', next);
  void nextTick(() => root.value?.querySelector<HTMLElement>(next === 'tasks' ? '#task-list-tab' : '#quick-analysis-tab')?.focus());
}
</script>

<style scoped>
/* Springs open out of the titlebar chip; folds away faster. */
.task-hub-motion-enter-active { transition: opacity var(--motion-fast) var(--ease-out), transform var(--motion-normal) var(--ease-spring); }
.task-hub-motion-leave-active { transition: opacity var(--motion-fast) var(--ease-in), transform var(--motion-fast) var(--ease-in); }
.task-hub-motion-enter-from, .task-hub-motion-leave-to { opacity: 0; transform: translateY(-8px) scale(0.96); }

.task-hub { position: fixed; top: 44px; right: 12px; z-index: 90; display: flex; flex-direction: column; width: min(440px, calc(100vw - 24px)); height: min(680px, calc(100vh - 84px)); overflow: hidden; border: 1px solid var(--border-color); border-radius: var(--radius-lg); background: var(--surface-secondary); color: var(--text-primary); box-shadow: var(--modal-shadow); outline: none; transform-origin: top right; }
.hub-head { display: flex; align-items: center; gap: 10px; flex-shrink: 0; padding: 0 8px 0 16px; border-bottom: 1px solid var(--border-color); }
.hub-tabs { align-self: stretch; flex-shrink: 0; gap: 20px; border-bottom: 0; }
.hub-tabs .ui-tab { height: 46px; }
.hub-tabs .ui-count { height: 16px; min-width: 16px; padding: 0 5px; font-size: 10px; }
.hub-evidence { display: inline-flex; align-items: center; gap: 5px; min-width: 0; height: 24px; margin-left: auto; padding: 0 9px; border-radius: 999px; background: var(--surface-tertiary); color: var(--text-secondary); font-size: 11px; font-weight: 500; }
.hub-evidence > svg { flex-shrink: 0; color: var(--primary-color); }
.hub-evidence > span { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.hub-close { flex-shrink: 0; margin-left: auto; }
.hub-evidence + .hub-close { margin-left: 0; }
.hub-body { display: flex; flex: 1; min-height: 0; overflow: hidden; }
.hub-empty { flex: 1; background: var(--surface-primary); }
</style>
