// 任务中心的数据源:后端统一列出的插件任务与内置任务(`task_list`),由
// `task-entry` 事件增量合并。模块级单例:订阅一次,案件切换时整表重拉;
// 调度通知(执行器离线 / 进度超时 / 全部完成 / 收尾失败)转成 toast。
import { computed, effectScope, ref, shallowRef, triggerRef, watch } from 'vue';

import { tr } from '../../../composables/useLocale';
import { notifyError, notifySuccess } from '../../../composables/useToast';
import type { UnlistenFn } from '../../../lib/ipc/events';
import { onTaskEntry, taskCancel, taskList, type TaskEntry, type TaskEntryEvent } from '../../../lib/ipc/taskRuntime';
import type { SchedNotice } from '../../../lib/ipc/tasks';
import { ensureTaskDetection, taskCaseContext } from './taskContext';

const MAX_NOTICES = 30;
/** Elapsed times of running tasks refresh this often. */
const CLOCK_TICK_MS = 1000;

const entryMap = shallowRef(new Map<string, TaskEntry>());
const notices = shallowRef<SchedNotice[]>([]);
const loading = shallowRef(false);
const error = shallowRef<string | null>(null);
const now = ref(Date.now());
/** Cancels clicked before their task registered; sent again once it appears. */
const pendingCancels = new Set<string>();
let listening: Promise<void> | null = null;
let unlisten: UnlistenFn | null = null;
let listenerGeneration = 0;
let refreshGeneration = 0;
let pendingChanges: Map<string, TaskEntry | null> | null = null;
let caseScope: ReturnType<typeof effectScope> | null = null;
let clock: ReturnType<typeof setInterval> | null = null;

function caseKey() { return taskCaseContext()?.dir ?? null; }

function isActive(entry: TaskEntry) {
  return entry.status === 'created' || entry.status === 'queued' || entry.status === 'running' || entry.status === 'pause';
}

/** Records a task the backend reported (an event, or a command's reply). */
export function upsertTaskEntry(entry: TaskEntry) {
  pendingChanges?.set(entry.id, entry);
  entryMap.value.set(entry.id, entry);
  triggerRef(entryMap);
  if (pendingCancels.delete(entry.id) && isActive(entry) && !entry.cancelling) {
    void taskCancel({ taskId: entry.id }).catch(cause => console.warn('取消任务失败', cause));
  }
}

export function removeTaskEntry(id: string) {
  pendingChanges?.set(id, null);
  if (entryMap.value.delete(id)) triggerRef(entryMap);
}

function pushNotice(notice: SchedNotice) {
  const next = [notice, ...notices.value];
  if (next.length > MAX_NOTICES) next.length = MAX_NOTICES;
  notices.value = next;
  switch (notice.kind) {
    case 'taskOffline':
      notifyError(tr("任务执行器离线"), notice.message);
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

function applyEvent(event: TaskEntryEvent) {
  switch (event.kind) {
    case 'changed':
      upsertTaskEntry(event.entry);
      break;
    case 'removed':
      removeTaskEntry(event.id);
      break;
    case 'notice':
      pushNotice(event.notice);
      break;
  }
}

/** Reloads every task; events that arrive meanwhile win over the older list. */
async function refresh(): Promise<void> {
  void ensureListening();
  const generation = ++refreshGeneration;
  const directory = caseKey();
  const changes = new Map<string, TaskEntry | null>();
  pendingChanges = changes;
  loading.value = true;
  error.value = null;
  try {
    const listed = await taskList();
    if (generation !== refreshGeneration || directory !== caseKey()) return;
    const merged = new Map(listed.map(entry => [entry.id, entry]));
    for (const [id, entry] of changes) {
      if (entry) merged.set(id, entry);
      else merged.delete(id);
    }
    entryMap.value = merged;
  } catch (cause) {
    if (generation === refreshGeneration && directory === caseKey()) error.value = cause instanceof Error ? cause.message : String(cause);
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
    // The image-info page reads detection reports from useAutoDetect; its
    // listener must be up before the first automatic detection ends.
    ensureTaskDetection();
    listening = onTaskEntry(({ payload }) => {
      if (generation === listenerGeneration) applyEvent(payload);
    })
      .then(stop => {
        if (generation !== listenerGeneration) stop();
        else unlisten = stop;
      })
      .catch(cause => {
        if (generation !== listenerGeneration) return;
        listening = null;
        console.error('subscribe task-entry failed:', cause);
      });
  }
  if (!caseScope) {
    caseScope = effectScope(true);
    caseScope.run(() => watch(
      () => taskCaseContext()?.dir ?? null,
      () => {
        // A case owns its tasks: start over, from the new case's list.
        ++refreshGeneration;
        pendingChanges = null;
        entryMap.value = new Map();
        notices.value = [];
        pendingCancels.clear();
        loading.value = false;
        error.value = null;
        void refresh();
      },
      { flush: 'sync', immediate: true },
    ));
  }
  if (!clock) {
    clock = setInterval(() => {
      if ([...entryMap.value.values()].some(entry => entry.source === 'builtin' && isActive(entry))) now.value = Date.now();
    }, CLOCK_TICK_MS);
  }
  return listening;
}

/** Cancels a task; one not registered yet is cancelled as soon as it appears. */
export async function cancelTaskEntry(id: string): Promise<void> {
  if (!entryMap.value.has(id)) pendingCancels.add(id);
  await taskCancel({ taskId: id });
}

export function useTaskEntries() {
  void ensureListening();
  return {
    entries: computed(() => [...entryMap.value.values()]),
    notices,
    loading,
    error,
    now,
    refresh,
  };
}

/** Disconnects and forgets every task (workspace teardown, tests). */
export function disposeTaskEntries(): void {
  listenerGeneration++;
  ++refreshGeneration;
  unlisten?.();
  unlisten = null;
  listening = null;
  caseScope?.stop();
  caseScope = null;
  if (clock) clearInterval(clock);
  clock = null;
  pendingChanges = null;
  pendingCancels.clear();
  entryMap.value = new Map();
  notices.value = [];
  loading.value = false;
  error.value = null;
}

/** Tests: inject one backend event. */
export function applyTaskEntryEventForTests(event: TaskEntryEvent): void {
  applyEvent(event);
}
