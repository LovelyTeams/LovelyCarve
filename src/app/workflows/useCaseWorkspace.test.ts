import { beforeEach, describe, expect, it, vi } from 'vitest';
import { useCaseWorkspace } from './useCaseWorkspace';
import type { WorkspaceTab } from '../workspace/useTabManager';
import type { EvidenceRef } from '../../lib/ipc/cases';
import type { RaidStack } from '../../lib/ipc/raid';

const notifications = vi.hoisted(() => ({ error: vi.fn(), info: vi.fn() }));
const sources = vi.hoisted(() => ({ load: vi.fn(async (_args: { path: string }) => ({ id: 'source' })) }));
vi.mock('../../lib/ipc/image', () => ({ imageLoad: sources.load }));
vi.mock('../../composables/useToast', () => ({ notifyError: notifications.error, notifyInfo: notifications.info }));
beforeEach(() => { vi.clearAllMocks(); sources.load.mockReset().mockResolvedValue({ id: 'source' }); });

describe('case evidence restoration', () => {
  it('stages members without tabs and opens the array plus independent evidence', async () => {
    const operations: string[] = [];
    sources.load.mockImplementation(async ({ path }) => { operations.push(`stage:${path}`); return { id: 'source' }; });
    const evidence = [{ image_id: 'member', path: 'D:/member.dd' }, { image_id: 'assembled', path: 'assembled://set' }] as EvidenceRef[];
    const stack = { images: [], volume_groups: [], arrays: [{ assembled_image_id: 'assembled', members: [{ origin: { kind: 'whole', image_id: 'member' } }] }] } as unknown as RaidStack;
    const flow = useCaseWorkspace({
      evidence: () => evidence, tabs: () => [], closeTab: vi.fn(),
      detectRaid: async () => { operations.push('detect'); return stack; },
      openImagePath: async path => { operations.push(`open:${path}`); return { ok: true }; },
    });
    await flow.onOpenCaseEvidence(['assembled://set', 'D:/member.dd', 'D:/ordinary.E01']);
    expect(operations).toEqual(['stage:D:/member.dd', 'stage:D:/ordinary.E01', 'detect', 'open:D:/ordinary.E01', 'open:assembled://set']);
    expect(notifications.info).toHaveBeenCalledWith('已加载案件检材', '已加载 3 个检材');
  });

  it('continues restoration after a failed member load without opening its tab', async () => {
    sources.load.mockRejectedValueOnce(new Error('文件不存在'));
    const openImagePath = vi.fn(async () => ({ ok: true as const }));
    const detectRaid = vi.fn(async () => null);
    const flow = useCaseWorkspace({ evidence: () => [], tabs: () => [], closeTab: vi.fn(), detectRaid, openImagePath });
    await flow.onOpenCaseEvidence(['D:/missing.dd', 'assembled://set']);
    expect(detectRaid).toHaveBeenCalledWith({ promptAssembled: false });
    expect(openImagePath.mock.calls).toEqual([['assembled://set', { quiet: true }]]);
    expect(notifications.error).toHaveBeenCalledOnce();
    expect(notifications.error.mock.calls[0][1]).toContain('missing.dd');
  });
  it('stops a late source load from reopening the old case array after a switch', async () => {
    let directory = 'case-a';
    let finish!: (value: { id: string }) => void;
    sources.load.mockReturnValue(new Promise(resolve => { finish = resolve; }));
    const openImagePath = vi.fn(); const detectRaid = vi.fn();
    const flow = useCaseWorkspace({ caseDirectory: () => directory, evidence: () => [], tabs: () => [], closeTab: vi.fn(), openImagePath, detectRaid });
    const restoration = flow.onOpenCaseEvidence(['D:/member.dd', 'assembled://set']);
    directory = 'case-b'; finish({ id: 'member' });
    await restoration;
    expect(detectRaid).not.toHaveBeenCalled(); expect(openImagePath).not.toHaveBeenCalled();
    expect(notifications.info).not.toHaveBeenCalled(); expect(notifications.error).not.toHaveBeenCalled();
  });
  it('loads all real members before detecting and opening synthetic arrays', async () => {
    const operations: unknown[][] = [];
    const flow = useCaseWorkspace({
      openImagePath: async (path, options) => { operations.push(['open', path, options]); return { ok: true }; },
      detectRaid: async options => { operations.push(['detect', options]); return null; },
      tabs: () => [], closeTab: vi.fn(), evidence: () => [],
    });
    await flow.onOpenCaseEvidence(['assembled://set', 'D:/folder', 'D:/member.dd']);
    expect(operations).toEqual([
      ['detect', { promptAssembled: false }],
      ['open', 'D:/folder', { forceImage: true, quiet: true }],
      ['open', 'D:/member.dd', { forceImage: true, quiet: true }],
      ['open', 'assembled://set', { quiet: true }],
    ]);
    expect(sources.load.mock.calls).toEqual([[{ path: 'D:/folder' }], [{ path: 'D:/member.dd' }]]);
    expect(notifications.info).toHaveBeenCalledWith('已加载案件检材', '已加载 3 个检材');
  });

  it('continues after a missing source and aggregates failures once', async () => {
    const openImagePath = vi.fn(async (path: string) => path.endsWith('missing.dd')
      ? { ok: false as const, error: '文件不存在' } : { ok: true as const });
    const flow = useCaseWorkspace({ openImagePath, detectRaid: vi.fn(async () => null), tabs: () => [], closeTab: vi.fn(), evidence: () => [] });
    await flow.onOpenCaseEvidence(['D:/missing.dd', 'D:/good.dd']);
    expect(openImagePath).toHaveBeenCalledTimes(2);
    expect(notifications.error.mock.calls).toEqual([
      ['1 个检材未能打开', '已打开 1 个；以下检材可能已被移动或删除，或 RAID 成员不全：missing.dd（文件不存在）'],
    ]);
    expect(notifications.info).not.toHaveBeenCalled();
  });

  it('closes only the tab belonging to the removed evidence', () => {
    const closeTab = vi.fn();
    const tabs = [{ id: 'first', filePath: 'D:/a.dd' }, { id: 'second', filePath: 'D:/b.dd' }] as WorkspaceTab[];
    const flow = useCaseWorkspace({ openImagePath: vi.fn(), detectRaid: vi.fn(), tabs: () => tabs, closeTab, evidence: () => [] });
    flow.onCaseEvidenceRemoved('D:/b.dd');
    flow.onCaseEvidenceRemoved('D:/unknown.dd');
    expect(closeTab.mock.calls).toEqual([['second']]);
  });
});
