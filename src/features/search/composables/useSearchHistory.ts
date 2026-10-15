
import { tr } from '../../../composables/useLocale';
import { computed, onScopeDispose, reactive, ref, shallowRef, watch } from 'vue';
import { globalSearchHistoryDelete, globalSearchHistoryList, type GlobalSearchHistoryEntry, type GlobalSearchProgress } from '../../../lib/ipc/search';
import { taskCancel } from '../../../lib/ipc/taskRuntime';
import { confirm } from '../../../composables/useConfirm';
import { notifyError, notifyInfo } from '../../../composables/useToast';
import { taskDisplayTitle } from '../model/searchResults';
import { searchProgressPercent } from '../model/searchTaskView';

export function useSearchHistory(options: { imageId: () => string }) {
  const historyVisible = ref(false);
  const tasks = shallowRef<GlobalSearchHistoryEntry[]>([]);
  const loadingTasks = ref(false);
  const historyError = ref('');
  const selectedSearchId = ref<string | null>(null);
  const deletingTaskId = ref<string | null>(null);
  const cancelling = ref(false);
  const runningProgress = reactive(new Map<string, GlobalSearchProgress>());
  let contextGeneration = 0;
  let requestGeneration = 0;
  let deleteGeneration = 0;
  let cancelGeneration = 0;
  let disposed = false;
  const current = (generation: number) => !disposed && generation === contextGeneration;
  const selectedTask = computed(
    () => tasks.value.find(task => task.search_id === selectedSearchId.value) ?? null,
  );
  const selectedRunning = computed(() => selectedTask.value?.status === 'running');
  const selectedProgress = computed(() =>
    selectedSearchId.value ? runningProgress.get(selectedSearchId.value) ?? null : null,
  );

  const progressPercent = computed(() => searchProgressPercent(selectedProgress.value));

  const filesDoneLabel = computed(() =>
    (selectedProgress.value?.files_done ?? 0).toLocaleString(),
  );
  const filesTotalLabel = computed(() =>
    (selectedProgress.value?.files_total ?? selectedTask.value?.files_total ?? 0).toLocaleString(),
  );
  const totalMatchesLabel = computed(() =>
    (selectedProgress.value?.hits ?? selectedTask.value?.total_matches ?? 0).toLocaleString(),
  );
  const scannedLabel = computed(() => {
    if (selectedRunning.value) return filesDoneLabel.value;
    return (selectedTask.value?.files_scanned ?? 0).toLocaleString();
  });
  /** 标题栏的模式标签：关键词在前、正则在后，最多显示 4 个，其余折成 +N。 */
  const selectedPatterns = computed(() => {
    const task = selectedTask.value;
    const patterns = task
      ? [...task.keywords.map(text => ({ text, regex: false })), ...task.regexes.map(text => ({ text, regex: true }))]
      : [];
    return { visible: patterns.slice(0, 4), hidden: Math.max(0, patterns.length - 4) };
  });
  const matchedFileCountLabel = computed(() =>
    (
      selectedProgress.value?.matched_files ?? selectedTask.value?.matched_file_count ?? 0
    ).toLocaleString(),
  );


  const searchStatusTone = computed(() => ({ running: 'info', success: 'success', cancelled: '', failed: 'danger' } as Record<string, string>)[selectedTask.value?.status ?? ''] ?? '');

  async function refreshTasks(selectSearchId?: string) {
    if (disposed) return;
    const context = contextGeneration;
    const request = ++requestGeneration;
    const imageId = options.imageId();
    const valid = () => current(context) && request === requestGeneration;
    loadingTasks.value = true;
    historyError.value = '';
    try {
      const entries = await globalSearchHistoryList({ imageId, limit: 50 });
      if (!valid()) return;
      for (const id of runningProgress.keys()) {
        if (!entries.some(task => task.search_id === id && task.status === 'running')) runningProgress.delete(id);
      }
      if (entries.some(entry => !tasks.value.some(task => task.search_id === entry.search_id))) historyVisible.value = true;
      tasks.value = entries;
      selectedSearchId.value = selectSearchId && entries.some(task => task.search_id === selectSearchId)
        ? selectSearchId
        : selectedSearchId.value && entries.some(task => task.search_id === selectedSearchId.value)
          ? selectedSearchId.value : entries[0]?.search_id ?? null;
    } catch (error) {
      if (valid()) historyError.value = String(error);
    } finally {
      if (valid()) loadingTasks.value = false;
    }
  }
  function selectTask(task: GlobalSearchHistoryEntry) { selectedSearchId.value = task.search_id; }
  function acceptProgress(progress: GlobalSearchProgress): boolean {
    if (disposed || progress.image_id !== options.imageId()) return false;
    const task = tasks.value.find(task => task.search_id === progress.search_id);
    // A delayed progress event cannot reopen a persisted terminal task.
    if (task && task.status !== 'running') return false;
    runningProgress.set(progress.search_id, progress);
    return true;
  }
  async function deleteTask(task: GlobalSearchHistoryEntry) {
    if (disposed || deletingTaskId.value !== null) return;
    const context = contextGeneration;
    const imageId = options.imageId();
    const confirmed = await confirm({
      title: tr("删除整次搜索"),
      message: tr("将删除「{0}」任务及其全部命中记录，且不可恢复。", { 0: taskDisplayTitle(task.keywords, task.regexes, tr) }),
      confirmText: tr("删除"), danger: true
    });
    if (!confirmed || !current(context) || deletingTaskId.value !== null) return;
    const operation = ++deleteGeneration;
    const valid = () => current(context) && operation === deleteGeneration;
    deletingTaskId.value = task.search_id;
    try {
      const deleted = await globalSearchHistoryDelete({ imageId, searchId: task.search_id });
      if (!valid()) return;
      if (deleted) {
        runningProgress.delete(task.search_id);
        if (selectedSearchId.value === task.search_id) selectedSearchId.value = null;
        await refreshTasks();
      }
    } catch (error) { if (valid()) notifyError(tr("删除搜索任务失败"), String(error)); }
    finally { if (valid()) deletingTaskId.value = null; }
  }
  async function cancelSelectedTask() {
    const taskId = selectedSearchId.value;
    if (!taskId || cancelling.value || disposed) return;
    const context = contextGeneration;
    const operation = ++cancelGeneration;
    const valid = () => current(context) && operation === cancelGeneration && taskId === selectedSearchId.value;
    cancelling.value = true;
    try {
      await taskCancel({ taskId });
      if (valid()) notifyInfo(tr("已请求取消"), tr("扫描会在当前对象完成后停止，已命中的结果保留。"));
    } catch (error) { if (valid()) notifyError(tr("取消搜索失败"), String(error)); }
    finally { if (valid()) cancelling.value = false; }
  }
  watch(selectedSearchId, () => { cancelGeneration++; cancelling.value = false; }, { flush: 'sync' });
  watch(options.imageId, () => {
    contextGeneration++; requestGeneration++;
    runningProgress.clear(); tasks.value = []; selectedSearchId.value = null;
    deletingTaskId.value = null; cancelling.value = false; loadingTasks.value = false;
    historyError.value = '';
    void refreshTasks();
  }, { flush: 'sync' });
  onScopeDispose(() => { disposed = true; contextGeneration++; requestGeneration++; });
  void refreshTasks();
  return {
    historyVisible, tasks, loadingTasks, historyError, selectedSearchId, deletingTaskId, cancelling, runningProgress,
    selectedTask, selectedRunning, selectedProgress, progressPercent, filesDoneLabel, filesTotalLabel,
    totalMatchesLabel, scannedLabel, selectedPatterns, matchedFileCountLabel, searchStatusTone,
    refreshTasks, selectTask, acceptProgress, deleteTask, cancelSelectedTask
  };
}
