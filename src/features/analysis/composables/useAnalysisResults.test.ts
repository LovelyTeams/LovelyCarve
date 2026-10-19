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

import type { AnalysisRecord, AnalysisRecordDetail, AnalysisRecordsPage, AnalysisResultNode, AnalysisRunAudit } from '../../../lib/ipc/analysisResults';
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
let pageResponse: AnalysisRecordsPage = { total: 0, columns: [], items: [] };
let pagesQueue: AnalysisRecordsPage[] | null = null;
let detailResponse: AnalysisRecordDetail | null = null;
let auditResponse: AnalysisRunAudit | null = null;

beforeEach(() => {
  vi.clearAllMocks();
  listenMock.mockReset();
  listenMock.mockReturnValue(Promise.resolve(() => {}));
  treeResponse = [];
  pageResponse = { total: 0, columns: [], items: [] };
  pagesQueue = null;
  detailResponse = null;
  auditResponse = null;
  invokeMock.mockImplementation((command: string) => {
    switch (command) {
      case 'analysis_result_tree':
        return Promise.resolve(treeResponse);
      case 'analysis_records_page':
        if (pagesQueue && pagesQueue.length > 0) return Promise.resolve(pagesQueue.shift());
        return Promise.resolve(pageResponse);
      case 'analysis_record_detail':
        return Promise.resolve(detailResponse);
      case 'analysis_run_get':
        return Promise.resolve(auditResponse);
      default:
        return Promise.resolve([]);
    }
  });
  resetAnalysisResultsForTests();
  registerAnalysisContext({ caseDirectory: () => useCaseSession().activeCase.value?.dir ?? null, completedRuns: () => [] });
});

function treeNode(overrides: Partial<AnalysisResultNode>): AnalysisResultNode {
  return {
    nodeId: '',
    kind: 'evidence',
    label: '',
    imageId: 'image-1',
    typeName: '',
    pluginId: '',
    analyzer: '',
    function: '',
    sourceNodeId: null,
    targetLabel: '',
    artifactType: '',
    runId: '',
    completedAt: '',
    childCount: 0,
    warningCount: 0,
    legacy: false,
    children: [],
    ...overrides,
  };
}

function resultNode(overrides: Partial<AnalysisResultNode> = {}): AnalysisResultNode {
  return treeNode({
    nodeId: 'image-1::linuxinfo/linuxbaseinfo#linux@part:2::linux.user',
    kind: 'result',
    label: '用户列表',
    typeName: 'baseinfo',
    pluginId: 'org.lovelycarve.linuxinfo',
    analyzer: 'linuxbaseinfo',
    function: 'org.lovelycarve.linuxinfo/linuxbaseinfo',
    targetLabel: 'Kali · part:2',
    artifactType: 'linux.user',
    runId: 'run-1',
    childCount: 2,
    ...overrides,
  });
}

function runNode(overrides: Partial<AnalysisResultNode> = {}): AnalysisResultNode {
  return treeNode({
    nodeId: 'image-1::linuxinfo/linuxbaseinfo#linux@part:2',
    kind: 'function',
    label: 'Linux基本信息',
    typeName: 'baseinfo',
    pluginId: 'org.lovelycarve.linuxinfo',
    analyzer: 'linuxbaseinfo',
    function: 'org.lovelycarve.linuxinfo/linuxbaseinfo',
    targetLabel: 'Kali · part:2',
    runId: 'run-1',
    completedAt: '2026-09-18T00:00:00Z',
    childCount: 1,
    warningCount: 1,
    children: [resultNode()],
    ...overrides,
  });
}

function sampleTree(): AnalysisResultNode[] {
  return [
    treeNode({
      nodeId: 'image-1',
      label: 'kali.vmdk',
      childCount: 1,
      children: [
        treeNode({ nodeId: 'image-1::baseinfo', kind: 'class', label: 'baseinfo', typeName: 'baseinfo', childCount: 1, children: [runNode()] }),
      ],
    }),
  ];
}

function record(recordId: string, username: string): AnalysisRecord {
  return {
    recordId,
    runId: 'run-1',
    recordType: 'linux.user',
    title: `用户 ${username}`,
    severity: null,
    values: { username },
    sources: [{ root: 'part:2', path: 'etc/passwd', note: '第 1 行' }],
    imageId: 'image-1',
    legacy: false,
  };
}

function page(total: number, items: AnalysisRecord[]): AnalysisRecordsPage {
  return { total, columns: [{ id: 'username', label: '用户名', kind: 'text', key: true, hidden: false, timeline: null, choices: [] }], items };
}

