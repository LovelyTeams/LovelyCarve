// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, nextTick, ref, type App } from 'vue';
import AnalysisResultsPanel from './AnalysisResultsPanel.vue';
import type { AnalysisRecord, RecordColumn } from '../../../lib/ipc/analysisResults';

const state = vi.hoisted(() => ({
  results: {} as Record<string, unknown>,
  exportRecords: vi.fn(),
  toggleSort: vi.fn(),
  saveText: vi.fn(),
  notifyInfo: vi.fn(),
  notifyError: vi.fn(),
  pushProgress: vi.fn(),
  updateProgress: vi.fn(),
  dismissProgress: vi.fn(),
}));
vi.mock('../composables/useAnalysisResults', () => ({ ANALYSIS_PAGE_SIZE: 100, useAnalysisResults: () => state.results }));
vi.mock('../../tasks/api', () => ({ usePluginCatalog: () => ({ packages: ref([]), text: (_plugin: string, _key: string, fallback: string) => fallback }) }));
vi.mock('../composables/useAnalysisLabels', () => ({
  useAnalysisLabels: () => ({
    nodeLabel: (node: { label: string }) => node.label,
    columnLabel: (_node: unknown, column: RecordColumn) => column.label,
    choiceLabel: (_node: unknown, column: RecordColumn, id: string) => column.choices.find(choice => choice.id === id)?.label ?? id,
    nodeLabels: { node: (node: { label: string }) => node.label, category: (node: { typeName: string }) => node.typeName },
  }),
}));
vi.mock('../../../lib/ipc/client', () => ({ invokeCmd: vi.fn() }));
vi.mock('../../../lib/ipc/events', () => ({ onEvent: async () => () => {}, emitEvent: vi.fn() }));
vi.mock('../../../composables/export', () => ({ saveTextWithDialog: state.saveText }));
vi.mock('../../../composables/useToast', () => ({
  notifyInfo: state.notifyInfo,
  notifyError: state.notifyError,
  pushPersistentToast: state.pushProgress,
}));

let app: App;
let host: HTMLDivElement;
const openFile = vi.fn();
const openRecord = vi.fn();
const locating = ref(false);
const column = (id: string, label: string, overrides: Partial<RecordColumn> = {}): RecordColumn =>
  ({ id, label, kind: 'text', key: false, hidden: false, timeline: null, choices: [], ...overrides });
const record: AnalysisRecord = {
  recordId: '1', runId: 'run-1', recordType: 'linux.user', title: '用户 root', severity: null,
  values: { username: 'root', size: 2048, shell: 'zsh' },
  sources: [{ root: 'part:1', path: 'etc/passwd', note: '第 1 行' }, { root: 'part:1', path: 'etc/shadow' }],
  imageId: 'source-disk', legacy: false,
};
beforeEach(() => {
  localStorage.clear();
  vi.resetAllMocks(); locating.value = false;
  state.pushProgress.mockReturnValue({ update: state.updateProgress, dismiss: state.dismissProgress });
  state.results = {
    tree: ref([]), treeLoading: ref(false), treeError: ref(null), dirty: ref(false),
    selectedNode: ref({ kind: 'result', label: '用户', imageId: 'source-disk', pluginId: 'p', artifactType: 'linux.user', legacy: false, children: [] }),
    records: ref([record, { ...record, recordId: 'no-source', sources: [] }]), recordsTotal: ref(2),
    columns: ref([column('username', '用户名'), column('size', '大小', { kind: 'size' }), column('shell', 'Shell', { hidden: true })]),
    properties: ref(null),
    pageIndex: ref(0), pageCount: ref(1), keyword: ref(''), sort: ref(null), recordsLoading: ref(false), recordsError: ref(null),
    detail: ref(record), detailColumns: ref([column('username', '用户名'), column('shell', 'Shell', { hidden: true })]),
    detailLoading: ref(false), detailError: ref(null), selectedRecordId: ref('1'),
    audit: ref(null), auditLoading: ref(false), auditError: ref(null),
    openRecord, setImageId: vi.fn(), ensureFresh: vi.fn(), exportRecords: state.exportRecords, toggleSort: state.toggleSort,
    selectNode: vi.fn(), retryAudit: vi.fn(), retryRecords: vi.fn(), closeDetail: vi.fn(), goToPage: vi.fn(), applyKeyword: vi.fn(), reload: vi.fn(),
  };
  host = document.createElement('div'); document.body.append(host);
  app = createApp(AnalysisResultsPanel, { imageId: 'active-disk', get locating() { return locating.value; }, onOpenFile: openFile });
  app.mount(host);
});

