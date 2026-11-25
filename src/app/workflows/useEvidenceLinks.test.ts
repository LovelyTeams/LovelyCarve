import { beforeEach, describe, expect, it, vi } from 'vitest';
import { effectScope } from 'vue';
import type { CaseInfo, CaseListItem } from '../../lib/ipc/cases';
import type { EvidenceLinkPlace, ReceivedEvidenceLink } from '../../lib/ipc/evidenceLinks';
import { useEvidenceLinks } from './useEvidenceLinks';

const ipc = vi.hoisted(() => ({
  take: vi.fn(),
  announce: null as null | (() => void),
  caseList: vi.fn(),
  confirm: vi.fn(),
}));
vi.mock('../../lib/ipc/evidenceLinks', () => ({
  takeEvidenceLinks: ipc.take,
  onEvidenceLinkReceived: async (handler: () => void) => {
    ipc.announce = handler;
    return () => { ipc.announce = null; };
  },
}));
vi.mock('../../lib/ipc/cases', () => ({ caseList: ipc.caseList }));
vi.mock('../../composables/useConfirm', () => ({ confirm: ipc.confirm }));

function caseInfo(id: string, name: string, imageIds: string[]): CaseInfo {
  return {
    id, name, case_number: '', dir: `D:/cases/${id}`, examiner: '', description: '', locked: false,
    schema_version: 1, created_at: '', updated_at: '', snapshots: [],
    evidence: imageIds.map(image_id => ({ image_id, path: `D:/evidence/${image_id}.E01`, source_format: 'ewf', size_bytes: 1, added_at: '' })),
  };
}

function listItem(id: string, name: string, missing = false): CaseListItem {
  return { dir: `D:/cases/${id}`, id, case_number: '', name, examiner: '', locked: false, created_at: '', updated_at: '', opened_at: '', missing };
}

const registered: Record<string, CaseInfo> = {
  c1: caseInfo('c1', '甲案', ['e1']),
  c2: caseInfo('c2', '乙案', ['e2']),
};

function received(...links: (EvidenceLinkPlace | string)[]): ReceivedEvidenceLink[] {
  return links.map(link => typeof link === 'string'
    ? { url: 'lovelycarve://bad', link: null, error: link }
    : { url: `lovelycarve://open?case=${link.case_id}`, link, error: null });
}

function start(active: CaseInfo | null) {
  let current = active;
  const options = {
    activeCase: () => current,
    refreshActiveCase: vi.fn(async () => {}),
    openCase: vi.fn(async (entry: CaseListItem) => {
      current = registered[entry.id] ?? null;
      return true;
    }),
    openEvidence: vi.fn(async () => {}),
    openLocation: vi.fn(async () => {}),
    openPluginInstall: vi.fn(),
    onError: vi.fn(),
  };
  const scope = effectScope();
  scope.run(() => useEvidenceLinks(options));
  return { options, scope };
}

async function settle() {
  for (let round = 0; round < 10; round++) await new Promise(resolve => setTimeout(resolve, 0));
}

beforeEach(() => {
  vi.resetAllMocks();
  ipc.announce = null;
  ipc.take.mockResolvedValue([]);
  ipc.caseList.mockResolvedValue([listItem('c1', '甲案'), listItem('c2', '乙案')]);
});

