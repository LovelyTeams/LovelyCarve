import { beforeEach, describe, expect, it, vi } from 'vitest';

const { invokeMock, listenMock, toastMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  listenMock: vi.fn(),
  toastMock: {
    notifyError: vi.fn(),
    notifyInfo: vi.fn(),
    notifySuccess: vi.fn(),
  },
}));

vi.mock('../../../lib/ipc/client', () => ({
  invokeCmd: invokeMock,
}));

vi.mock('../../../lib/ipc/events', () => ({
  onEvent: listenMock,
}));

vi.mock('../../../composables/useToast', () => toastMock);

vi.mock('../../cases/composables/useCaseSession', async () => {
  const { shallowRef } = await import('vue');
  const activeCase = shallowRef<{ dir: string } | null>(null);
  return {
    useCaseSession: () => ({ activeCase }),
  };
});

import { useCaseSession } from '../../cases/composables/useCaseSession';
import { registerTaskContext } from './taskContext';

import type { TaskEntry } from '../../../lib/ipc/taskRuntime';
import type { SchedTaskRecord } from '../../../lib/ipc/tasks';
import { pluginTaskEntry } from '../model/taskEntryView';
import { resetTaskCenterForTests, useTaskCenter } from './useTaskCenter';
import { applyTaskEntryEventForTests, cancelTaskEntry, useTaskEntries } from './useTaskEntries';
import { canCancelStatus, canRestartStatus, isTerminalStatus, taskGroupOf } from '../model/taskStatus';

function record(overrides: Partial<SchedTaskRecord> = {}): SchedTaskRecord {
  return {
    id: 1,
    name: 'org.test.plugin/demo',
    displayName: 'Demo · demo',
    typeName: 'fileanalyzer',
    evidenceId: 7,
    progress: 0,
    params: { nodeId: 'node-1', fileName: 'file.bin', systemId: null, targetLabel: null, options: {} },
    result: null,
    dependence: '',
    status: 'created',
    info: {
      pluginId: 'org.test.plugin',
      analyzer: 'demo',
      imageId: 'image-1',
      trigger: 'manually',
      parentTid: null,
      error: null,
    },
    canPause: false,
    singleInstance: true,
    internal: false,
    spendTimeSecs: null,
    remainTimeSecs: null,
    startAt: null,
    createAt: '2026-01-01T00:00:00.000Z',
    updateAt: '2026-01-01T00:00:00.000Z',
    ...overrides,
  };
}
const entry = (overrides: Partial<SchedTaskRecord> = {}): TaskEntry => pluginTaskEntry(record(overrides));

function hash(): TaskEntry {
  return {
    id: 'hash-1', imageId: 'image-1', status: 'running', cancelling: false, automatic: false, createdAtMs: 1, startedAtMs: 1,
    finishedAtMs: null, progress: null, error: null, actions: ['cancel'], source: 'builtin', kind: 'imageHash',
    view: { subject: ['MD5'], stage: null, progress: null, counts: {}, labels: {}, flags: [] },
  };
}

beforeEach(() => {
  resetTaskCenterForTests();
  registerTaskContext({ activeCase: () => useCaseSession().activeCase.value, ensureDetection: () => {} });
  invokeMock.mockReset();
  invokeMock.mockResolvedValue([]);
  listenMock.mockReset();
  listenMock.mockResolvedValue(() => {});
  toastMock.notifyError.mockReset();
  toastMock.notifyInfo.mockReset();
  toastMock.notifySuccess.mockReset();
});

describe('task status helpers', () => {
  it('maps the reference state machine to frontend groups', () => {
    expect(taskGroupOf('created')).toBe('todo');
    expect(taskGroupOf('queued')).toBe('todo');
    expect(taskGroupOf('running')).toBe('running');
    expect(taskGroupOf('offline')).toBe('offline');
    expect(isTerminalStatus('cancel')).toBe(true);
    expect(isTerminalStatus('pause')).toBe(false);
    expect(canCancelStatus('running')).toBe(true);
    expect(canCancelStatus('success')).toBe(false);
    expect(canRestartStatus('failed')).toBe(true);
    expect(canRestartStatus('queued')).toBe(false);
  });
});