describe('analysis UI feedback', () => {
  it('keeps a full field tooltip while abbreviating the cell', async () => {
    const value = '字段内容'.repeat(200);
    (state.results.columns as ReturnType<typeof ref>).value = [column('long', '长字段')];
    (state.results.records as ReturnType<typeof ref>).value = [{ ...record, values: { long: value } }];
    await nextTick();
    const cell = host.querySelector<HTMLTableCellElement>('tbody td:nth-child(2)')!;
    expect(cell.title).toBe(value);
    expect(cell.textContent?.length).toBe(120);
    for (const button of host.querySelectorAll<HTMLButtonElement>('.pager button')) {
      expect(button.title).toBe(button.getAttribute('aria-label'));
    }
  });

  it('shows the declared columns by kind and orders by a header', async () => {
    await nextTick();
    expect([...host.querySelectorAll('thead th')].map(header => header.textContent?.trim())).toEqual(['序号', '用户名', '大小', '源文件']);
    expect([...host.querySelectorAll('tbody tr:first-child td')].slice(1, 3).map(cell => cell.textContent)).toEqual(['root', '2.0 KB']);
    host.querySelectorAll<HTMLButtonElement>('thead .sort-header')[0].click();
    expect(state.toggleSort).toHaveBeenCalledWith('username');
    (state.results.sort as ReturnType<typeof ref>).value = { column: 'username', descending: true };
    await nextTick();
    expect(host.querySelectorAll('thead th')[1].getAttribute('aria-sort')).toBe('descending');
    expect(host.querySelector('tbody td:nth-child(3)')?.classList.contains('numeric')).toBe(true);
  });

  it('shows system information once using its declared fields', async () => {
    (state.results.columns as ReturnType<typeof ref>).value = [column('name', '名称'), column('key', '英文名称'), column('value', '值')];
    (state.results.records as ReturnType<typeof ref>).value = [{ ...record, title: '主机名', values: { name: '主机名', key: 'hostname', value: 'neko-virtual-machine' } }];
    await nextTick();
    expect([...host.querySelectorAll('thead th')].map(header => header.textContent?.trim())).toEqual(['序号', '名称', '英文名称', '值', '源文件']);
    expect([...host.querySelectorAll('tbody td')].map(cell => cell.textContent?.trim())).toEqual(['1', '主机名', 'hostname', 'neko-virtual-machine', 'etc/passwd+1']);
    expect(host.querySelectorAll('col')).toHaveLength(5);
    // Row number 56 + source 220 + three free-text columns sharing the rest, 200 each at least.
    expect(host.querySelector<HTMLTableElement>('table')?.style.minWidth).toBe('876px');
    expect([...host.querySelectorAll<HTMLTableColElement>('col')].map(column => column.style.width)).toEqual(['56px', '', '', '', '220px']);
    host.querySelector<HTMLButtonElement>('thead .sort-header')!.click();
    expect(state.toggleSort).toHaveBeenCalledWith('name');
  });

  it.each([{ columns: [] }, { columns: [column('extra', '附加信息', { hidden: true })] }])('keeps a sortable title fallback when no fields are visible (%j)', async ({ columns }) => {
    (state.results.columns as ReturnType<typeof ref>).value = columns;
    await nextTick();
    expect([...host.querySelectorAll('thead th')].map(header => header.textContent?.trim())).toEqual(['序号', '名称', '源文件']);
    expect(host.querySelector('tbody td:nth-child(2)')?.textContent).toBe('用户 root');
    host.querySelector<HTMLButtonElement>('thead .sort-header')!.click();
    expect(state.toggleSort).toHaveBeenCalledWith('title');
  });

  it('sizes columns by field kind, shows times with their zone and quiets missing values', async () => {
    (state.results.columns as ReturnType<typeof ref>).value = [column('user', '用户'), column('uid', 'UID', { kind: 'int' }), column('time', '时间', { kind: 'timestamp' })];
    (state.results.records as ReturnType<typeof ref>).value = [
      { ...record, values: { user: 'root', uid: 0, time: '2024-04-11T00:00:00+08:00' } },
      { ...record, recordId: '2', values: { user: 'deploy', uid: 1000, time: null } },
    ];
    await nextTick();
    expect([...host.querySelectorAll<HTMLTableColElement>('col')].map(column => column.style.width)).toEqual(['56px', '', '112px', '240px', '220px']);
    expect([...host.querySelectorAll('tbody td:nth-child(4)')].map(cell => [cell.textContent, cell.classList.contains('empty')]))
      .toEqual([['2024-04-11 00:00:00 UTC+08:00', false], ['—', true]]);
  });

  it('preserves severity beside the first field and in the title fallback', async () => {
    (state.results.records as ReturnType<typeof ref>).value = [{ ...record, severity: 'high' }];
    await nextTick();
    expect(host.querySelector('tbody td:nth-child(2) .ui-badge')).not.toBeNull();
    expect(host.querySelector('tbody td:nth-child(2) .title-text')?.textContent).toBe('root');
    (state.results.columns as ReturnType<typeof ref>).value = [];
    await nextTick();
    expect(host.querySelector('tbody td:nth-child(2) .ui-badge')).not.toBeNull();
    expect(host.querySelector('tbody td:nth-child(2) .title-text')?.textContent).toBe('用户 root');
  });

  it('reads plugin properties vertically, keeps duplicate labels distinct, and can switch to a table', async () => {
    (state.results.columns as ReturnType<typeof ref>).value = [column('name', '名称'), column('key', '英文名称'), column('value', '值')];
    (state.results.properties as ReturnType<typeof ref>).value = { label: 'name', secondary: 'key', value: 'value' };
    (state.results.records as ReturnType<typeof ref>).value = [
      { ...record, title: '内核版本', values: { name: '内核版本', key: 'Kernel', value: '6.8.0-138-generic' } },
      { ...record, recordId: '2', title: '内核版本', values: { name: '内核版本', key: 'Kernel', value: '6.8.0-40-generic' } },
    ];
    await nextTick();
    expect(host.querySelector('table')).toBeNull();
    expect(host.querySelectorAll('.property-row')).toHaveLength(2);
    expect([...host.querySelectorAll('.property-value')].map(value => value.textContent)).toEqual(['6.8.0-138-generic', '6.8.0-40-generic']);
    host.querySelector<HTMLButtonElement>('.property-label')!.click();
    expect(openRecord).toHaveBeenCalledWith('1');
    host.querySelector<HTMLButtonElement>('.property-row .source-link')!.click();
    expect(openFile).toHaveBeenCalledWith({ imageId: 'source-disk', rootId: 'part:1', path: 'etc/passwd' });
    host.querySelectorAll<HTMLButtonElement>('.reading-mode button')[1].click();
    await nextTick();
    expect(host.querySelector('table')).not.toBeNull();
    expect(JSON.parse(localStorage.getItem('lovelycarve-analysis-presentation')!)).toEqual({ 'p::linux.user': 'table' });
  });

  it('announces record loading and errors in their original location', async () => {
    (state.results.recordsLoading as ReturnType<typeof ref>).value = true;
    await nextTick();
    expect(host.querySelector('.table-scroll [role="status"]')?.textContent).toContain('正在读取记录');
    (state.results.recordsLoading as ReturnType<typeof ref>).value = false;
    (state.results.recordsError as ReturnType<typeof ref>).value = '读取失败';
    (state.results.detailError as ReturnType<typeof ref>).value = '详情失败';
    await nextTick();
    expect(host.querySelector('.table-scroll [role="alert"]')?.textContent).toContain('读取失败');
    expect(host.querySelector('.detail-dock [role="alert"]')?.textContent).toContain('详情失败');
  });

  it.each(['notice', 'toolbar'])('reports progress and cancels through the %s, then releases the notice', async source => {
    state.exportRecords.mockImplementation((_build, _write, options) => {
      options.onProgress({ loaded: 100, total: 200 });
      return new Promise((_resolve, reject) => options.signal.addEventListener('abort', () => reject(options.signal.reason), { once: true }));
    });
    host.querySelector<HTMLButtonElement>('.ui-toolbar-actions .ui-btn')!.click();
    await nextTick();
    expect(state.updateProgress).toHaveBeenCalledWith(expect.objectContaining({ progress: 50 }));
    if (source === 'notice') state.pushProgress.mock.calls[0][0].action.onClick();
    else host.querySelector<HTMLButtonElement>('.ui-toolbar-actions .ui-btn.ghost')!.click();
    await vi.waitFor(() => expect(state.dismissProgress).toHaveBeenCalledOnce());
    expect(state.saveText).not.toHaveBeenCalled();
    expect(state.notifyError).not.toHaveBeenCalled();
    expect(state.notifyInfo).toHaveBeenCalledWith('已取消导出', '用户');
    expect(host.querySelector<HTMLButtonElement>('.ui-toolbar-actions .ui-btn')!.disabled).toBe(false);
  });

  it('prevents stale result export while the directory is loading or has failed', async () => {
    const button = host.querySelector<HTMLButtonElement>('.ui-toolbar-actions .ui-btn')!;
    (state.results.treeLoading as ReturnType<typeof ref>).value = true;
    await nextTick();
    expect(button.disabled).toBe(true);
    (state.results.treeLoading as ReturnType<typeof ref>).value = false;
    (state.results.treeError as ReturnType<typeof ref>).value = '读取失败';
    await nextTick();
    expect(button.disabled).toBe(true);
    expect(host.querySelector('section > [role="status"]')?.textContent).toBe('');
  });

  it('exports stored values under unique headers and releases progress after saving', async () => {
    state.exportRecords.mockImplementation(async (build, write) => {
      await write(build([{ ...record, values: { username: 'root', name: 'x' } }], [column('username', '用户名'), column('name', '名称')]));
      return 1;
    });
    host.querySelector<HTMLButtonElement>('.ui-toolbar-actions .ui-btn')!.click();
    await vi.waitFor(() => expect(state.dismissProgress).toHaveBeenCalledOnce());
    expect(state.saveText).toHaveBeenCalledWith(expect.objectContaining({ defaultPath: '分析结果-用户.csv' }));
    const content: string = state.saveText.mock.calls[0][0].content;
    expect(content.split('\n')).toEqual(['名称,用户名,名称 (name)', '用户 root,root,x']);
    expect(state.updateProgress).toHaveBeenCalledWith(expect.objectContaining({ progress: 100, action: undefined }));
  });
});
afterEach(() => { app.unmount(); host.remove(); });