describe('evidence links', () => {
  it('routes a release-page install without opening or switching a case', async () => {
    const plugin = { pluginId: 'org.example.demo', version: '1.2.0' };
    ipc.take.mockResolvedValueOnce([{
      url: 'lovelycarve://install-plugin?id=org.example.demo&version=1.2.0',
      link: null, plugin_install: plugin, error: null,
    } satisfies ReceivedEvidenceLink]);
    const { options } = start(registered.c1);
    await vi.waitFor(() => expect(options.openPluginInstall).toHaveBeenCalledExactlyOnceWith(plugin));
    expect(options.openCase).not.toHaveBeenCalled();
    expect(options.onError).not.toHaveBeenCalled();
  });

  it('highlights a byte range in the case that is already open', async () => {
    ipc.take.mockResolvedValueOnce(received({ case_id: 'c1', image_id: 'e1', node_id: 'ntfs:part:0:42', offset: 256, length: 12 }));
    const { options } = start(registered.c1);
    await vi.waitFor(() => expect(options.openLocation).toHaveBeenCalledExactlyOnceWith({ imageId: 'e1', nodeId: 'ntfs:part:0:42', offset: 256, length: 12 }));
    expect(ipc.caseList).not.toHaveBeenCalled();
    expect(ipc.confirm).not.toHaveBeenCalled();
  });

  it('asks before leaving another open case, then opens the linked file there', async () => {
    ipc.confirm.mockResolvedValue(true);
    ipc.take.mockResolvedValueOnce(received({ case_id: 'c2', image_id: 'e2', root_id: 'part:0', path: 'Windows/notepad.exe' }));
    const { options } = start(registered.c1);
    await vi.waitFor(() => expect(options.openLocation).toHaveBeenCalledExactlyOnceWith({ imageId: 'e2', rootId: 'part:0', path: 'Windows/notepad.exe' }));
    expect(ipc.confirm.mock.calls[0][0].title).toContain('乙案');
    expect(ipc.confirm.mock.calls[0][0].message).toContain('甲案');
    expect(options.openCase).toHaveBeenCalledExactlyOnceWith(listItem('c2', '乙案'));
  });

  it('keeps the open case when the switch is declined', async () => {
    ipc.confirm.mockResolvedValue(false);
    ipc.take.mockResolvedValueOnce(received({ case_id: 'c2', image_id: 'e2', node_id: 'n' }));
    const { options } = start(registered.c1);
    await vi.waitFor(() => expect(ipc.confirm).toHaveBeenCalled());
    await settle();
    expect(options.openCase).not.toHaveBeenCalled();
    expect(options.openLocation).not.toHaveBeenCalled();
    expect(options.onError).not.toHaveBeenCalled();
  });

  it('opens a registered case without asking when none is open', async () => {
    ipc.take.mockResolvedValueOnce(received({ case_id: 'c2', image_id: 'e2' }));
    const { options } = start(null);
    await vi.waitFor(() => expect(options.openEvidence).toHaveBeenCalledExactlyOnceWith('e2'));
    expect(options.refreshActiveCase).toHaveBeenCalled();
    expect(ipc.confirm).not.toHaveBeenCalled();
  });

  it('reports links it cannot open', async () => {
    ipc.caseList.mockResolvedValue([listItem('c1', '甲案'), listItem('c3', '丙案', true)]);
    const { options } = start(registered.c1);
    await settle();
    for (const link of [
      '链接缺少参数 case',
      { case_id: 'c9' },
      { case_id: 'c3' },
      { case_id: 'c1', image_id: 'e9', node_id: 'n' },
    ]) {
      ipc.take.mockResolvedValueOnce(received(link));
      ipc.announce?.();
      await settle();
    }
    expect(options.onError.mock.calls.map(([title, message]) => `${title}：${message}`)).toEqual([
      '链接无效：链接缺少参数 case',
      '打开链接失败：本机案件列表中没有链接所指的案件',
      '打开链接失败：无法读取案件目录：D:/cases/c3',
      '打开链接失败：链接所指的检材不在案件中',
    ]);
    expect(options.openLocation).not.toHaveBeenCalled();
  });

  it('opens only the latest of the links that arrive together or while one opens', async () => {
    let finish!: () => void;
    const { options } = start(registered.c1);
    await settle();
    options.openLocation.mockImplementationOnce(() => new Promise<void>(resolve => { finish = resolve; }));
    ipc.take.mockResolvedValueOnce(received({ case_id: 'c1', image_id: 'e1', node_id: 'a' }, { case_id: 'c1', image_id: 'e1', node_id: 'b' }));
    ipc.announce?.();
    await vi.waitFor(() => expect(options.openLocation).toHaveBeenCalledTimes(1));
    for (const node of ['c', 'd']) {
      ipc.take.mockResolvedValueOnce(received({ case_id: 'c1', image_id: 'e1', node_id: node }));
      ipc.announce?.();
      await settle();
    }
    finish();
    await vi.waitFor(() => expect(options.openLocation).toHaveBeenCalledTimes(2));
    await settle();
    expect(options.openLocation.mock.calls.map(([target]) => (target as { nodeId: string }).nodeId)).toEqual(['b', 'd']);
  });

  it('stops listening with its scope', async () => {
    const { scope } = start(registered.c1);
    await settle();
    expect(ipc.announce).not.toBeNull();
    scope.stop();
    expect(ipc.announce).toBeNull();
  });
});
