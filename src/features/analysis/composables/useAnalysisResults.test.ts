import { effectScope, nextTick } from 'vue';
import { useCaseSession } from '../../cases/composables/useCaseSession';
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

import type { AnalysisRecord, AnalysisRecordsPage, AnalysisResultNode } from '../../../lib/ipc/analysisResults';
import {
  ANALYSIS_PAGE_SIZE,
  focusAnalysisResult,
  resetAnalysisResultsForTests,
  useAnalysisResults,
} from './useAnalysisResults';

import { registerAnalysisContext } from './analysisContext';

// 按命令路由后端响应:useTaskCenter 首次接入时也会发 sched_* 引导调用,
// 不能用一次性的 mockResolvedValueOnce 占位。
let treeResponse: AnalysisResultNode[] = [];
let pageResponse: AnalysisRecordsPage = { total: 0, items: [] };
let pagesQueue: AnalysisRecordsPage[] | null = null;
let detailResponse: AnalysisRecord | null = null;

beforeEach(() => {
  vi.clearAllMocks();
  listenMock.mockReset();
  listenMock.mockReturnValue(Promise.resolve(() => {}));
  treeResponse = [];
  pageResponse = { total: 0, items: [] };
  pagesQueue = null;
  detailResponse = null;
  invokeMock.mockImplementation((command: string) => {
    switch (command) {
      case 'analysis_result_tree':
        return Promise.resolve(treeResponse);
      case 'analysis_records_page':
        if (pagesQueue && pagesQueue.length > 0) return Promise.resolve(pagesQueue.shift());
        return Promise.resolve(pageResponse);
      case 'analysis_record_detail':
        return Promise.resolve(detailResponse);
      default:
        return Promise.resolve([]);
    }
  });
  resetAnalysisResultsForTests();
  registerAnalysisContext({ caseDirectory: () => useCaseSession().activeCase.value?.dir ?? null, completedRuns: () => [] });
});

function resultNode(overrides: Partial<AnalysisResultNode> = {}): AnalysisResultNode {
  return {
    nodeId: 'image-1::linuxbaseinfo::linux.user',
    kind: 'result',
    label: '用户列表',
    imageId: 'image-1',
    typeName: '',
    analyzer: 'linuxbaseinfo',
    function: 'org.lovelycarve.linuxinfo/linuxbaseinfo',
    sourceNodeId: null,
    artifactType: 'linux.user',
    runId: 'run-1',
    completedAt: '',
    childCount: 2,
    columns: [{ key: 'linux.username', label: '用户名' }],
    children: [],
    ...overrides,
  };
}

function sampleTree(): AnalysisResultNode[] {
  return [
    {
      nodeId: 'image-1',
      kind: 'evidence',
      label: 'kali.vmdk',
      imageId: 'image-1',
      typeName: '',
      analyzer: '',
      artifactType: '',
      runId: '',
      completedAt: '',
      childCount: 1,
      columns: [],
      children: [
        {
          nodeId: 'image-1::baseinfo',
          kind: 'class',
          label: '基本信息',
          imageId: 'image-1',
          typeName: 'baseinfo',
          analyzer: '',
          artifactType: '',
          runId: '',
          completedAt: '',
          childCount: 1,
          columns: [],
          children: [
            {
              nodeId: 'image-1::baseinfo/linuxbaseinfo',
              kind: 'function',
              label: 'Linux基本信息',
              imageId: 'image-1',
              typeName: 'baseinfo',
              analyzer: 'linuxbaseinfo',
              artifactType: '',
              runId: 'run-1',
              completedAt: '2026-09-18T00:00:00Z',
              childCount: 1,
              columns: [],
              children: [resultNode()],
            },
          ],
        },
      ],
    },
  ];
}

function record(artifactId: string, username: string): AnalysisRecord {
  return {
    artifactId,
    runId: 'run-1',
    artifactType: 'linux.user',
    title: `用户 ${username}`,
    value: null,
    attributes: { 'linux.username': username },
  };
}