describe('analysis source actions', () => {
  it('emits a source-evidence address from the row and stops row selection', async () => {
    await nextTick();
    const button = host.querySelector<HTMLButtonElement>('tbody .ui-btn')!;
    button.click();
    expect(openFile).toHaveBeenCalledWith({ imageId: 'source-disk', rootId: 'part:1', path: 'etc/passwd' });
    expect(openRecord).not.toHaveBeenCalled();
    expect(host.querySelectorAll<HTMLButtonElement>('tbody .table-source')[1].disabled).toBe(true);
  });
  it('opens all provenance from the source count instead of locating only the first file', async () => {
    await nextTick();
    host.querySelector<HTMLButtonElement>('tbody .source-more')!.click();
    expect(openRecord).toHaveBeenCalledWith('1');
    expect(openFile).not.toHaveBeenCalled();
  });
  it('returns focus to the originating record control when details close', async () => {
    host.querySelectorAll<HTMLButtonElement>('.reading-mode button')[0].click();
    await nextTick();
    const origin = host.querySelector<HTMLButtonElement>('.record-sheet .property-label')!;
    origin.focus(); origin.click();
    const close = host.querySelector<HTMLButtonElement>('[aria-label="关闭详情"]')!;
    close.focus(); close.click();
    await nextTick();
    expect(document.activeElement).toBe(origin);
  });
  it('offers each source separately in details, with its note', async () => {
    await nextTick();
    const sources = host.querySelectorAll<HTMLElement>('.source-file');
    expect([...sources].map(source => source.textContent)).toEqual(['etc/passwd第 1 行', 'etc/shadow']);
    sources[1].querySelector<HTMLButtonElement>('.ui-btn')!.click();
    expect(openFile).toHaveBeenCalledWith({ imageId: 'source-disk', rootId: 'part:1', path: 'etc/shadow' });
  });
  it('lists hidden columns in the detail, which the table leaves out', async () => {
    await nextTick();
    const labels = [...host.querySelectorAll('.detail-dock dt')].map(term => term.textContent);
    expect(labels).toEqual(expect.arrayContaining(['用户名', 'Shell', 'size']));
    const sections = [...host.querySelectorAll('.detail-dock .file-detail-section')].map(section => section.getAttribute('aria-label'));
    expect(sections).toEqual(['记录内容', '源文件', '技术信息']);
  });
  it('steps the dock through the records of the page', async () => {
    await nextTick();
    const previous = host.querySelector<HTMLButtonElement>('[aria-label="上一条记录"]')!;
    const next = host.querySelector<HTMLButtonElement>('[aria-label="下一条记录"]')!;
    expect([previous.disabled, next.disabled]).toEqual([true, false]);
    next.click();
    expect(openRecord).toHaveBeenCalledWith('no-source');
    (state.results.selectedRecordId as ReturnType<typeof ref>).value = 'no-source';
    await nextTick();
    expect([previous.disabled, next.disabled]).toEqual([false, true]);
  });
  it('walks table rows with the arrow keys and lets an open record follow', async () => {
    await nextTick();
    const rows = host.querySelectorAll<HTMLTableRowElement>('tbody tr');
    rows[0].focus();
    rows[0].dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true, cancelable: true }));
    expect(document.activeElement).toBe(rows[1]);
    expect(openRecord).toHaveBeenCalledWith('no-source');
    rows[1].dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true, cancelable: true }));
    expect(document.activeElement).toBe(rows[1]);
    expect(openRecord).toHaveBeenCalledOnce();
  });
});

