// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, defineComponent, h, nextTick, ref, type App } from 'vue';
import TimelinePanel from './TimelinePanel.vue';

const ipc = vi.hoisted(() => ({ count: vi.fn(), datas: vi.fn(), types: vi.fn(), status: vi.fn(), start: vi.fn(), export: vi.fn(), save: vi.fn() }));
vi.mock('../../../lib/ipc/timeline', () => ({ timelineCount: ipc.count, timelineDatas: ipc.datas, timelineTypes: ipc.types, timelineStatus: ipc.status, timelineStart: ipc.start, timelineExport: ipc.export, onTimelineProgress: vi.fn(async () => () => {}) }));
vi.mock('../../../lib/ipc/taskRuntime', () => ({ taskCancel: vi.fn() }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ save: ipc.save }));
vi.mock('../../../composables/useToast', () => ({ notifyInfo: vi.fn(), notifyError: vi.fn(), notifyExportSuccess: vi.fn() }));
vi.mock('./TimelineHeatmap.vue', () => ({ default: { render: () => null } }));
vi.mock('./TimelineHistogram.vue', () => ({ default: { render: () => null } }));

let app: App;
const imageId = ref('disk-a');
async function settle() { for (let index = 0; index < 16; index++) await nextTick(); }
function clickButton(text: string) {
  const button = [...document.querySelectorAll('button')].find(button => button.textContent?.includes(text));
  expect(button, text).toBeDefined();
  button!.click();
}
beforeEach(async () => {
  vi.resetAllMocks();
  imageId.value = 'disk-a';
  ipc.status.mockResolvedValue({ status: 'ready', start_time: 100, end_time: 200, record_count: 1, meta: null });
  ipc.count.mockResolvedValue([]);
  ipc.types.mockResolvedValue([]);
  ipc.datas.mockImplementation(async ({ imageId }) => ({ items: [{ id: 1, oid: 'file-a', eid: imageId, tid: '', tl_time: 100, from_app: 'files', data_type: 1, tl_flag: '修改', content: 'test.txt', extra: '', deleted: false }], total: 1 }));
  ipc.save.mockResolvedValue('timeline.csv');
  ipc.export.mockResolvedValue({ output_path: 'timeline.csv', record_count: 1 });
  const host = document.createElement('div');
  document.body.append(host);
  app = createApp(defineComponent({ setup: () => () => h(TimelinePanel, { imageId: imageId.value }) }));
  app.mount(host);
  await settle();
});
afterEach(() => { app.unmount(); document.body.replaceChildren(); });

