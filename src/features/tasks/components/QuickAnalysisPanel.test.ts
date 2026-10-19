// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, h, nextTick, ref, type App } from 'vue';
import type { DetectedSystem, SchedFunction, SchedTaskRecord, SchedTaskStatus } from '../../../lib/ipc/tasks';
import QuickAnalysisPanel from './QuickAnalysisPanel.vue';

const mocks = vi.hoisted(() => ({
  refresh: vi.fn(), create: vi.fn(), restart: vi.fn(), start: vi.fn(), confirm: vi.fn(), notifyInfo: vi.fn(), submitted: vi.fn(), systems: vi.fn(),
}));
const functions = ref<SchedFunction[]>([]);
const tasks = ref<SchedTaskRecord[]>([]);
vi.mock('../composables/useTaskCenter', async importOriginal => ({
  ...await importOriginal<typeof import('../composables/useTaskCenter')>(),
  useTaskCenter: () => ({ functions, tasks, loading: ref(false), error: ref(null), refresh: mocks.refresh, create: mocks.create, restart: mocks.restart, start: mocks.start }),
}));
vi.mock('../composables/usePluginCatalog', () => ({
  usePluginCatalog: () => ({ text: (_pluginId: string, _key: string, fallback: string) => fallback }),
}));
vi.mock('../../../lib/ipc/tasks', async importOriginal => ({
  ...await importOriginal<typeof import('../../../lib/ipc/tasks')>(),
  schedSystemsList: mocks.systems,
}));
vi.mock('../../../composables/useConfirm', () => ({ confirm: mocks.confirm }));
vi.mock('../../../composables/useToast', () => ({ notifyInfo: mocks.notifyInfo }));

const SYSTEM = 'windows@part:2';
/** One analysis function of plugin `name.split('/')[0]`, named `pluginName`. */
function fn(name: string, pluginName: string, displayName: string, typeName: string, overrides: Partial<SchedFunction> = {}): SchedFunction {
  const [pluginId, analyzer] = name.split('/');
  return {
    function: name, pluginId, pluginName, pluginVersion: '1', analyzer, displayName, typeName,
    scope: 'system', platforms: ['windows'], trigger: 'manually', internal: false, singleInstance: false,
    provides: [], requires: [], extensions: [], mimeTypes: [], inputs: [], options: [], ...overrides,
  };
}
function task(id: number, name: string, status: SchedTaskStatus, imageId = 'disk-a', systemId: string | null = SYSTEM): SchedTaskRecord {
  return {
    id, name, status, displayName: functions.value.find(item => item.function === name)?.displayName ?? name, typeName: 'baseinfo', evidenceId: 1,
    progress: 50, canPause: false, singleInstance: false, internal: false, dependence: '', spendTimeSecs: 3, remainTimeSecs: null,
    startAt: null, createAt: '2026-10-02 05:00:00', updateAt: '', result: null,
    info: { imageId, pluginId: name.split('/')[0], analyzer: 'base', trigger: 'manually', parentTid: null, error: null },
    params: { fileName: null, nodeId: null, systemId, targetLabel: systemId ? 'Windows 10 · part:2' : null, options: {} },
  };
}
function system(id: string, family: string, label: string): DetectedSystem {
  return { id, family, product: label, version: null, label, root: id.split('@')[1], mounts: [] };
}

