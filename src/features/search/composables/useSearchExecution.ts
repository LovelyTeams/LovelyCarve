
import { tr } from '../../../composables/useLocale';
import { onScopeDispose, watch } from 'vue';
import { trackedGlobalSearchStart } from '../../tasks/api';
import { onGlobalSearchProgress, type GlobalSearchOptions, type GlobalSearchProgress } from '../../../lib/ipc/search';
import type { UnlistenFn } from '../../../lib/ipc/events';
import { notifyError, notifyInfo } from '../../../composables/useToast';
let searchSequence = 0;
interface SearchExecutionOptions {
  imageId: () => string; selectedSearchId: () => string | null;
  refreshTasks: (searchId?: string) => Promise<void>;
  acceptProgress: (progress: GlobalSearchProgress) => boolean;
  pollIncremental: () => Promise<void>; loadFirstPage: () => Promise<void>;
}
/** The UI observes an existing tracked task; destroying it does not cancel the task. */
export function useSearchExecution(options: SearchExecutionOptions) {
  let disposed = false;
  let generation = 0;
  let selectionGeneration = 0;
  let unlisten: UnlistenFn | null = null;
  let pollTimer: number | null = null;
  let refreshTimer: number | null = null;
  const current = (captured: number) => !disposed && captured === generation;
  function clearPoll() { if (pollTimer !== null) window.clearTimeout(pollTimer); pollTimer = null; }
  function clearRefresh() { if (refreshTimer !== null) window.clearTimeout(refreshTimer); refreshTimer = null; }
  async function subscribe() {
    try {
      const cleanup = await onGlobalSearchProgress(event => {
        if (disposed || !options.acceptProgress(event.payload)) return;
        const searchId = event.payload.search_id;
        if (searchId !== options.selectedSearchId() || pollTimer !== null) return;
        const captured = generation;
        const selection = selectionGeneration;
        pollTimer = window.setTimeout(() => {
          pollTimer = null;
          if (current(captured) && selection === selectionGeneration && searchId === options.selectedSearchId()) void options.pollIncremental();
        }, 200);
      });
      if (disposed) cleanup(); else unlisten = cleanup;
    } catch (error) { if (!disposed) notifyError(tr("订阅搜索进度失败"), String(error)); }
  }
  function startSearch(configured: GlobalSearchOptions) {
    if (disposed) return;
    const imageId = options.imageId();
    const captured = generation;
    const searchId = `search-${Date.now()}-${++searchSequence}`;
    void (async () => {
      try {
        const summary = await trackedGlobalSearchStart({ imageId, searchId, options: configured });
        if (current(captured) && summary.status === 'cancelled') notifyInfo(tr("搜索已取消"), tr("已保留 {0} 条部分结果。", { 0: summary.record_count.toLocaleString() }));
      } catch (error) { if (current(captured)) notifyError(tr("全局搜索失败"), String(error)); }
      finally {
        if (current(captured)) {
          await options.refreshTasks(searchId);
          if (current(captured) && searchId === options.selectedSearchId()) await options.loadFirstPage();
        }
      }
    })();
    clearRefresh();
    refreshTimer = window.setTimeout(() => {
      refreshTimer = null;
      if (current(captured)) void options.refreshTasks(searchId);
    }, 400);
  }
  watch(options.selectedSearchId, () => { selectionGeneration++; clearPoll(); }, { flush: 'sync' });
  watch(options.imageId, () => { generation++; clearPoll(); clearRefresh(); }, { flush: 'sync' });
  onScopeDispose(() => { disposed = true; generation++; clearPoll(); clearRefresh(); unlisten?.(); unlisten = null; });
  void subscribe();
  return { startSearch };
}