describe('timeline current evidence scope', () => {
  it('moves focus into event filters and restores the opener on Escape', async () => {
    const opener = [...document.querySelectorAll('button')].find(button => button.textContent?.includes('全部类型'))!;
    opener.focus(); opener.click(); await settle();
    const dialog = document.querySelector('[aria-label="时间线筛选配置"]') as HTMLElement;
    expect(document.activeElement).toBe(dialog);
    dialog.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); await settle();
    expect(document.activeElement).toBe(opener);
    await vi.waitFor(() => expect(document.querySelector('[aria-label="时间线筛选配置"]')).toBeNull());
  });
  it('starts from the shared landing page and opens the workspace while building', async () => {
    ipc.status.mockResolvedValue({ status: 'buildRequired', record_count: 0, meta: null });
    imageId.value = 'disk-b';
    await settle();
    expect(document.querySelector('.workspace-start')?.textContent).toContain('还没有时间线');
    expect(document.querySelector('.tl-filter-row')).toBeNull();
    expect(document.querySelector('.tl-table')).toBeNull();
    let finish!: (value: unknown) => void;
    ipc.start.mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
    clickButton('创建时间线');
    await settle();
    expect(ipc.start).toHaveBeenCalledExactlyOnceWith({ imageId: 'disk-b' });
    expect(document.querySelector('.workspace-start')).toBeNull();
    expect(document.querySelector('.tl-notice')?.textContent).toContain('正在构建');
    ipc.status.mockResolvedValue({ status: 'ready', record_count: 1, meta: null });
    finish({ status: 'success', file_rows: 1, event_rows: 0, skipped_invalid_time: 0 });
    await settle();
    expect(document.querySelector('.tl-table')?.textContent).toContain('test.txt');
  });

  it('disables the initial action while loading and retries failed status reads', async () => {
    let fail!: (error: Error) => void;
    ipc.status.mockReturnValueOnce(new Promise((_, reject) => { fail = reject; }));
    imageId.value = 'disk-b';
    await settle();
    const action = document.querySelector('.workspace-start .primary') as HTMLButtonElement;
    expect(action.disabled).toBe(true);
    action.click();
    expect(ipc.start).not.toHaveBeenCalled();
    fail(new Error('状态读取失败'));
    await settle();
    expect(document.querySelector('.workspace-start')?.textContent).toContain('状态读取失败');
    ipc.status.mockResolvedValue({ status: 'buildRequired', record_count: 0, meta: null });
    clickButton('重试');
    await settle();
    expect(document.querySelector('.workspace-start')?.textContent).toContain('还没有时间线');
    expect(ipc.status).toHaveBeenLastCalledWith({ imageId: 'disk-b' });
    expect(ipc.start).not.toHaveBeenCalled();
  });

  it('keeps charts, records, event types and export scoped to the active evidence', async () => {
    expect(ipc.count).toHaveBeenCalled();
    expect(ipc.datas).toHaveBeenCalled();
    for (const [request] of [...ipc.count.mock.calls, ...ipc.datas.mock.calls]) expect(request.imageId).toBe('disk-a');
    clickButton('全部类型');
    await settle();
    expect(ipc.types).toHaveBeenLastCalledWith({ imageId: 'disk-a' });
    expect(document.body.textContent).not.toContain('全部检材');
    expect(document.querySelector('[aria-label="时间线筛选配置"]')?.textContent).not.toContain('检材');
    clickButton('应用');
    await settle();
    ipc.count.mockClear(); ipc.datas.mockClear();
    imageId.value = 'disk-b';
    await settle();
    expect(ipc.count).toHaveBeenCalled();
    expect(ipc.datas).toHaveBeenCalled();
    for (const [request] of [...ipc.count.mock.calls, ...ipc.datas.mock.calls]) expect(request.imageId).toBe('disk-b');
    clickButton('导出结果');
    await settle();
    expect(ipc.export.mock.calls[0][0].imageId).toBe('disk-b');
  });

  it('ignores a previous evidence’s delayed event-type response', async () => {
    let finish!: (value: unknown) => void;
    ipc.types.mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
    clickButton('全部类型');
    await settle();
    imageId.value = 'disk-b';
    await settle();
    expect(document.querySelector('[aria-label="时间线筛选配置"]')).toBeNull();
    finish([{ from_app: '旧检材事件', types: [] }]);
    await settle();
    clickButton('全部类型');
    await settle();
    expect(ipc.types).toHaveBeenLastCalledWith({ imageId: 'disk-b' });
    expect(document.body.textContent).not.toContain('旧检材事件');
  });

  it('ignores an old status response after A→B→A and keeps the current workspace', async () => {
    let finish!: (value: unknown) => void;
    ipc.status.mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
    imageId.value = 'disk-b'; await settle(); imageId.value = 'disk-a'; await settle();
    finish({ status: 'buildRequired', record_count: 0, meta: null }); await settle();
    expect(document.querySelector('.workspace-start')).toBeNull();
    expect(document.querySelector('.tl-table')?.textContent).toContain('test.txt');
  });

  it('keeps a new build running when an old evidence build completes', async () => {
    let finishOld!: (value: unknown) => void; let finishNew!: (value: unknown) => void;
    ipc.start.mockReturnValueOnce(new Promise(resolve => { finishOld = resolve; }))
      .mockReturnValueOnce(new Promise(resolve => { finishNew = resolve; }));
    clickButton('重建时间线'); await settle(); imageId.value = 'disk-b'; await settle();
    clickButton('重建时间线'); await settle();
    const statusCalls = ipc.status.mock.calls.length;
    finishOld({ status: 'success', file_rows: 1, event_rows: 0, skipped_invalid_time: 0 }); await settle();
    expect(document.querySelector('.tl-notice')?.textContent).toContain('正在构建');
    expect(ipc.status).toHaveBeenCalledTimes(statusCalls);
    finishNew({ status: 'success', file_rows: 1, event_rows: 0, skipped_invalid_time: 0 }); await settle();
    expect(document.querySelector('.tl-notice')).toBeNull();
  });

  it('does not export after a save chooser from a previous evidence returns', async () => {
    let finish!: (value: string) => void; ipc.save.mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
    clickButton('导出结果'); await settle(); imageId.value = 'disk-b'; await settle();
    finish('old.csv'); await settle(); expect(ipc.export).not.toHaveBeenCalled();
  });
});
