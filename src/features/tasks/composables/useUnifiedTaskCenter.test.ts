import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ref } from 'vue';
import type { BuiltinTaskEntry, TaskEntry } from '../../../lib/ipc/taskRuntime';
import type { SchedTaskRecord } from '../../../lib/ipc/tasks';
import { pluginTaskEntry } from '../model/taskEntryView';
import { registerTaskContext } from './taskContext';
import { resetTaskCenterForTests } from './useTaskCenter';
import { applyTaskEntryEventForTests, useTaskEntries } from './useTaskEntries';
import { useUnifiedTaskCenter } from './useUnifiedTaskCenter';

const ipc = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn(), error: vi.fn() }));
vi.mock('../../../lib/ipc/client', () => ({ invokeCmd: ipc.invoke }));
vi.mock('../../../lib/ipc/events', () => ({ onEvent: ipc.listen }));
vi.mock('../../../composables/useToast', () => ({ notifyError: ipc.error, notifyInfo: vi.fn(), notifySuccess: vi.fn() }));

function job(id: number, imageId: string): TaskEntry {
  return pluginTaskEntry({
    id, status: 'running', evidenceId: id, progress: 0, name: `test/${id}`, displayName: `任务 ${id}`,
    params: { nodeId: null, fileName: null, systemId: null, targetLabel: null, options: {} },
    info: { imageId, pluginId: 'test', analyzer: 'demo', trigger: 'manually', parentTid: null, error: null },
    createAt: '2026-01-01T00:00:00.000Z', updateAt: '2026-01-01T00:00:00.000Z', startAt: null,
  } as SchedTaskRecord);
}
function native(id: string, kind: BuiltinTaskEntry['kind'], imageId: string | null, extra: Partial<BuiltinTaskEntry> = {}): TaskEntry {
  return {
    id, imageId, status: 'running', cancelling: false, automatic: false, createdAtMs: 1, startedAtMs: 1, finishedAtMs: null,
    progress: null, error: null, actions: ['cancel'], source: 'builtin', kind,
    view: { subject: [], stage: null, progress: null, counts: {}, labels: {}, flags: [] }, ...extra,
  };
}

beforeEach(async () => {
  vi.clearAllMocks();
  resetTaskCenterForTests();
  registerTaskContext({ activeCase: () => null, ensureDetection: () => {} });
  ipc.invoke.mockResolvedValue([]);
  ipc.listen.mockResolvedValue(() => {});
  // The store starts from the backend's (empty) list; events then arrive.
  useTaskEntries();
  await Promise.resolve();
  for (const entry of [job(1, 'disk-a'), job(2, 'disk-b'), native('docker:disk-b', 'dockerPrepare', 'disk-b', { actions: [] })]) {
    applyTaskEntryEventForTests({ kind: 'changed', entry });
  }
  for (const notice of [
    { kind: 'taskTimeout' as const, taskId: 1, evidenceId: 1, message: 'A 通知' },
    { kind: 'taskTimeout' as const, taskId: 2, evidenceId: 2, message: 'B 通知' },
    { kind: 'allTasksFinished' as const, taskId: null, evidenceId: null, message: '全局通知' },
  ]) applyTaskEntryEventForTests({ kind: 'notice', notice });
});

describe('current evidence task scope', () => {
  it('shows evidence loading before an evidence session exists and across tab switches', () => {
    applyTaskEntryEventForTests({ kind: 'changed', entry: native('load-1', 'imageLoad', null) });
    const imageId = ref<string | null>(null);
    const center = useUnifiedTaskCenter(imageId);
    expect(center.tasks.value.map(task => task.id)).toEqual(['load-1']);
    expect(center.activeCount.value).toBe(1);
    imageId.value = 'disk-a';
    expect(center.tasks.value.map(task => task.id).sort()).toEqual(['load-1', 'plugin:1']);
  });

  it('switches plugin and native tasks, counts and notices together with the active tab', () => {
    const imageId = ref<string | null>('disk-a');
    const center = useUnifiedTaskCenter(imageId);
    expect(center.tasks.value.map(task => task.id)).toEqual(['plugin:1']);
    expect(center.activeCount.value).toBe(1);
    expect(center.notices.value.map(notice => notice.message)).toEqual(['A 通知']);
    imageId.value = 'disk-b';
    expect(center.tasks.value.map(task => task.id).sort()).toEqual(['docker:disk-b', 'plugin:2']);
    expect(center.activeCount.value).toBe(2);
    expect(center.notices.value.map(notice => notice.message)).toEqual(['B 通知']);
    imageId.value = null;
    expect(center.tasks.value).toEqual([]);
    expect(center.activeCount.value).toBe(0);
    expect(center.notices.value).toEqual([]);
  });
});

describe('task actions', () => {
  it('does not operate on the previous evidence after a tab switch', async () => {
    const imageId = ref<string | null>('disk-a');
    const center = useUnifiedTaskCenter(() => imageId.value);
    imageId.value = 'disk-b';
    ipc.invoke.mockResolvedValue({ ...((job(2, 'disk-b') as { job: SchedTaskRecord }).job), status: 'cancel' });
    await center.cancel('plugin:1');
    expect(ipc.invoke).not.toHaveBeenCalledWith('sched_task_cancel', expect.anything());
    await center.cancel('plugin:2');
    expect(ipc.invoke).toHaveBeenCalledWith('sched_task_cancel', { taskId: 2 });
  });

  it('cancels native work by its id, pauses and resumes carving by its scan and offers nothing it does not list', async () => {
    applyTaskEntryEventForTests({ kind: 'changed', entry: native('carve-task', 'carveScan', 'disk-a', { actions: ['cancel', 'pause'], scanId: 'scan-1' }) });
    const center = useUnifiedTaskCenter(() => 'disk-a');
    ipc.invoke.mockResolvedValue(undefined);
    await center.pause('carve-task');
    expect(ipc.invoke).toHaveBeenLastCalledWith('carve_scan_pause', { scanId: 'scan-1' });
    await center.resume('carve-task');
    expect(ipc.invoke).not.toHaveBeenCalledWith('carve_scan_resume', expect.anything());
    await center.cancel('carve-task');
    expect(ipc.invoke).toHaveBeenLastCalledWith('task_cancel', { taskId: 'carve-task' });
    ipc.invoke.mockRejectedValueOnce(new Error('已结束的雕刻扫描不能暂停'));
    await center.pause('carve-task');
    expect(ipc.error).toHaveBeenCalledWith('暂停任务失败', 'Error: 已结束的雕刻扫描不能暂停');
  });
});