function auditOf(runId: string): AnalysisRunAudit {
  return {
    runId, state: 'published', pluginId: 'org.lovelycarve.linuxinfo', pluginVersion: '2.0.0', analyzer: 'linuxbaseinfo', analyzerName: 'Linux基本信息',
    target: { kind: 'system', id: 'linux@part:2', label: 'Kali · part:2' }, options: {}, profile: { hostname: 'kali' },
    inputs: [], inputCount: 0, schemaHash: 'fnv1a64:0', summary: null, recordCount: 2, recordTypes: [], warningCount: 1,
    warnings: [{ code: 'MALFORMED_LINE', message: '第 2 行字段不足', path: 'etc/passwd', nodeId: null }],
    badRegions: [], syntheticBytes: 0, startedAt: '2026-09-18T00:00:00Z', finishedAt: '2026-09-18T00:00:01Z', publishedAt: '2026-09-18T00:00:01Z',
  };
}

const pageArgs = (overrides: Record<string, unknown> = {}) => ({
  imageId: 'image-1',
  runId: 'run-1',
  recordType: 'linux.user',
  legacy: false,
  keyword: null,
  sort: null,
  descending: false,
  skip: 0,
  limit: ANALYSIS_PAGE_SIZE,
  ...overrides,
});

describe('useAnalysisResults', () => {
  it('refreshTree loads the tree, clears dirty and keeps the selection', async () => {
    const results = useAnalysisResults();
    pageResponse = page(2, [record('1', 'root')]);
    await results.selectNode(resultNode());

    treeResponse = sampleTree();
    results.reload();
    await vi.waitFor(() => {
      expect(results.tree.value).toHaveLength(1);
      expect(results.dirty.value).toBe(false);
    });
    expect(results.selectedNode.value?.nodeId).toBe(resultNode().nodeId);
    await vi.waitFor(() => {
      expect(results.records.value).toHaveLength(1);
    });
    expect(results.columns.value.map(column => column.id)).toEqual(['username']);
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
    pageResponse = page(250, [record('1', 'root')]);
    await results.selectNode(resultNode());

    expect(invokeMock).toHaveBeenCalledWith('analysis_records_page', pageArgs());
    expect(results.pageCount.value).toBe(3);

    results.applyKeyword(' root ');
    expect(invokeMock).toHaveBeenLastCalledWith('analysis_records_page', pageArgs({ keyword: 'root' }));

    results.goToPage(2);
    expect(invokeMock).toHaveBeenLastCalledWith('analysis_records_page', pageArgs({ keyword: 'root', skip: 2 * ANALYSIS_PAGE_SIZE }));
  });

  it('sorts by a column: ascending, then descending, then the stored order, back on the first page', async () => {
    const results = useAnalysisResults();
    pageResponse = page(250, [record('1', 'root')]);
    await results.selectNode(resultNode());
    results.goToPage(1);
    results.toggleSort('username');
    expect(invokeMock).toHaveBeenLastCalledWith('analysis_records_page', pageArgs({ sort: 'username' }));
    results.toggleSort('username');
    expect(invokeMock).toHaveBeenLastCalledWith('analysis_records_page', pageArgs({ sort: 'username', descending: true }));
    results.toggleSort('username');
    expect(invokeMock).toHaveBeenLastCalledWith('analysis_records_page', pageArgs());
    results.toggleSort('title');
    expect(results.sort.value).toEqual({ column: 'title', descending: false });
    // Another record type starts unsorted.
    await results.selectNode(resultNode({ nodeId: 'groups', artifactType: 'linux.group' }));
    expect(invokeMock).toHaveBeenLastCalledWith('analysis_records_page', pageArgs({ recordType: 'linux.group' }));
  });

  it('selecting a run loads its audit; legacy runs have none', async () => {
    const results = useAnalysisResults();
    auditResponse = auditOf('run-1');
    await results.selectNode(runNode());
    expect(invokeMock).toHaveBeenLastCalledWith('analysis_run_get', { imageId: 'image-1', runId: 'run-1' });
    expect(results.audit.value?.warnings[0].code).toBe('MALFORMED_LINE');
    expect(results.records.value).toEqual([]);

    const calls = invokeMock.mock.calls.length;
    await results.selectNode(runNode({ nodeId: 'legacy-run', legacy: true }));
    expect(invokeMock.mock.calls.length).toBe(calls);
    expect([results.audit.value, results.auditError.value, results.auditLoading.value]).toEqual([null, null, false]);

    auditResponse = null;
    await results.selectNode(runNode({ nodeId: 'gone' }));
    expect(results.auditError.value).toContain('已不存在');
  });

  it('opens a record with the columns of its type', async () => {
    const results = useAnalysisResults();
    detailResponse = { record: record('1', 'root'), columns: page(1, []).columns };
    await results.selectNode(resultNode());
    await results.openRecord('1');
    expect(invokeMock).toHaveBeenLastCalledWith('analysis_record_detail', { imageId: 'image-1', recordId: '1' });
    expect(results.detail.value?.values).toEqual({ username: 'root' });
    expect(results.detailColumns.value.map(column => column.id)).toEqual(['username']);
  });

  it('drops the selection when the node disappears after a refresh', async () => {
    const results = useAnalysisResults();
    pageResponse = page(2, [record('1', 'root')]);
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
      page(2, [record('1', 'root'), record('2', 'daemon')]),
      page(2, []),
    ];
    const buildCsv = vi.fn(() => 'csv-body');
    const writeCsv = vi.fn();
    const count = await results.exportRecords(buildCsv, writeCsv);

    expect(count).toBe(2);
    expect(buildCsv).toHaveBeenCalledWith([record('1', 'root'), record('2', 'daemon')], page(0, []).columns);
    expect(writeCsv).toHaveBeenCalledWith('csv-body');
  });

  it('exports more than 100,000 records with the captured filter and order and reports progress', async () => {
    const results = useAnalysisResults();
    await results.selectNode(resultNode());
    results.applyKeyword('root');
    results.toggleSort('username');
    await vi.waitFor(() => expect(results.recordsLoading.value).toBe(false));
    const total = 100_001;
    invokeMock.mockImplementation((_command, args) => Promise.resolve(page(
      total,
      Array.from({ length: Math.min(ANALYSIS_PAGE_SIZE, total - args.skip) }, (_, index) => record(String(args.skip + index), 'root')),
    )));
    const buildCsv = vi.fn((items: AnalysisRecord[]) => String(items.length));
    const writeCsv = vi.fn();
    const onProgress = vi.fn();
    expect(await results.exportRecords(buildCsv, writeCsv, { onProgress })).toBe(total);
    expect(writeCsv).toHaveBeenCalledWith('100001');
    expect(onProgress).toHaveBeenLastCalledWith({ loaded: total, total });
    expect(invokeMock).toHaveBeenLastCalledWith('analysis_records_page', expect.objectContaining({ skip: 100_000, keyword: 'root', sort: 'username' }));
  });

  it('refuses to save when a page is empty before the declared total', async () => {
    const results = useAnalysisResults();
    await results.selectNode(resultNode());
    pagesQueue = [page(2, [record('1', 'root')]), page(2, [])];
    const writeCsv = vi.fn();
    await expect(results.exportRecords(vi.fn(), writeCsv)).rejects.toThrow('读取不完整');
    expect(writeCsv).not.toHaveBeenCalled();
  });

  it.each(['count', 'run'])('refuses to save if the %s changes between pages', async change => {
    const results = useAnalysisResults();
    await results.selectNode(resultNode());
    pagesQueue = [
      page(2, [record('1', 'root')]),
      page(change === 'count' ? 1 : 2, [{ ...record('2', 'daemon'), runId: change === 'run' ? 'run-2' : 'run-1' }]),
    ];
    const writeCsv = vi.fn();
    await expect(results.exportRecords(vi.fn(), writeCsv)).rejects.toThrow(change === 'count' ? '数量已变化' : '已更新');
    expect(writeCsv).not.toHaveBeenCalled();
  });

  it('cancels a pending page without building or saving CSV', async () => {
    const results = useAnalysisResults();
    await results.selectNode(resultNode());
    let resolvePage!: (value: AnalysisRecordsPage) => void;
    invokeMock.mockImplementation(() => new Promise(resolve => { resolvePage = resolve; }));
    const controller = new AbortController();
    const buildCsv = vi.fn();
    const writeCsv = vi.fn();
    const request = results.exportRecords(buildCsv, writeCsv, { signal: controller.signal });
    controller.abort();
    resolvePage(page(1, [record('1', 'root')]));
    await expect(request).rejects.toMatchObject({ name: 'AbortError' });
    expect(buildCsv).not.toHaveBeenCalled();
    expect(writeCsv).not.toHaveBeenCalled();
  });

  it('stops exporting after the case changes during a page request', async () => {
    const results = useAnalysisResults();
    await results.selectNode(resultNode());
    let resolvePage!: (value: AnalysisRecordsPage) => void;
    invokeMock.mockImplementation(() => new Promise(resolve => { resolvePage = resolve; }));
    const writeCsv = vi.fn();
    const request = results.exportRecords(vi.fn(), writeCsv);
    useCaseSession().activeCase.value = { dir: 'export-other-case' };
    resolvePage(page(1, [record('1', 'root')]));
    await expect(request).rejects.toThrow('案件已切换');
    expect(writeCsv).not.toHaveBeenCalled();
  });
});