let app: App | undefined;
let host: HTMLDivElement;
async function settle() {
  for (let index = 0; index < 4; index++) {
    await nextTick();
    await Promise.resolve();
  }
}
async function mount() {
  app = createApp({ render: () => h(QuickAnalysisPanel, { imageId: 'disk-a', onSubmitted: mocks.submitted }) });
  app.mount(host);
  await settle();
}
const tile = (name: string) => [...host.querySelectorAll<HTMLElement>('.qa-tile')].find(element => element.querySelector('.qa-tile-name')?.textContent === name)!;
const checkbox = (name: string) => tile(name).querySelector<HTMLInputElement>('.qa-tile-choice input')!;
const status = (name: string) => tile(name).querySelector('.qa-tile-status')?.textContent ?? null;
const selectAll = () => host.querySelector<HTMLInputElement>('.qa-check-all input')!;
const categoryBox = (label: string) => host.querySelector<HTMLInputElement>(`input[aria-label="全选${label}"]`)!;
const tileNames = () => [...host.querySelectorAll('.qa-tile-name')].map(name => name.textContent);
async function click(label: string) {
  const button = [...host.querySelectorAll<HTMLButtonElement>('button')].find(element => element.getAttribute('aria-label') === label || element.textContent?.trim() === label);
  if (!button) throw new Error(`Missing button: ${label}`);
  button.click();
  await settle();
}
async function tick(element: HTMLInputElement) {
  element.click();
  await settle();
}
const footer = () => host.querySelector('.qa-summary')?.textContent ?? '';

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  mocks.confirm.mockResolvedValue(true);
  mocks.systems.mockResolvedValue([system(SYSTEM, 'windows', 'Windows 10 · part:2')]);
  mocks.create.mockImplementation(async (args: { function: string }) => ({ id: 99, name: args.function }));
  functions.value = [
    fn('chat/messages', '聊天', '消息', 'baseinfo'),
    fn('chat/contacts', '聊天', '联系人', 'baseinfo'),
    fn('qq/main', 'QQ', 'QQ 聊天记录', 'baseinfo'),
    fn('web/history', '浏览器', '浏览历史', 'browserinfo'),
    fn('web/downloads', '浏览器', '下载记录', 'browserinfo'),
    fn('log/system', '日志', '系统日志', 'loginfo'),
    fn('linux/base', 'Linux 信息', '基本信息', 'baseinfo', { platforms: ['linux'] }),
    fn('car/can', '汽车', 'CAN 总线', 'vehicle', { scope: 'evidence', platforms: [] }),
    fn('mail/file', '邮件', '邮件文件', 'fileanalyzer', { scope: 'files', platforms: [] }),
  ];
  tasks.value = [
    task(1, 'qq/main', 'success'),
    task(2, 'chat/messages', 'running'),
    task(3, 'qq/main', 'failed'),
    task(4, 'web/history', 'success', 'disk-b'),
    task(6, 'log/system', 'success'),
  ];
  host = document.createElement('div');
  document.body.append(host);
});
afterEach(() => { app?.unmount(); app = undefined; host.remove(); });

describe('quick analysis checklist', () => {
  it('lays plugins out as tiles by category, each with its latest task on the selected system', async () => {
    await mount();
    expect(mocks.refresh).toHaveBeenCalledOnce();
    expect(mocks.systems).toHaveBeenCalledExactlyOnceWith('disk-a');
    expect(host.querySelector('.qa-system-label')?.textContent).toBe('Windows 10 · part:2');
    expect([...host.querySelectorAll('.qa-group-label')].map(label => label.textContent)).toEqual(['基本信息', '浏览器', '日志', 'vehicle']);
    expect(tileNames()).toEqual(['聊天', 'QQ', '浏览器', '日志', '汽车']);
    expect(tile('聊天').querySelector('label')?.title).toContain('包含：消息、联系人');
    expect(status('聊天')).toBe('执行中 50%');
    expect(status('QQ')).toBe('失败');
    expect(status('浏览器')).toBeNull();
    expect(checkbox('聊天').disabled).toBe(true);
    expect(checkbox('QQ').disabled).toBe(false);
    expect(host.querySelector('.qa-selectbar')?.textContent).toContain('5 个插件');
    expect(footer()).toContain('勾选要运行的插件');
  });

  it('lists only whole-evidence analyzers when no system was detected', async () => {
    mocks.systems.mockResolvedValue([]);
    await mount();
    expect(host.querySelector('.qa-system-label')?.textContent).toBe('未识别出操作系统');
    expect(tileNames()).toEqual(['汽车']);
  });

  it('offers a retry when systems cannot be detected', async () => {
    mocks.systems.mockRejectedValueOnce(new Error('检材未加载'));
    await mount();
    expect(tileNames()).toEqual(['汽车']);
    await click('重新识别系统');
    expect(tileNames()).toEqual(['聊天', 'QQ', '浏览器', '日志', '汽车']);
  });

  it('switches between detected systems and remembers the choice per evidence', async () => {
    mocks.systems.mockResolvedValue([system(SYSTEM, 'windows', 'Windows 10 · part:2'), system('linux@part:5', 'linux', 'Kali · part:5')]);
    await mount();
    expect(host.querySelector('.qa-system')).not.toBeNull();
    expect(tileNames()).toContain('QQ');
    host.querySelector<HTMLButtonElement>('.qa-system')!.click();
    await settle();
    [...document.querySelectorAll<HTMLElement>('.ui-select-option')].find(option => option.textContent === 'Kali · part:5')!.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await settle();
    expect(tileNames()).toEqual(['Linux 信息', '汽车']);
    app?.unmount();
    await mount();
    expect(tileNames()).toEqual(['Linux 信息', '汽车']);
  });

  it('selects everything listed with one box and a whole category with its own', async () => {
    await mount();
    await tick(selectAll());
    expect(['QQ', '浏览器', '日志', '汽车'].map(name => checkbox(name).checked)).toEqual([true, true, true, true]);
    expect(checkbox('聊天').checked).toBe(false);
    expect(selectAll().checked).toBe(true);
    expect(footer()).toContain('已选 4 个插件');
    expect(footer()).toContain('其中 1 个已有结果，重新分析成功后替换');
    await tick(selectAll());
    expect(footer()).toContain('勾选要运行的插件');

    await tick(categoryBox('基本信息'));
    expect(checkbox('QQ').checked).toBe(true);
    expect(categoryBox('基本信息').checked).toBe(true);
    expect(selectAll().indeterminate).toBe(true);
    expect(host.querySelector('.qa-group-count')?.textContent).toBe('1/2');
    tasks.value = [...tasks.value, task(5, 'qq/main', 'queued')];
    await settle();
    expect(checkbox('QQ').checked).toBe(false);
    expect(categoryBox('基本信息').disabled).toBe(true);
  });

  it('narrows the select-all box to the search matches', async () => {
    await mount();
    const search = host.querySelector<HTMLInputElement>('.qa-search input')!;
    search.value = '下载';
    search.dispatchEvent(new Event('input'));
    await settle();
    expect(tileNames()).toEqual(['浏览器']);
    expect(host.querySelector('.qa-check-all')?.textContent).toContain('全选匹配的插件');
    await tick(selectAll());
    expect(footer()).toContain('已选 1 个插件');
  });

  it('folds a category and unfolds everything again', async () => {
    await mount();
    const toggle = () => [...host.querySelectorAll<HTMLButtonElement>('.qa-group-toggle')].find(button => button.textContent?.includes('浏览器'))!;
    expect(toggle().getAttribute('aria-expanded')).toBe('true');
    toggle().click();
    await settle();
    expect(toggle().getAttribute('aria-expanded')).toBe('false');
    await click('全部展开');
    expect(toggle().getAttribute('aria-expanded')).toBe('true');
    await click('全部收起');
    expect([...host.querySelectorAll('.qa-group-toggle')].every(button => button.getAttribute('aria-expanded') === 'false')).toBe(true);
  });
});