describe('useAnalysisResults', () => {
  it('refreshTree loads the tree, clears dirty and keeps the selection', async () => {
    const results = useAnalysisResults();
    pageResponse = { total: 2, items: [record('a1', 'root')] };
    await results.selectNode(resultNode());

    treeResponse = sampleTree();
    results.reload();
    await vi.waitFor(() => {
      expect(results.tree.value).toHaveLength(1);
      expect(results.dirty.value).toBe(false);
    });
    expect(results.selectedNode.value?.nodeId).toBe('image-1::linuxbaseinfo::linux.user');
    await vi.waitFor(() => {
      expect(results.records.value).toHaveLength(1);
    });
  });

  it('ensureFresh only pulls when dirty', async () => {
    const results = useAnalysisResults();
    results.ensureFresh();
    await vi.waitFor(() => {
      expect(results.dirty.value).toBe(false);
    });
    const callsAfterFresh = invokeMock.mock.calls.filter(
      call => call[0] === 'analysis_result_tree',
    ).length;
    expect(callsAfterFresh).toBe(1);

    results.ensureFresh();
    await Promise.resolve();
    const callsStill = invokeMock.mock.calls.filter(
      call => call[0] === 'analysis_result_tree',
    ).length;
    expect(callsStill).toBe(1);
  });

  it('selectNode pages records and applies keyword filtering', async () => {
    const results = useAnalysisResults();
    pageResponse = { total: 250, items: [record('a1', 'root')] };
    await results.selectNode(resultNode());

    expect(invokeMock).toHaveBeenCalledWith('analysis_records_page', {
      imageId: 'image-1',
      analyzer: 'linuxbaseinfo',
      runId: 'run-1',
      artifactType: 'linux.user',
      keyword: null,
      skip: 0,
      limit: ANALYSIS_PAGE_SIZE,
    });
    expect(results.pageCount.value).toBe(3);

    results.applyKeyword(' root ');
    expect(invokeMock).toHaveBeenLastCalledWith('analysis_records_page', {
      imageId: 'image-1',
      analyzer: 'linuxbaseinfo',
      runId: 'run-1',
      artifactType: 'linux.user',
      keyword: 'root',
      skip: 0,
      limit: ANALYSIS_PAGE_SIZE,
    });

    results.goToPage(2);
    expect(invokeMock).toHaveBeenLastCalledWith('analysis_records_page', {
      imageId: 'image-1',
      analyzer: 'linuxbaseinfo',
      runId: 'run-1',
      artifactType: 'linux.user',
      keyword: 'root',
      skip: 2 * ANALYSIS_PAGE_SIZE,
      limit: ANALYSIS_PAGE_SIZE,
    });
  });

  it('toggling a function node reveals its result items', async () => {
    const results = useAnalysisResults();
    treeResponse = sampleTree();
    results.reload();
    await vi.waitFor(() => {
      expect(results.tree.value).toHaveLength(1);
    });
    // 默认展开检材/分类,功能层折叠 → 功能行可见但结果项不显示。
    expect(results.treeRows.value.map(row => row.node.label)).toEqual([
      'kali.vmdk',
      '基本信息',
      'Linux基本信息',
    ]);
    const functionNode = results.tree.value[0].children[0].children[0];
    results.toggleExpanded(functionNode);
    // 展开状态是响应式的:treeRows 必须立即重算(回归:依赖未收集时点击无反应)。
    expect(results.treeRows.value.map(row => row.node.label)).toEqual([
      'kali.vmdk',
      '基本信息',
      'Linux基本信息',
      '用户列表',
    ]);
    results.toggleExpanded(functionNode);
    expect(results.treeRows.value.map(row => row.node.label)).toEqual([
      'kali.vmdk',
      '基本信息',
      'Linux基本信息',
    ]);
  });

  it('drops the selection when the node disappears after a refresh', async () => {
    const results = useAnalysisResults();
    pageResponse = { total: 2, items: [record('a1', 'root')] };
    await results.selectNode(resultNode());

    treeResponse = [];
    results.reload();
    await vi.waitFor(() => {
      expect(results.tree.value).toHaveLength(0);
    });
    expect(results.selectedNode.value).toBeNull();
    expect(results.records.value).toHaveLength(0);
  });

  it('exports every filtered page through the provided writer', async () => {
    const results = useAnalysisResults();
    await results.selectNode(resultNode());
    pagesQueue = [
      { total: 2, items: [record('a1', 'root'), record('a2', 'daemon')] },
      { total: 2, items: [] },
    ];
    const buildCsv = vi.fn(() => 'csv-body');
    const writeCsv = vi.fn();
    const count = await results.exportRecords(buildCsv, writeCsv);

    expect(count).toBe(2);
    expect(writeCsv).toHaveBeenCalledWith('csv-body');
  });

  it('exports more than 100,000 records with the captured filter and reports progress', async () => {
    const results = useAnalysisResults();
    await results.selectNode(resultNode());
    results.applyKeyword('root');
    await vi.waitFor(() => expect(results.recordsLoading.value).toBe(false));
    const total = 100_001;
    invokeMock.mockImplementation((_command, args) => Promise.resolve({
      total,
      items: Array.from({ length: Math.min(ANALYSIS_PAGE_SIZE, total - args.skip) }, (_, index) => record(String(args.skip + index), 'root')),
    }));
    const buildCsv = vi.fn((items: AnalysisRecord[]) => String(items.length));
    const writeCsv = vi.fn();
    const onProgress = vi.fn();
    expect(await results.exportRecords(buildCsv, writeCsv, { onProgress })).toBe(total);
    expect(writeCsv).toHaveBeenCalledWith('100001');
    expect(onProgress).toHaveBeenLastCalledWith({ loaded: total, total });
    expect(invokeMock).toHaveBeenLastCalledWith('analysis_records_page', expect.objectContaining({ skip: 100_000, keyword: 'root' }));
  });

  it('refuses to save when a page is empty before the declared total', async () => {
    const results = useAnalysisResults();
    await results.selectNode(resultNode());
    pagesQueue = [{ total: 2, items: [record('a1', 'root')] }, { total: 2, items: [] }];
    const writeCsv = vi.fn();
    await expect(results.exportRecords(vi.fn(), writeCsv)).rejects.toThrow('读取不完整');
    expect(writeCsv).not.toHaveBeenCalled();
  });

  it.each(['count', 'run'])('refuses to save if the %s changes between pages', async change => {
    const results = useAnalysisResults();
    await results.selectNode(resultNode());
    pagesQueue = [
      { total: 2, items: [record('a1', 'root')] },
      { total: change === 'count' ? 1 : 2, items: [{ ...record('a2', 'daemon'), runId: change === 'run' ? 'run-2' : 'run-1' }] },
    ];
    const writeCsv = vi.fn();
    await expect(results.exportRecords(vi.fn(), writeCsv)).rejects.toThrow(change === 'count' ? '数量已变化' : '已更新');
    expect(writeCsv).not.toHaveBeenCalled();
  });

  it('cancels a pending page without building or saving CSV', async () => {
    const results = useAnalysisResults();
    await results.selectNode(resultNode());
    let resolvePage!: (page: AnalysisRecordsPage) => void;
    invokeMock.mockImplementation(() => new Promise(resolve => { resolvePage = resolve; }));
    const controller = new AbortController();
    const buildCsv = vi.fn();
    const writeCsv = vi.fn();
    const request = results.exportRecords(buildCsv, writeCsv, { signal: controller.signal });
    controller.abort();
    resolvePage({ total: 1, items: [record('a1', 'root')] });
    await expect(request).rejects.toMatchObject({ name: 'AbortError' });
    expect(buildCsv).not.toHaveBeenCalled();
    expect(writeCsv).not.toHaveBeenCalled();
  });

  it('stops exporting after the case changes during a page request', async () => {
    const results = useAnalysisResults();
    await results.selectNode(resultNode());
    let resolvePage!: (page: AnalysisRecordsPage) => void;
    invokeMock.mockImplementation(() => new Promise(resolve => { resolvePage = resolve; }));
    const writeCsv = vi.fn();
    const request = results.exportRecords(vi.fn(), writeCsv);
    useCaseSession().activeCase.value = { dir: 'export-other-case' };
    resolvePage({ total: 1, items: [record('a1', 'root')] });
    await expect(request).rejects.toThrow('案件已切换');
    expect(writeCsv).not.toHaveBeenCalled();
  });
});


