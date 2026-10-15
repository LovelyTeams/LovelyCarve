
import { tr } from '../../../composables/useLocale';
import { computed, toValue, type MaybeRefOrGetter } from 'vue';
import type { SchedTaskRecord } from '../../../lib/ipc/tasks';
import { useTaskCenter } from './useTaskCenter';
import { useBuiltinTasks } from './useBuiltinTasks';
import { notifyError } from '../../../composables/useToast';

/** UI identity is namespaced: native task ids never collide with plugin ids. */
export interface UnifiedTask extends Omit<SchedTaskRecord, 'id'> {
  id: string;
  sourceId: number | string;
  builtin: boolean;
  cancellable: boolean;
  indeterminate: boolean;
  cancelling: boolean;
}

export function useUnifiedTaskCenter(imageId: MaybeRefOrGetter<string | null>) {
  const plugins = useTaskCenter();
  const builtin = useBuiltinTasks();
  const tasks = computed<UnifiedTask[]>(() => [
    ...plugins.tasks.value.map(task => ({ ...task, id: `plugin:${task.id}`, sourceId: task.id,
      builtin: false, cancellable: true, indeterminate: false, cancelling: false })),
    ...builtin.tasks.value.map(task => ({
      id: task.id, sourceId: task.id, builtin: true, cancellable: !!task.cancelId,
      indeterminate: task.progress === null && task.status === 'running', cancelling: task.cancelling,
      name: task.id, displayName: task.title, typeName: tr("内置任务"), evidenceId: 0,
      status: task.status, progress: task.progress ?? 0, internal: false, canPause: task.kind === 'carve', singleInstance: false,
      params: { nodeId: null, fileName: task.subject || null, platform: null, subItems: [] },
      result: { summary: task.cancelling ? tr("正在取消，请稍候…") : task.message, artifactCount: task.results, timelineCount: task.timelineRows, findingCount: 0, runId: null },
      info: { pluginId: '', analyzer: task.kind, imageId: task.imageId, trigger: task.trigger, parentTid: null, error: task.error },
      dependence: '', spendTimeSecs: Math.max(0, ((task.finished ?? builtin.now.value) - task.started) / 1000), remainTimeSecs: null,
      startAt: new Date(task.started).toISOString(), createAt: new Date(task.started).toISOString(), updateAt: '',
    })),
  ].filter(task => task.builtin && task.info.analyzer === 'imageLoad'
    || Boolean(toValue(imageId)) && task.info.imageId === toValue(imageId)));
  const notices = computed(() => plugins.notices.value.filter(notice => tasks.value.some(task =>
    !task.builtin && (notice.taskId !== null
      ? task.sourceId === notice.taskId
      : notice.evidenceId !== null && task.evidenceId === notice.evidenceId))));
  const activeCount = computed(() => tasks.value.filter(task => ['created', 'queued', 'running'].includes(task.status)).length);
  async function refresh() { await Promise.all([plugins.refresh(), builtin.refresh()]); }
  async function act(id: string, action: 'cancel' | 'pause' | 'start' | 'restart' | 'remove') {
    const task = tasks.value.find(task => task.id === id);
    if (!task) return;
    if (!task.builtin) return plugins[action](task.sourceId as number);
    try {
      if (action === 'cancel' && task.cancellable) await builtin.cancel(id);
      else if ((action === 'pause' || action === 'start') && task.canPause) await builtin[action](id);
    } catch (error) { notifyError({ cancel: tr("取消任务失败"), pause: tr("暂停任务失败"), start: tr("继续任务失败"), restart: tr("重启任务失败"), remove: tr("删除任务失败") }[action], String(error)); }
  }
  return { tasks, activeCount, refresh, notices, loading: plugins.loading, error: plugins.error,
    cancel: (id: string) => act(id, 'cancel'), pause: (id: string) => act(id, 'pause'),
    start: (id: string) => act(id, 'start'), restart: (id: string) => act(id, 'restart'), remove: (id: string) => act(id, 'remove') };
}
