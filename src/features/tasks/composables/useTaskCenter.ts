
import { tr } from '../../../composables/useLocale';
// 任务中心状态(插件任务调度全生命周期的前端镜像)。
//
// 模块级单例:订阅一次 `sched-task-event`,把任务变更增量合并进列表,
// 通知(worker 离线 / 进度超时 / 检材全部完成 / 收尾动作失败)转成 toast。
// 事件已携带完整任务行,直接按 id 合并,不再回查。
import { computed, shallowRef, triggerRef, watch, effectScope } from 'vue';
import {
  onSchedTaskEvent,
  schedTaskCancel,
  schedTaskCreate,
  schedTaskFunctions,
  schedTaskList,
  schedTaskPause,
  schedTaskRemove,
  schedTaskRestart,
  schedTaskStart,
  type SchedEvent,
  type SchedFunction,
  type SchedNotice,
  type SchedTaskRecord,
} from '../../../lib/ipc/tasks';
import type { UnlistenFn } from '../../../lib/ipc/events';
import { taskCaseContext } from './taskContext';
import { notifyError, notifyInfo, notifySuccess } from '../../../composables/useToast';

const MAX_NOTICES = 30;

const taskMap = shallowRef(new Map<number, SchedTaskRecord>());
const functions = shallowRef<SchedFunction[]>([]);
const notices = shallowRef<SchedNotice[]>([]);
const loading = shallowRef(false);
const error = shallowRef<string | null>(null);
let unlisten: UnlistenFn | null = null;
let listening: Promise<void> | null = null;
let caseWatchInstalled = false;
let caseWatchScope: ReturnType<typeof effectScope> | null = null;
let refreshGeneration = 0;
let listenerGeneration = 0;
let caseRevision = 0;
let pendingChanges: Map<number, SchedTaskRecord | null> | null = null;
function caseKey() { return taskCaseContext()?.dir ?? null; }

function replaceTasks(records: SchedTaskRecord[]) {
  taskMap.value = new Map(records.map(record => [record.id, record]));
}

function upsertTask(record: SchedTaskRecord) {
  pendingChanges?.set(record.id, record);
  taskMap.value.set(record.id, record);
  triggerRef(taskMap);
}

function removeTask(taskId: number) {
  pendingChanges?.set(taskId, null);
  if (taskMap.value.delete(taskId)) triggerRef(taskMap);
}

function pushNotice(notice: SchedNotice) {
  const next = [notice, ...notices.value];
  if (next.length > MAX_NOTICES) next.length = MAX_NOTICES;
  notices.value = next;
  switch (notice.kind) {
    case 'taskOffline':
      notifyError(tr("任务 worker 离线"), notice.message);
      break;
    case 'taskTimeout':
      notifyError(tr("任务进度超时"), notice.message);
      break;
    case 'allTasksFinished':
      notifySuccess(tr("检材任务全部完成"), notice.message);
      break;
    case 'postActionFailed':
      notifyError(tr("收尾动作失败"), notice.message);
      break;
  }
}

