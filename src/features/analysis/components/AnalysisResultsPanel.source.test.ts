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
  vi.resetAllMocks(); locating.value = false;
  state.pushProgress.mockReturnValue({ update: state.updateProgress, dismiss: state.dismissProgress });
  state.results = {
    tree: ref([]), treeLoading: ref(false), treeError: ref(null), dirty: ref(false),
    selectedNode: ref({ kind: 'result', label: '用户', imageId: 'source-disk', pluginId: 'p', artifactType: 'linux.user', legacy: false, children: [] }),
    records: ref([record, { ...record, recordId: 'no-source', sources: [] }]), recordsTotal: ref(2),
    columns: ref([column('username', '用户名'), column('size', '大小', { kind: 'size' }), column('shell', 'Shell', { hidden: true })]),
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
    const cell = host.querySelector<HTMLTableCellElement>('tbody td:nth-child(3)')!;
    expect(cell.title).toBe(value);
    expect(cell.textContent?.length).toBe(120);
    for (const button of host.querySelectorAll<HTMLButtonElement>('.pager button')) {
      expect(button.title).toBe(button.getAttribute('aria-label'));
    }
  });

  it('shows the declared columns by kind and orders by a header', async () => {
    await nextTick();
    expect([...host.querySelectorAll('thead th')].map(header => header.textContent?.trim())).toEqual(['序号', '名称', '用户名', '大小', '定位']);
    expect([...host.querySelectorAll('tbody tr:first-child td')].slice(2, 4).map(cell => cell.textContent)).toEqual(['root', '2.0 KB']);
    host.querySelectorAll<HTMLButtonElement>('thead .sort-header')[1].click();
    expect(state.toggleSort).toHaveBeenCalledWith('username');
    (state.results.sort as ReturnType<typeof ref>).value = { column: 'username', descending: true };
    await nextTick();
    expect(host.querySelectorAll('thead th')[2].getAttribute('aria-sort')).toBe('descending');
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
    expect(host.querySelectorAll<HTMLButtonElement>('tbody .ui-btn')[1].disabled).toBe(true);
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
  });
});
