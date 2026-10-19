// 当前检材的任务中心:统一列表中属于这份检材的任务(以及尚未属于任何检材的
// 打开检材任务)、进行中的计数与相关的调度通知,和按任务提供的操作。
import { computed, toValue, type MaybeRefOrGetter } from 'vue';

import { tr } from '../../../composables/useLocale';
import { notifyError } from '../../../composables/useToast';
import { carveScanPause, carveScanResume } from '../../../lib/ipc/carve';
import type { TaskAction, TaskEntry } from '../../../lib/ipc/taskRuntime';
import { cancelTaskEntry, useTaskEntries } from './useTaskEntries';
import { useTaskCenter } from './useTaskCenter';

const FAILURE: Record<TaskAction, string> = {
  get cancel() { return tr("取消任务失败"); }, get pause() { return tr("暂停任务失败"); }, get resume() { return tr("继续任务失败"); },
  get start() { return tr("启动任务失败"); }, get restart() { return tr("重启任务失败"); }, get remove() { return tr("删除任务失败"); },
};

export function useUnifiedTaskCenter(imageId: MaybeRefOrGetter<string | null>) {
  const store = useTaskEntries();
  const plugins = useTaskCenter();
  const tasks = computed<TaskEntry[]>(() => {
    const current = toValue(imageId);
    return store.entries.value.filter(entry => entry.imageId === null || Boolean(current) && entry.imageId === current);
  });
  const notices = computed(() => store.notices.value.filter(notice => tasks.value.some(entry =>
    entry.source === 'plugin' && (notice.taskId !== null
      ? entry.job.id === notice.taskId
      : notice.evidenceId !== null && entry.job.evidenceId === notice.evidenceId))));
  const activeCount = computed(() => tasks.value.filter(entry => ['created', 'queued', 'running'].includes(entry.status)).length);

  /** Acts on a task of the current evidence only: a tab switch may have
   *  moved the list under a pending click. */
  async function act(id: string, action: TaskAction): Promise<void> {
    const entry = tasks.value.find(task => task.id === id);
    if (!entry || !entry.actions.includes(action)) return;
    if (entry.source === 'plugin') {
      // The plugin facade reports its own failures.
      switch (action) {
        case 'cancel': await plugins.cancel(entry.job.id); break;
        case 'start': await plugins.start(entry.job.id); break;
        case 'restart': await plugins.restart(entry.job.id); break;
        case 'remove': await plugins.remove(entry.job.id); break;
        case 'pause':
        case 'resume': break;
      }
      return;
    }
    try {
      switch (action) {
        case 'cancel': await cancelTaskEntry(entry.id); break;
        case 'pause': if (entry.scanId) await carveScanPause({ scanId: entry.scanId }); break;
        case 'resume': if (entry.scanId) await carveScanResume({ scanId: entry.scanId }); break;
        case 'start':
        case 'restart':
        case 'remove': break;
      }
    } catch (error) {
      notifyError(FAILURE[action], String(error));
    }
  }

  return {
    tasks, activeCount, notices, refresh: plugins.refresh, loading: store.loading, error: store.error, now: store.now,
    cancel: (id: string) => act(id, 'cancel'), pause: (id: string) => act(id, 'pause'), resume: (id: string) => act(id, 'resume'),
    start: (id: string) => act(id, 'start'), restart: (id: string) => act(id, 'restart'), remove: (id: string) => act(id, 'remove'),
  };
}
