
import { tr } from '../../../composables/useLocale';
// 插件任务:可调度的分析器清单、建任务与插件任务的控制。任务行来自任务中心的
// 统一列表(useTaskEntries),这里取其中的插件任务;命令的回复先行合并,
// 随后的 `task-entry` 事件覆盖它。
import { computed, shallowRef } from 'vue';
import {
  schedTaskCancel,
  schedTaskCreate,
  schedTaskFunctions,
  schedTaskRemove,
  schedTaskRestart,
  schedTaskStart,
  type SchedFunction,
  type SchedTaskRecord,
} from '../../../lib/ipc/tasks';
import { pluginTaskEntry } from '../model/taskEntryView';
import { taskCaseContext } from './taskContext';
import { disposeTaskEntries, removeTaskEntry, upsertTaskEntry, useTaskEntries } from './useTaskEntries';
import { notifyError, notifyInfo } from '../../../composables/useToast';

const functions = shallowRef<SchedFunction[]>([]);
const functionsLoading = shallowRef(false);
const functionsError = shallowRef<string | null>(null);
let functionsGeneration = 0;

function caseKey() { return taskCaseContext()?.dir ?? null; }

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

async function refreshFunctions(): Promise<void> {
  const generation = ++functionsGeneration;
  functionsLoading.value = true;
  functionsError.value = null;
  try {
    const registry = await schedTaskFunctions();
    if (generation === functionsGeneration) functions.value = registry;
  } catch (err) {
    if (generation === functionsGeneration) functionsError.value = errorMessage(err);
  } finally {
    if (generation === functionsGeneration) functionsLoading.value = false;
  }
}

/** Workspace teardown and tests: forget the catalogue and every task. */
export function disposeTaskCenter(): void {
  ++functionsGeneration;
  functions.value = [];
  functionsLoading.value = false;
  functionsError.value = null;
  disposeTaskEntries();
}

export function useTaskCenter() {
  const store = useTaskEntries();

  const tasks = computed(() => store.entries.value
    .flatMap(entry => entry.source === 'plugin' ? [entry.job] : [])
    .sort((left, right) => right.id - left.id));
  const activeCount = computed(
    () => tasks.value.filter(task => task.status === 'created' || task.status === 'queued' || task.status === 'running').length,
  );
  const runningCount = computed(() => tasks.value.filter(task => task.status === 'running').length);
  const loading = computed(() => store.loading.value || functionsLoading.value);
  const error = computed(() => store.error.value ?? functionsError.value);

  async function refresh(): Promise<void> {
    await Promise.all([store.refresh(), refreshFunctions()]);
  }

  async function runAction(
    label: string,
    action: () => Promise<SchedTaskRecord>,
  ): Promise<SchedTaskRecord | null> {
    const directory = caseKey();
    try {
      const record = await action();
      if (directory === caseKey()) upsertTaskEntry(pluginTaskEntry(record));
      return record;
    } catch (err) {
      notifyError(tr("{0}失败", { 0: label }), errorMessage(err));
      return null;
    }
  }

  function cancel(taskId: number) {
    return runAction(tr("取消任务"), () => schedTaskCancel(taskId));
  }

  /** Offline tasks, and tasks an older version paused, run again from the start. */
  function start(taskId: number) {
    return runAction(tr("启动任务"), () => schedTaskStart(taskId));
  }

  function restart(taskId: number) {
    return runAction(tr("重启任务"), () => schedTaskRestart(taskId));
  }

  async function remove(taskId: number): Promise<boolean> {
    const directory = caseKey();
    try {
      await schedTaskRemove(taskId);
      if (directory === caseKey()) removeTaskEntry(`plugin:${taskId}`);
      return true;
    } catch (err) {
      notifyError(tr("删除任务失败"), errorMessage(err));
      return false;
    }
  }

  /** `quiet` skips the per-task confirmation toast (a batch reports once);
   *  failures are always reported. */
  async function create(args: {
    function: string;
    imageId: string;
    nodeId?: string | null;
    fileName?: string | null;
    systemId?: string | null;
    options?: Record<string, unknown> | null;
  }, options: { quiet?: boolean } = {}): Promise<SchedTaskRecord | null> {
    const directory = caseKey();
    try {
      const response = await schedTaskCreate(args);
      if (directory !== caseKey()) return response.task;
      upsertTaskEntry(pluginTaskEntry(response.task));
      if (options.quiet) {
        // The caller summarises the batch.
      } else if (response.created) {
        notifyInfo(tr("任务已创建"), `${response.task.displayName} · #${response.task.id}`);
      } else {
        notifyInfo(tr("任务已存在"), tr("{0} 在同一目标上已有未完成任务 #{1}", { 0: response.task.displayName, 1: response.task.id }));
      }
      return response.task;
    } catch (err) {
      notifyError(tr("创建任务失败"), errorMessage(err));
      return null;
    }
  }

  return {
    tasks,
    functions,
    notices: store.notices,
    loading,
    error,
    activeCount,
    runningCount,
    refresh,
    cancel,
    start,
    restart,
    remove,
    create,
  };
}

export function resetTaskCenterForTests(): void { disposeTaskCenter(); }
