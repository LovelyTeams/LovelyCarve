import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ref, shallowRef } from 'vue';
import type { SchedTaskRecord, SchedNotice } from '../../../lib/ipc/tasks';
import type { BuiltinTask } from '../model/types';
import { useUnifiedTaskCenter } from './useUnifiedTaskCenter';

const pluginTasks = shallowRef<SchedTaskRecord[]>([]);
const builtinTasks = shallowRef<BuiltinTask[]>([]);
const notices = shallowRef<SchedNotice[]>([]);
const actions = vi.hoisted(() => ({ cancel: vi.fn(), pause: vi.fn(), start: vi.fn(), restart: vi.fn(), remove: vi.fn(), refresh: vi.fn() }));
vi.mock('./useTaskCenter', () => ({ useTaskCenter: () => ({ tasks: pluginTasks, notices, loading: ref(false), error: ref(null), ...actions }) }));
vi.mock('./useBuiltinTasks', () => ({ useBuiltinTasks: () => ({ tasks: builtinTasks, now: ref(1000), refresh: vi.fn(), cancel: vi.fn() }) }));
vi.mock('../../../composables/useToast', () => ({ notifyError: vi.fn() }));

function task(id: number, imageId: string): SchedTaskRecord {
  return { id, status: 'running', evidenceId: id, info: { imageId } } as SchedTaskRecord;
}
beforeEach(() => {
  vi.clearAllMocks();
  pluginTasks.value = [task(1, 'disk-a'), task(2, 'disk-b')];
  builtinTasks.value = [{ id: 'docker:disk-b:container', kind: 'docker', imageId: 'disk-b', status: 'running', started: 0, finished: null }] as BuiltinTask[];
  notices.value = [
    { kind: 'taskTimeout', taskId: 1, evidenceId: 1, message: 'A 通知' },
    { kind: 'taskTimeout', taskId: 2, evidenceId: 2, message: 'B 通知' },
    { kind: 'allTasksFinished', taskId: null, evidenceId: null, message: '全局通知' },
  ];
});

describe('current evidence task scope', () => {
  it('shows case container loading even before an evidence session exists and across tab switches', () => {
    builtinTasks.value.push({ id: 'imageLoad:one', kind: 'imageLoad', imageId: '', status: 'running', started: 0, finished: null } as BuiltinTask);
    const imageId = ref<string | null>(null);
    const center = useUnifiedTaskCenter(imageId);
    expect(center.tasks.value.map(task => task.id)).toEqual(['imageLoad:one']);
    expect(center.activeCount.value).toBe(1);
    imageId.value = 'disk-a';
    expect(center.tasks.value.map(task => task.id)).toEqual(['plugin:1', 'imageLoad:one']);
  });
  it('switches plugin/native tasks, counts and notices together with the active tab', () => {
    const imageId = ref<string | null>('disk-a');
    const center = useUnifiedTaskCenter(imageId);
    expect(center.tasks.value.map(task => task.id)).toEqual(['plugin:1']);
    expect(center.activeCount.value).toBe(1);
    expect(center.notices.value.map(notice => notice.message)).toEqual(['A 通知']);
    imageId.value = 'disk-b';
    expect(center.tasks.value.map(task => task.id)).toEqual(['plugin:2', 'docker:disk-b:container']);
    expect(center.activeCount.value).toBe(2);
    expect(center.notices.value.map(notice => notice.message)).toEqual(['B 通知']);
    imageId.value = null;
    expect(center.tasks.value).toEqual([]);
    expect(center.activeCount.value).toBe(0);
    expect(center.notices.value).toEqual([]);
  });

  it('does not operate on the previous evidence after a tab switch', async () => {
    const imageId = ref<string | null>('disk-a');
    const center = useUnifiedTaskCenter(() => imageId.value);
    imageId.value = 'disk-b';
    await center.cancel('plugin:1');
    expect(actions.cancel).not.toHaveBeenCalled();
    await center.cancel('plugin:2');
    expect(actions.cancel).toHaveBeenCalledExactlyOnceWith(2);
  });
});