describe('quick analysis submission', () => {
  it('confirms replacing published results, runs every function on its target and returns to the task list', async () => {
    await mount();
    await tick(checkbox('日志'));
    await tick(checkbox('浏览器'));
    await tick(checkbox('汽车'));
    await click('开始分析');
    expect(mocks.confirm).toHaveBeenCalledWith(expect.objectContaining({ title: '已有分析结果', message: expect.stringContaining('日志') }));
    expect(mocks.create.mock.calls).toEqual([
      [{ function: 'log/system', imageId: 'disk-a', systemId: SYSTEM }, { quiet: true }],
      [{ function: 'web/history', imageId: 'disk-a', systemId: SYSTEM }, { quiet: true }],
      [{ function: 'web/downloads', imageId: 'disk-a', systemId: SYSTEM }, { quiet: true }],
      [{ function: 'car/can', imageId: 'disk-a', systemId: null }, { quiet: true }],
    ]);
    expect(mocks.notifyInfo).toHaveBeenCalledWith('快速分析', '已提交 4 个分析任务，可在任务列表查看进度');
    expect(mocks.submitted).toHaveBeenCalledOnce();
    expect(footer()).toContain('勾选要运行的插件');
  });

  it('submits nothing when the replacement is declined', async () => {
    mocks.confirm.mockResolvedValueOnce(false);
    await mount();
    await tick(checkbox('日志'));
    await click('开始分析');
    expect(mocks.create).not.toHaveBeenCalled();
    expect(checkbox('日志').checked).toBe(true);
  });

  it('keeps a plugin selected when one of its tasks could not be created', async () => {
    mocks.create.mockImplementation(async (args: { function: string }) => (args.function === 'web/downloads' ? null : { id: 99 }));
    await mount();
    await tick(checkbox('浏览器'));
    await tick(checkbox('QQ'));
    await click('开始分析');
    expect(mocks.confirm).not.toHaveBeenCalled();
    expect(checkbox('浏览器').checked).toBe(true);
    expect(checkbox('QQ').checked).toBe(false);
    expect(mocks.submitted).not.toHaveBeenCalled();
  });

  it('re-runs every ended evidence task from the more menu and closes the menu on outside clicks', async () => {
    tasks.value = [...tasks.value, task(7, 'log/system', 'pause')];
    await mount();
    await click('更多操作');
    expect(host.querySelector('.qa-more-menu')).not.toBeNull();
    document.body.dispatchEvent(new Event('pointerdown', { bubbles: true }));
    await settle();
    expect(host.querySelector('.qa-more-menu')).toBeNull();
    await click('更多操作');
    await click('重新分析全部任务');
    expect(mocks.restart.mock.calls.map(([id]) => id)).toEqual([1, 3, 6]);
    expect(mocks.start).toHaveBeenCalledExactlyOnceWith(7);
  });
});