describe('useTaskCenter', () => {
  it('replays changed and removed events over an older refresh snapshot', async () => {
    let resolveList!: (entries: TaskEntry[]) => void;
    invokeMock.mockImplementation(command => command === 'task_list'
      ? new Promise(resolve => { resolveList = resolve; }) : Promise.resolve([]));
    const center = useTaskCenter();
    const pending = center.refresh();
    applyTaskEntryEventForTests({ kind: 'changed', entry: entry({ id: 1, status: 'success' }) });
    applyTaskEntryEventForTests({ kind: 'removed', id: 'plugin:2' });
    resolveList([entry({ id: 1, status: 'running' }), entry({ id: 2 })]);
    await pending;
    expect(center.tasks.value).toEqual([record({ id: 1, status: 'success' })]);
  });

  it('ignores out-of-order refreshes and responses after closing a case', async () => {
    const activeCase = useCaseSession().activeCase;
    const resolvers: Array<(entries: TaskEntry[]) => void> = [];
    invokeMock.mockImplementation(command => command === 'task_list'
      ? new Promise(resolve => { resolvers.push(resolve); }) : Promise.resolve([]));
    const center = useTaskCenter();
    const first = center.refresh();
    const second = center.refresh();
    resolvers.at(-1)!([entry({ id: 2 })]); await second;
    resolvers.at(-2)!([entry({ id: 1 })]); await first;
    expect(center.tasks.value.map(task => task.id)).toEqual([2]);
    activeCase.value = { dir: 'case-a' } as typeof activeCase.value;
    activeCase.value = null;
    // The caseless list is current; the closed case's answer arrives late and is dropped.
    resolvers.at(-1)!([entry({ id: 3 })]);
    resolvers.at(-2)!([entry({ id: 4 })]);
    await Promise.resolve(); await Promise.resolve();
    expect(center.tasks.value.map(task => task.id)).toEqual([3]);
  });

  it('loads every task and the functions, then merges backend events by id', async () => {
    invokeMock.mockImplementation(async (command: string) => {
      if (command === 'task_list') return [entry({ id: 1 }), entry({ id: 2, status: 'running', progress: 40 }), hash()];
      if (command === 'sched_task_functions') return [{ function: 'org.test.plugin/demo' }];
      throw new Error(`unexpected ${command}`);
    });
    const center = useTaskCenter();
    await center.refresh();

    expect(invokeMock).toHaveBeenCalledWith('task_list', { imageId: null });
    expect(center.tasks.value.map(task => task.id)).toEqual([2, 1]);
    expect(useTaskEntries().entries.value.map(item => item.id)).toContain('hash-1');
    expect(center.activeCount.value).toBe(2);
    expect(center.runningCount.value).toBe(1);
    expect(center.functions.value).toHaveLength(1);

    applyTaskEntryEventForTests({ kind: 'changed', entry: entry({ id: 2, status: 'success', progress: 100 }) });
    expect(center.tasks.value.find(task => task.id === 2)?.status).toBe('success');
    expect(center.activeCount.value).toBe(1);

    applyTaskEntryEventForTests({ kind: 'removed', id: 'plugin:1' });
    expect(center.tasks.value.map(task => task.id)).toEqual([2]);
  });

  it('turns scheduler notices into toasts and keeps a bounded history', () => {
    const center = useTaskCenter();
    applyTaskEntryEventForTests({
      kind: 'notice',
      notice: { kind: 'taskOffline', taskId: 3, evidenceId: 7, message: 'worker 离线' },
    });
    applyTaskEntryEventForTests({
      kind: 'notice',
      notice: { kind: 'allTasksFinished', taskId: null, evidenceId: 7, message: '全部完成' },
    });

    expect(toastMock.notifyError).toHaveBeenCalledWith('任务执行器离线', 'worker 离线');
    expect(toastMock.notifySuccess).toHaveBeenCalledWith('检材任务全部完成', '全部完成');
    expect(center.notices.value.map(notice => notice.kind)).toEqual(['allTasksFinished', 'taskOffline']);
  });

  it('applies control results locally and reports failures as toasts', async () => {
    const center = useTaskCenter();
    applyTaskEntryEventForTests({ kind: 'changed', entry: entry({ id: 5, status: 'running' }) });
    invokeMock.mockResolvedValueOnce(record({ id: 5, status: 'cancel' }));

    const cancelled = await center.cancel(5);
    expect(invokeMock).toHaveBeenCalledWith('sched_task_cancel', { taskId: 5 });
    expect(cancelled?.status).toBe('cancel');
    expect(center.tasks.value[0].status).toBe('cancel');

    invokeMock.mockRejectedValueOnce(new Error('任务 5 当前状态为 cancel,不能取消'));
    expect(await center.cancel(5)).toBeNull();
    expect(toastMock.notifyError).toHaveBeenCalledWith('取消任务失败', '任务 5 当前状态为 cancel,不能取消');

    invokeMock.mockResolvedValueOnce(record({ id: 5, status: 'cancel' }));
    expect(await center.remove(5)).toBe(true);
    expect(center.tasks.value).toEqual([]);
  });

  it('creates tasks and distinguishes single-instance hits', async () => {
    const center = useTaskCenter();
    invokeMock.mockResolvedValueOnce({ task: record({ id: 9 }), created: true });
    await center.create({ function: 'org.test.plugin/demo', imageId: 'image-1', nodeId: 'node-1', fileName: 'a.db', options: { items: ['users'] } });
    expect(invokeMock).toHaveBeenCalledWith('sched_task_create', {
      function: 'org.test.plugin/demo',
      imageId: 'image-1',
      nodeId: 'node-1',
      fileName: 'a.db',
      systemId: null,
      options: { items: ['users'] },
    });
    expect(toastMock.notifyInfo).toHaveBeenCalledWith('任务已创建', 'Demo · demo · #9');

    invokeMock.mockResolvedValueOnce({ task: record({ id: 9 }), created: false });
    await center.create({ function: 'org.test.plugin/demo', imageId: 'image-1', systemId: 'linux@part:2' });
    expect(invokeMock).toHaveBeenLastCalledWith('sched_task_create', {
      function: 'org.test.plugin/demo',
      imageId: 'image-1',
      nodeId: null,
      fileName: null,
      systemId: 'linux@part:2',
      options: null,
    });
    expect(toastMock.notifyInfo).toHaveBeenLastCalledWith('任务已存在', 'Demo · demo 在同一目标上已有未完成任务 #9');
    expect(center.tasks.value).toHaveLength(1);
  });

  it('creates a batch quietly but still reports failures', async () => {
    const center = useTaskCenter();
    invokeMock.mockResolvedValueOnce({ task: record({ id: 10 }), created: true });
    expect((await center.create({ function: 'org.test.plugin/demo', imageId: 'image-1' }, { quiet: true }))?.id).toBe(10);
    expect(toastMock.notifyInfo).not.toHaveBeenCalled();

    invokeMock.mockRejectedValueOnce(new Error('plugin missing'));
    expect(await center.create({ function: 'org.test.plugin/demo', imageId: 'image-1' }, { quiet: true })).toBeNull();
    expect(toastMock.notifyError).toHaveBeenCalledWith('创建任务失败', 'plugin missing');
  });
});