function applyEvent(event: SchedEvent) {
  switch (event.kind) {
    case 'taskChanged':
      upsertTask(event.task);
      break;
    case 'taskRemoved':
      removeTask(event.taskId);
      break;
    case 'notice':
      pushNotice(event.notice);
      break;
  }
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

async function refresh(): Promise<void> {
  const generation = ++refreshGeneration;
  const directory = caseKey();
  const changes = new Map<number, SchedTaskRecord | null>();
  pendingChanges = changes;
  loading.value = true;
  error.value = null;
  try {
    const [records, registry] = await Promise.all([
      schedTaskList({ includeInternal: true }),
      schedTaskFunctions(),
    ]);
    if (generation !== refreshGeneration || directory !== caseKey()) return;
    const merged = new Map(records.map(record => [record.id, record]));
    for (const [id, record] of changes) {
      if (record) merged.set(id, record);
      else merged.delete(id);
    }
    replaceTasks([...merged.values()]);
    functions.value = registry;
  } catch (err) {
    if (generation === refreshGeneration && directory === caseKey()) error.value = errorMessage(err);
  } finally {
    if (generation === refreshGeneration) {
      pendingChanges = null;
      loading.value = false;
    }
  }
}

function ensureListening(): Promise<void> {
  if (!listening) {
    const generation = listenerGeneration;
    listening = onSchedTaskEvent(({ payload }) => {
      if (generation === listenerGeneration) applyEvent(payload);
    })
      .then(stop => {
        if (generation !== listenerGeneration) stop();
        else unlisten = stop;
      })
      .catch(err => {
        if (generation !== listenerGeneration) return;
        listening = null;
        console.error('subscribe sched-task-event failed:', err);
      });
  }
  if (!caseWatchInstalled) {
    caseWatchInstalled = true;
    // 案件切换:任务表随案件台账变化,整表重拉;关案后清空。
    caseWatchScope = effectScope(true);
    caseWatchScope.run(() => watch(
      () => taskCaseContext()?.dir ?? null,
      dir => {
        ++refreshGeneration;
        ++caseRevision;
        pendingChanges = null;
        replaceTasks([]);
        notices.value = [];
        functions.value = [];
        loading.value = false;
        error.value = null;
        if (dir) {
          void refresh();
        } else {
          replaceTasks([]);
          notices.value = [];
        }
      },
      { flush: 'sync' },
    ));
  }
  return listening;
}

/** 测试用:断开订阅并清空状态。 */
export function disposeTaskCenter(): void {
  listenerGeneration++;
  caseRevision++;
  unlisten?.();
  unlisten = null;
  listening = null;
  caseWatchInstalled = false;
  caseWatchScope?.stop();
  caseWatchScope = null;
  ++refreshGeneration;
  pendingChanges = null;
  replaceTasks([]);
  functions.value = [];
  notices.value = [];
  loading.value = false;
  error.value = null;
}

/** 测试用:直接注入一条后端事件。 */
export function applySchedEventForTests(event: SchedEvent): void {
  applyEvent(event);
}

export function useTaskCenter() {
  void ensureListening();

  const tasks = computed(() =>
    [...taskMap.value.values()].sort((left, right) => right.id - left.id),
  );
  const activeCount = computed(
    () => tasks.value.filter(task => task.status === 'created' || task.status === 'queued' || task.status === 'running').length,
  );
  const runningCount = computed(() => tasks.value.filter(task => task.status === 'running').length);

  async function runAction(
    label: string,
    action: () => Promise<SchedTaskRecord>,
  ): Promise<SchedTaskRecord | null> {
    const directory = caseKey();
    const revision = caseRevision;
    try {
      const record = await action();
      if (revision === caseRevision && directory === caseKey()) upsertTask(record);
      return record;
    } catch (err) {
      notifyError(tr("{0}失败", { 0: label }), errorMessage(err));
      return null;
    }
  }

  function cancel(taskId: number) {
    return runAction(tr("取消任务"), () => schedTaskCancel(taskId));
  }

  function pause(taskId: number) {
    return runAction(tr("暂停任务"), () => schedTaskPause(taskId));
  }

  function start(taskId: number) {
    return runAction(tr("启动任务"), () => schedTaskStart(taskId));
  }

  function restart(taskId: number) {
    return runAction(tr("重启任务"), () => schedTaskRestart(taskId));
  }

  async function remove(taskId: number): Promise<boolean> {
    const directory = caseKey();
    const revision = caseRevision;
    try {
      await schedTaskRemove(taskId);
      if (revision === caseRevision && directory === caseKey()) removeTask(taskId);
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
    platform?: string | null;
    subItems?: string[];
  }, options: { quiet?: boolean } = {}): Promise<SchedTaskRecord | null> {
    const directory = caseKey();
    const revision = caseRevision;
    try {
      const response = await schedTaskCreate(args);
      if (revision !== caseRevision || directory !== caseKey()) return response.task;
      upsertTask(response.task);
      if (options.quiet) {
        // The caller summarises the batch.
      } else if (response.created) {
        notifyInfo(tr("任务已创建"), `${response.task.displayName} · #${response.task.id}`);
      } else {
        notifyInfo(tr("任务已存在"), tr("{0} 已有未完成任务 #{1}", { 0: response.task.displayName, 1: response.task.id }));
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
    notices,
    loading,
    error,
    activeCount,
    runningCount,
    refresh,
    cancel,
    pause,
    start,
    restart,
    remove,
    create,
  };
}

export function resetTaskCenterForTests(): void { disposeTaskCenter(); }