describe('analysis navigation and asynchronous state', () => {
  it('opens the first populated result in the current evidence automatically', async () => {
    const results = useAnalysisResults();
    treeResponse = sampleTree();
    treeResponse[0].children[0].children[0].children.unshift(resultNode({ nodeId: 'empty', label: '空结果', childCount: 0 }));
    pageResponse = { total: 1, items: [record('a1', 'root')] };
    results.setImageId('image-1');
    results.ensureFresh();
    await vi.waitFor(() => expect(results.records.value).toEqual(pageResponse.items));
    expect(results.selectedNode.value?.label).toBe('用户列表');
    results.setImageId('image-with-no-results');
    results.ensureFresh();
    await vi.waitFor(() => expect(results.dirty.value).toBe(false));
    expect(results.selectedNode.value).toBeNull();
    expect(results.records.value).toEqual([]);
  });

  it('focuses the exact input run and keeps page requests pinned to it', async () => {
    const results = useAnalysisResults();
    treeResponse = sampleTree();
    const owner = treeResponse[0].children[0].children[0];
    treeResponse[0].children[0].children.push({ ...owner, nodeId: 'file-two', sourceNodeId: 'file-two',
      children: [resultNode({ nodeId: 'file-two::users', runId: 'run-2', sourceNodeId: 'file-two' })] });
    pageResponse = { total: 250, items: [record('a1', 'root')] };
    results.setImageId('image-1');
    focusAnalysisResult('run-2');
    results.ensureFresh();
    await vi.waitFor(() => expect(results.selectedNode.value?.runId).toBe('run-2'));
    results.goToPage(1);
    expect(invokeMock).toHaveBeenLastCalledWith('analysis_records_page', expect.objectContaining({ runId: 'run-2', skip: 100 }));
  });

  it('opens the results of a requested analyzer once, then keeps the user selection', async () => {
    const results = useAnalysisResults();
    treeResponse = sampleTree();
    treeResponse[0].children.push({
      ...treeResponse[0].children[0], nodeId: 'image-1::web', label: '上网记录',
      children: [{ ...treeResponse[0].children[0].children[0], nodeId: 'image-1::web/browser', label: '浏览器', analyzer: 'browser',
        children: [resultNode({ nodeId: 'image-1::browser::history', label: '历史记录', analyzer: 'browser', function: 'org.browser/browser', artifactType: 'history' })] }],
    });
    pageResponse = { total: 1, items: [record('a1', 'root')] };
    results.setImageId('image-1');
    results.ensureFresh();
    await vi.waitFor(() => expect(results.selectedNode.value?.label).toBe('用户列表'));
    focusAnalysisResult('org.browser/browser');
    expect(results.dirty.value).toBe(true);
    results.ensureFresh();
    await vi.waitFor(() => expect(results.selectedNode.value?.label).toBe('历史记录'));
    expect(invokeMock).toHaveBeenLastCalledWith('analysis_records_page', expect.objectContaining({ analyzer: 'browser', runId: 'run-1', artifactType: 'history' }));
    await results.selectNode(resultNode());
    results.reload();
    await vi.waitFor(() => expect(results.treeLoading.value).toBe(false));
    expect(results.selectedNode.value?.label).toBe('用户列表');
    focusAnalysisResult('missing');
    results.ensureFresh();
    await vi.waitFor(() => expect(results.dirty.value).toBe(false));
    expect(results.selectedNode.value?.label).toBe('用户列表');
  });

  it('ignores an older page response after switching result items', async () => {
    const results = useAnalysisResults();
    let resolveOld!: (page: AnalysisRecordsPage) => void;
    invokeMock.mockImplementation((command, args) => {
      if (command !== 'analysis_records_page') return Promise.resolve([]);
      if (args.artifactType === 'linux.old') return new Promise(resolve => { resolveOld = resolve; });
      return Promise.resolve({ total: 1, items: [record('new', 'new')] });
    });
    const oldRequest = results.selectNode(resultNode({ nodeId: 'old', artifactType: 'linux.old' }));
    await results.selectNode(resultNode({ nodeId: 'new' }));
    resolveOld({ total: 1, items: [record('old', 'old')] });
    await oldRequest;
    expect(results.records.value).toEqual([record('new', 'new')]);
    expect(results.recordsLoading.value).toBe(false);
  });

  it('shows record loading errors and allows retry without losing selection', async () => {
    const results = useAnalysisResults();
    invokeMock.mockRejectedValueOnce(new Error('读取失败'));
    await results.selectNode(resultNode());
    expect(results.recordsError.value).toContain('读取失败');
    pageResponse = { total: 1, items: [record('a1', 'root')] };
    await results.retryRecords();
    expect(results.recordsError.value).toBeNull();
    expect(results.records.value).toEqual(pageResponse.items);
  });

  it('does not reopen a closed detail pane after a delayed response', async () => {
    const results = useAnalysisResults();
    let resolveDetail!: (value: AnalysisRecord) => void;
    invokeMock.mockImplementation(command => command === 'analysis_record_detail'
      ? new Promise(resolve => { resolveDetail = resolve; }) : Promise.resolve([]));
    const request = results.openRecord('a1');
    expect(results.detailLoading.value).toBe(true);
    results.closeDetail();
    resolveDetail(record('a1', 'root'));
    await request;
    expect(results.detail.value).toBeNull();
    expect(results.selectedRecordId.value).toBeNull();
    expect(results.detailLoading.value).toBe(false);
  });
});


describe('analysis result case lifetime', () => {
  it('keeps case reset subscriptions alive after the sidebar unmounts', async () => {
    const scope = effectScope();
    const results = scope.run(() => useAnalysisResults())!;
    treeResponse = sampleTree();
    results.setImageId('image-1');
    results.ensureFresh();
    await vi.waitFor(() => expect(results.selectedNode.value?.label).toBe('用户列表'));
    scope.stop();
    const { activeCase } = useCaseSession();
    activeCase.value = { dir: 'another-case' } as NonNullable<typeof activeCase.value>;
    await nextTick();
    expect(results.tree.value).toEqual([]);
    expect(results.selectedNode.value).toBeNull();
    expect(results.records.value).toEqual([]);
    expect(results.dirty.value).toBe(true);
    activeCase.value = null;
  });
});