it('releases a late subscription and rejects its old callback after disposal', async () => {
  let resolve!: (stop: () => void) => void; const stop = vi.fn();
  listenMock.mockReturnValueOnce(new Promise(yes => { resolve = yes; }));
  useTaskCenter(); const callback = listenMock.mock.calls[0][1]; resetTaskCenterForTests();
  const replacement = useTaskCenter(); resolve(stop);
  for (let index = 0; index < 8; index++) await Promise.resolve();
  callback({ payload: { kind: 'changed', entry: entry() } });
  expect(stop).toHaveBeenCalledOnce(); expect(replacement.tasks.value).toEqual([]);
});

it('starts following a case context registered after its first consumer', async () => {
  const release = registerTaskContext({ activeCase: () => null, ensureDetection: () => {} });
  const center = useTaskCenter(); release();
  const active = useCaseSession().activeCase; active.value = { dir: 'late-case' } as NonNullable<typeof active.value>;
  registerTaskContext({ activeCase: () => active.value, ensureDetection: () => {} });
  for (let index = 0; index < 10; index++) await Promise.resolve();
  expect(invokeMock).toHaveBeenCalledWith('task_list', { imageId: null });
  applyTaskEntryEventForTests({ kind: 'changed', entry: entry() });
  active.value = null; expect(center.tasks.value).toEqual([]);
});

it('cancels a task that registers after the click', async () => {
  useTaskEntries();
  await Promise.resolve();
  invokeMock.mockClear();
  invokeMock.mockResolvedValue(undefined);
  await cancelTaskEntry('load-1');
  applyTaskEntryEventForTests({ kind: 'changed', entry: { ...hash(), id: 'load-1', kind: 'imageLoad', imageId: null, status: 'queued' } });
  // Seen once, the task is cancelled once more; later events send nothing.
  applyTaskEntryEventForTests({ kind: 'changed', entry: { ...hash(), id: 'load-1', kind: 'imageLoad', imageId: null, status: 'running' } });
  expect(invokeMock.mock.calls).toEqual([
    ['task_cancel', { taskId: 'load-1' }],
    ['task_cancel', { taskId: 'load-1' }],
  ]);
});