describe('analysis run overview', () => {
  const resultNode = (nodeId: string, label: string, childCount: number) => ({
    nodeId, kind: 'result', label, imageId: 'source-disk', typeName: 'baseinfo', pluginId: 'p', analyzer: 'linux', function: 'p/linux',
    sourceNodeId: null, targetLabel: 'Ubuntu · part:2', artifactType: nodeId, runId: 'run-1', completedAt: '', childCount, warningCount: 0, legacy: false, children: [],
  });
  const users = resultNode('linux.user', '用户', 34);
  const usb = resultNode('linux.usb', 'USB 记录', 0);
  const run = { ...users, nodeId: 'run', kind: 'function', label: 'Linux 基本信息', artifactType: '', completedAt: '2026-10-19T13:30:32Z', childCount: 2, warningCount: 1, children: [users, usb] };
  const audit = {
    runId: 'run-1', state: 'published', pluginId: 'p', pluginVersion: '2.0.0', analyzer: 'linux', analyzerName: 'Linux 基本信息',
    target: { kind: 'system', id: 'sys', label: 'Ubuntu · part:2' }, options: { items: [] }, profile: { os: 'linux' },
    inputs: [{ path: '/etc/passwd', root: 'part:2', rootPath: 'etc/passwd', nodeId: null, size: 1873 }], inputCount: 1, schemaHash: 'hash', summary: '识别到 Ubuntu',
    recordCount: 34, recordTypes: [], warningCount: 1, warnings: [{ code: 'parse.skipped', message: '跳过一条记录', path: '/var/log/wtmp', nodeId: 'ext4:12' }],
    badRegions: [{ offset: 4096, len: 512 }], syntheticBytes: 512,
    startedAt: '2026-10-19T13:30:21Z', finishedAt: '2026-10-19T13:30:32.5Z', publishedAt: '2026-10-19T13:30:32.5Z',
  };
  async function showRun(overrides: Record<string, unknown> = {}) {
    Object.assign(state.results, { tree: ref([run]), selectedNode: ref(run), selectedRecordId: ref(null), audit: ref(audit), ...overrides });
    app.unmount();
    host.replaceChildren();
    app = createApp(AnalysisResultsPanel, { imageId: 'active-disk', locating: false, onOpenFile: openFile });
    app.mount(host);
    await nextTick();
  }

  it('summarizes the run and opens a record type from its output', async () => {
    await showRun();
    expect(host.querySelector('.run-title h2')?.textContent).toBe('Linux 基本信息');
    expect(host.querySelector('.run-target')?.textContent).toBe('Ubuntu · part:2');
    expect([...host.querySelectorAll('.stat')].map(stat => [stat.querySelector('dt')?.textContent, stat.querySelector('dd')?.textContent]))
      .toEqual([['记录数', '34'], ['记录类型', '2'], ['输入文件', '1'], ['告警', '1'], ['耗时', '11.5 s']]);
    expect(host.querySelector('.ui-alert.danger')?.textContent).toContain('512');
    const outputs = [...host.querySelectorAll<HTMLButtonElement>('.output-row')];
    expect(outputs.map(row => [row.querySelector('.output-name')?.textContent, row.querySelector('.output-count')?.textContent, row.classList.contains('empty')]))
      .toEqual([['用户', '34', false], ['USB 记录', '0', true]]);
    outputs[1].click();
    expect(state.results.selectNode).toHaveBeenCalledWith(usb);
    host.querySelector<HTMLButtonElement>('.warning-row .location')!.click();
    expect(openFile).toHaveBeenCalledWith({ imageId: 'source-disk', nodeId: 'ext4:12' });
    host.querySelector<HTMLButtonElement>('.input-row .location')!.click();
    expect(openFile).toHaveBeenLastCalledWith({ imageId: 'source-disk', rootId: 'part:2', path: 'etc/passwd' });
  });

  it('keeps the outputs of a result without an audit and reports a failed audit in place', async () => {
    await showRun({ selectedNode: ref({ ...run, legacy: true }), audit: ref(null) });
    expect(host.querySelector('.ui-alert.info')?.textContent).toContain('旧版本的分析结果没有运行信息');
    expect(host.querySelectorAll('.output-row')).toHaveLength(2);
    expect([...host.querySelectorAll('.stat dt')].map(term => term.textContent)).toEqual(['记录数', '记录类型']);
    await showRun({ audit: ref(null), auditError: ref('读取失败') });
    const alert = host.querySelector<HTMLElement>('[role="alert"]')!;
    expect(alert.textContent).toContain('读取失败');
    alert.querySelector<HTMLButtonElement>('button')!.click();
    expect(state.results.retryAudit).toHaveBeenCalledOnce();
  });

  it('points an empty record type to the run that explains it', async () => {
    await showRun({ selectedNode: ref(usb), records: ref([]), recordsTotal: ref(0) });
    const action = [...host.querySelectorAll<HTMLButtonElement>('.table-scroll .ui-empty-actions button')].find(button => button.textContent?.includes('查看运行信息'))!;
    action.click();
    expect(state.results.selectNode).toHaveBeenCalledWith(run);
  });
});