describe('analysis navigation and asynchronous state', () => {
  it('opens the first populated result in the current evidence automatically', async () => {
    const results = useAnalysisResults();
    treeResponse = sampleTree();
    treeResponse[0].children[0].children[0].children.unshift(resultNode({ nodeId: 'empty', label: '空结果', childCount: 0 }));
    pageResponse = page(1, [record('1', 'root')]);
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

  it('opens the run itself when it published only warnings', async () => {
    const results = useAnalysisResults();
    treeResponse = sampleTree();
    treeResponse[0].children[0].children = [runNode({ children: [], childCount: 0 })];
    auditResponse = auditOf('run-1');
    results.setImageId('image-1');
    results.ensureFresh();
    await vi.waitFor(() => expect(results.audit.value?.runId).toBe('run-1'));
    expect(results.selectedNode.value?.kind).toBe('function');
  });

  it('focuses the exact input run and keeps page requests pinned to it', async () => {
    const results = useAnalysisResults();
    treeResponse = sampleTree();
    const owner = treeResponse[0].children[0].children[0];
    treeResponse[0].children[0].children.push({ ...owner, nodeId: 'file-two', sourceNodeId: 'file-two', runId: 'run-2',
      children: [resultNode({ nodeId: 'file-two::users', runId: 'run-2', sourceNodeId: 'file-two' })] });
    pageResponse = page(250, [record('1', 'root')]);
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
    treeResponse[0].children.push(treeNode({
      nodeId: 'image-1::web', kind: 'class', label: 'browserinfo', typeName: 'browserinfo',
      children: [runNode({ nodeId: 'image-1::browser', label: '浏览器', pluginId: 'org.browser', analyzer: 'browser', function: 'org.browser/browser', targetLabel: '',
        children: [resultNode({ nodeId: 'image-1::browser::history', label: '历史记录', pluginId: 'org.browser', analyzer: 'browser', function: 'org.browser/browser', artifactType: 'history' })] })],
    }));
    pageResponse = page(1, [record('1', 'root')]);
    results.setImageId('image-1');
    results.ensureFresh();
    await vi.waitFor(() => expect(results.selectedNode.value?.label).toBe('用户列表'));
    focusAnalysisResult('org.browser/browser');
    expect(results.dirty.value).toBe(true);
    results.ensureFresh();
    await vi.waitFor(() => expect(results.selectedNode.value?.label).toBe('历史记录'));
    expect(invokeMock).toHaveBeenLastCalledWith('analysis_records_page', expect.objectContaining({ runId: 'run-1', recordType: 'history' }));
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
    let resolveOld!: (value: AnalysisRecordsPage) => void;
    invokeMock.mockImplementation((command, args) => {
      if (command !== 'analysis_records_page') return Promise.resolve([]);
      if (args.recordType === 'linux.old') return new Promise(resolve => { resolveOld = resolve; });
      return Promise.resolve(page(1, [record('new', 'new')]));
    });
    const oldRequest = results.selectNode(resultNode({ nodeId: 'old', artifactType: 'linux.old' }));
    await results.selectNode(resultNode({ nodeId: 'new' }));
    resolveOld(page(1, [record('old', 'old')]));
    await oldRequest;
    expect(results.records.value).toEqual([record('new', 'new')]);
    expect(results.recordsLoading.value).toBe(false);
  });

  it('shows record loading errors and allows retry without losing selection', async () => {
    const results = useAnalysisResults();
    invokeMock.mockRejectedValueOnce(new Error('读取失败'));
    await results.selectNode(resultNode());
    expect(results.recordsError.value).toContain('读取失败');
    pageResponse = page(1, [record('1', 'root')]);
    await results.retryRecords();
    expect(results.recordsError.value).toBeNull();
    expect(results.records.value).toEqual(pageResponse.items);
  });

  it('does not reopen a closed detail pane after a delayed response', async () => {
    const results = useAnalysisResults();
    let resolveDetail!: (value: AnalysisRecordDetail) => void;
    invokeMock.mockImplementation(command => command === 'analysis_record_detail'
      ? new Promise(resolve => { resolveDetail = resolve; }) : Promise.resolve([]));
    const request = results.openRecord('1');
    expect(results.detailLoading.value).toBe(true);
    results.closeDetail();
    resolveDetail({ record: record('1', 'root'), columns: [] });
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
