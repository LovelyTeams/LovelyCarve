import { computed, nextTick, ref, useTemplateRef, watch, type Ref } from 'vue';
import type { CaseInfo } from '../../lib/ipc/cases';
import { useUnifiedTaskCenter } from '../../features/tasks/api';
import type { TaskHubTab } from '../../features/tasks/model/types';
import type { TaskResultPanel } from '../../features/tasks/model/taskCenterView';
import { focusAnalysisResult } from '../../features/analysis/api';
import type { ImageId } from '../../lib/ipc/ids';

/** Shell-owned task hub visibility and navigation; task tracking lives in the tasks feature. */
export function useWorkspaceTaskHub(options: {
  activeCase: Ref<CaseInfo | null>;
  imageId: Ref<ImageId | null>;
  showCaseManager: Ref<boolean>;
  showPanel: (panel: TaskResultPanel) => void;
}) {
  const { activeCase, imageId, showCaseManager } = options;
  const { activeCount: taskActiveCount, refresh: refreshTaskHub } = useUnifiedTaskCenter(imageId);
  const taskHubRef = useTemplateRef<{ focus: () => void }>('taskHubRef');
  const taskHubOpen = ref(false);
  const taskHubMounted = ref(false);
  const taskHubTab = ref<TaskHubTab>('tasks');
  const taskHubTrigger = useTemplateRef<HTMLButtonElement>('taskHubTrigger');
  const taskHubEvidence = computed(() => activeCase.value?.evidence.find(item => item.image_id === imageId.value));
  const taskHubEvidenceName = computed(() => {
    const path = taskHubEvidence.value?.path ?? '';
    return path.split(/[\\/]/).pop() || path;
  });

  function closeTaskHub() {
    taskHubOpen.value = false;
    taskHubTrigger.value?.focus();
  }
  function showTaskHub() {
    taskHubMounted.value = true;
    taskHubOpen.value = true;
    void nextTick(() => taskHubRef.value?.focus());
  }
  /** A task's result link opens its page for the same evidence; the hub steps aside. */
  function openTaskResult(panel: TaskResultPanel, analyzer: string | null) {
    if (panel === 'analysis' && analyzer) focusAnalysisResult(analyzer);
    options.showPanel(panel);
    closeTaskHub();
  }
  function toggleTaskHub() {
    if (taskHubOpen.value) { closeTaskHub(); return; }
    showTaskHub();
  }
  function openQuickAnalysis() {
    showCaseManager.value = false;
    taskHubTab.value = 'quick';
    showTaskHub();
  }
  watch(() => activeCase.value?.dir, () => {
    taskHubOpen.value = false;
    taskHubMounted.value = false;
    if (activeCase.value) void refreshTaskHub();
  }, { immediate: true });

  return {
    taskActiveCount, taskHubRef, taskHubOpen, taskHubMounted, taskHubTab, taskHubTrigger,
    taskHubEvidence, taskHubEvidenceName, closeTaskHub, openTaskResult, toggleTaskHub, openQuickAnalysis,
  };
}
