import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { effectScope, ref, type EffectScope } from 'vue';
import type { CaseInfo } from '../../lib/ipc/cases';
import type { RaidStack } from '../../lib/ipc/raid';
import { useRaidMemberOpen } from './useRaidMemberOpen';

const calls = vi.hoisted(() => ({ load: vi.fn(), detect: vi.fn(), info: vi.fn(), evidence: vi.fn(), notify: vi.fn(), error: vi.fn() }));
vi.mock('../../lib/ipc/image', () => ({ imageLoad: calls.load, imageInfoById: calls.info }));
vi.mock('../../lib/ipc/cases', () => ({ caseEvidenceList: calls.evidence }));
vi.mock('../../features/raid/api', () => ({ useRaidStack: () => ({ detect: calls.detect, error: ref(null) }) }));
vi.mock('../../composables/useToast', () => ({ notifyInfo: calls.notify, notifyError: calls.error }));
const emptyStack: RaidStack = { images: [], arrays: [], volume_groups: [], filesystems: [], diagnostics: [] };
const scopes: EffectScope[] = [];
function setup() {
  const scope = effectScope(); scopes.push(scope);
  const options = { activeCase: ref<CaseInfo | null>(null), removeEvidence: vi.fn(async () => {}),
    pickFiles: vi.fn(async () => [] as string[]), openImagePath: vi.fn(async () => ({ ok: true as const })) };
  return { options, flow: scope.run(() => useRaidMemberOpen(options))! };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}
beforeEach(() => {
  vi.resetAllMocks();
  calls.load.mockImplementation(async ({ path }: { path: string }) => ({ id: path, size_bytes: 1024 }));
  calls.detect.mockResolvedValue(emptyStack);
  calls.evidence.mockResolvedValue([]);
});
afterEach(() => { for (const scope of scopes.splice(0)) scope.stop(); vi.restoreAllMocks(); });

describe('RAID member intake', () => {
  it('shares loading and deduplication between native drops and the picker without opening tabs', async () => {
    const { flow, options } = setup();
    await flow.addRaidMemberPaths(['D:/sdb.E01', 'D:/sdb.E01', 'D:/sdc.E01']);
    expect(flow.raidMemberQueue.value.map(member => [member.path, member.status])).toEqual([
      ['D:/sdb.E01', 'loaded'], ['D:/sdc.E01', 'loaded'],
    ]);
    expect(calls.load).toHaveBeenCalledTimes(2);
    expect(calls.detect).toHaveBeenCalledExactlyOnceWith({ promptAssembled: false });
    expect(options.openImagePath).not.toHaveBeenCalled();
    options.pickFiles.mockResolvedValue(['D:/sdb.E01']);
    await flow.addRaidMemberFiles();
    expect(calls.load).toHaveBeenCalledTimes(2);
    expect(calls.notify).toHaveBeenCalledWith('成员盘已在队列中');
  });

  it('reserves members immediately and serializes rapid drops through the final detection', async () => {
    const first = deferred<{ id: string; size_bytes: number }>();
    calls.load.mockImplementationOnce(() => first.promise);
    const { flow } = setup();
    const firstDrop = flow.addRaidMemberPaths(['D:/sdb.E01']);
    await Promise.resolve();
    const secondDrop = flow.addRaidMemberPaths(['D:/sdb.E01', 'D:/sdc.E01']);
    expect(flow.raidMemberQueue.value.map(member => member.path)).toEqual(['D:/sdb.E01', 'D:/sdc.E01']);
    expect(calls.load).toHaveBeenCalledTimes(1);
    first.resolve({ id: 'D:/sdb.E01', size_bytes: 1024 });
    await Promise.all([firstDrop, secondDrop]);
    expect(calls.load.mock.calls).toEqual([[{ path: 'D:/sdb.E01' }], [{ path: 'D:/sdc.E01' }]]);
    expect(calls.detect).toHaveBeenCalledTimes(2);
    expect(flow.raidMemberQueue.value.map(member => member.status)).toEqual(['loaded', 'loaded']);
  });

  it('retains a failed member and detects the successfully loaded remainder', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    calls.load.mockRejectedValueOnce(new Error('镜像不可读取'));
    const { flow } = setup();
    await flow.addRaidMemberPaths(['D:/bad.E01', 'D:/sdc.E01']);
    expect(flow.raidMemberQueue.value.map(member => [member.status, member.message])).toEqual([
      ['failed', '镜像不可读取'], ['loaded', null],
    ]);
    expect(calls.detect).toHaveBeenCalledOnce();
  });

  it('does not publish late loads into a newly opened queue', async () => {
    const first = deferred<{ id: string; size_bytes: number }>();
    calls.load.mockImplementationOnce(() => first.promise);
    const { flow } = setup();
    const oldDrop = flow.addRaidMemberPaths(['D:/old.E01']);
    await Promise.resolve();
    await flow.openRaidMemberDialog();
    const newDrop = flow.addRaidMemberPaths(['D:/new.E01']);
    first.resolve({ id: 'old', size_bytes: 1024 });
    await Promise.all([oldDrop, newDrop]);
    expect(flow.raidMemberQueue.value.map(member => [member.path, member.id])).toEqual([['D:/new.E01', 'D:/new.E01']]);
    expect(calls.detect).toHaveBeenCalledOnce();
  });
});
