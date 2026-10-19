// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, h, nextTick, ref, type App } from 'vue';
import type { BuiltinTaskEntry, TaskEntry, TaskView } from '../../../lib/ipc/taskRuntime';
import type { SchedFunction, SchedTaskRecord, SchedTaskStatus } from '../../../lib/ipc/tasks';
import { pluginTaskEntry } from '../model/taskEntryView';
import TaskCenterPanel from './TaskCenterPanel.vue';

const actions = vi.hoisted(() => ({
  refresh: vi.fn(), cancel: vi.fn(), pause: vi.fn(), resume: vi.fn(), start: vi.fn(), restart: vi.fn(), remove: vi.fn(), confirm: vi.fn(),
  openResult: vi.fn(), quickAnalysis: vi.fn(),
}));
const entries = ref<TaskEntry[]>([]);
const functions = ref<Partial<SchedFunction>[]>([{
  function: 'test/base', options: [{ id: 'items', kind: 'multi', label: '子项', default: [], choices: [{ id: 'users', label: '用户列表' }, { id: 'groups', label: '用户组' }] }],
}]);
vi.mock('../composables/useUnifiedTaskCenter', async () => {
  const { computed, ref: vueRef } = await import('vue');
  return {
    useUnifiedTaskCenter: (imageId: () => string | null) => ({
      tasks: computed(() => entries.value.filter(entry => entry.imageId === null || entry.imageId === imageId())),
      notices: vueRef([]), loading: vueRef(false), error: vueRef(null), now: vueRef(Date.now()), ...actions,
    }),
  };
});
vi.mock('../composables/useTaskCenter', () => ({ useTaskCenter: () => ({ functions }) }));
vi.mock('../composables/usePluginCatalog', () => ({
  usePluginCatalog: () => ({ text: (_pluginId: string, key: string, fallback: string) => key === 'analyzer.translated' ? 'Translated analyzer' : fallback }),
}));
vi.mock('../../../composables/useConfirm', () => ({ confirm: actions.confirm }));

let app: App | undefined;
let host: HTMLDivElement;
const imageId = ref<string | null>('disk-a');
const minute = (id: number) => String(id).padStart(2, '0');
function job(id: number, status: SchedTaskStatus, extra: Partial<SchedTaskRecord> = {}): SchedTaskRecord {
  return {
    id, status, name: `test/${id}`, displayName: `任务 ${id}`, typeName: '系统分析', evidenceId: 1,
    progress: 42, canPause: false, singleInstance: false, internal: false, dependence: '',
    spendTimeSecs: 7, remainTimeSecs: null, startAt: null, createAt: `2026-09-19T10:${minute(id)}:00Z`, updateAt: `2026-09-19T11:${minute(id)}:00Z`,
    info: { imageId: 'disk-a', pluginId: 'test', analyzer: 'base', trigger: 'manually', parentTid: null, error: null },
    params: { fileName: null, nodeId: null, systemId: null, targetLabel: null, options: {} }, result: null, ...extra,
  };
}
function plugin(id: number, status: SchedTaskStatus, extra: Partial<SchedTaskRecord> = {}): TaskEntry {
  return pluginTaskEntry(job(id, status, extra));
}
function view(extra: Partial<TaskView> = {}): TaskView {
  return { subject: [], stage: null, progress: null, counts: {}, labels: {}, flags: [], ...extra };
}
function builtin(kind: BuiltinTaskEntry['kind'], extra: Partial<BuiltinTaskEntry> = {}): BuiltinTaskEntry {
  return {
    id: `${kind}-1`, imageId: 'disk-a', status: 'running', cancelling: false, automatic: false, createdAtMs: Date.now(), startedAtMs: Date.now(),
    finishedAtMs: null, progress: null, error: null, actions: ['cancel'], source: 'builtin', kind, view: view(), ...extra,
  };
}
/** Replaces one entry, as a backend event would. */
function update(id: string, change: Partial<BuiltinTaskEntry>) {
  entries.value = entries.value.map(entry => entry.id === id ? { ...entry, ...change } as TaskEntry : entry);
}
async function mount() {
  app = createApp({ render: () => h(TaskCenterPanel, { imageId: imageId.value, onOpenResult: actions.openResult, onQuickAnalysis: actions.quickAnalysis }) });
  app.mount(host);
  await nextTick();
}
async function settle() {
  await nextTick();
  await Promise.resolve();
  await nextTick();
}
async function click(label: string) {
  const button = [...host.querySelectorAll<HTMLButtonElement>('button')].find(item => item.getAttribute('aria-label') === label || item.textContent?.trim() === label);
  if (!button) throw new Error(`Missing button: ${label}`);
  button.click();
  await settle();
}
const rowNames = () => [...host.querySelectorAll('.task-row')].map(row => row.getAttribute('aria-label'));
const sectionLabels = () => [...host.querySelectorAll('.task-card')].map(section => section.getAttribute('aria-label'));
const row = (name: string) => host.querySelector(`.task-row[aria-label="${name}"]`)!;

beforeEach(() => {
  vi.clearAllMocks();
  actions.confirm.mockResolvedValue(true);
  actions.cancel.mockResolvedValue(undefined);
  imageId.value = 'disk-a';
  entries.value = [plugin(1, 'success'), plugin(2, 'running'), plugin(3, 'queued'), plugin(4, 'failed'), plugin(5, 'pause'), plugin(6, 'offline'), plugin(7, 'cancel'), plugin(8, 'created', { internal: true })];
  host = document.createElement('div');
  document.body.append(host);
});
afterEach(() => { app?.unmount(); app = undefined; host.remove(); });

describe('task list sections', () => {
  it('shows image export progress, cancellation and the retained destination', async () => {
    entries.value = [builtin('imageExport', { id: 'export-1', view: view({ subject: ['D:/exports/disk.dd'] }), progress: { unit: 'bytes', done: 1024, total: 4096 } })];
    await mount();
    expect(row('镜像导出').textContent).toContain('D:/exports/disk.dd');
    expect(host.querySelector('[aria-label="镜像导出进度"]')?.getAttribute('aria-valuenow')).toBe('25');
    await click('取消 镜像导出');
    expect(actions.cancel).toHaveBeenCalledWith('export-1');
    update('export-1', { cancelling: true });
    await nextTick();
    expect(row('镜像导出').textContent).toContain('正在取消，请稍候');
    update('export-1', { status: 'cancel', cancelling: false, actions: [], finishedAtMs: Date.now() });
    await nextTick();
    expect(sectionLabels()).toEqual(['已结束 1 项']);
    expect(row('镜像导出').textContent).toContain('D:/exports/disk.dd');
    expect(row('镜像导出').textContent).toContain('保留部分文件');
  });

  it('shows encrypted unlocks as cancellable background work and retains the outcome', async () => {
    entries.value = [builtin('encryptedVolumeUnlock', { id: 'unlock-1', view: view({ subject: ['part:2'], labels: { scheme: 'luks' } }) })];
    await mount();
    expect(row('加密卷解锁').textContent).toContain('LUKS · 分区 2');
    expect(host.querySelector('[aria-label="加密卷解锁进度"]')?.hasAttribute('aria-valuenow')).toBe(false);
    await click('取消 加密卷解锁');
    expect(actions.cancel).toHaveBeenCalledWith('unlock-1');
    update('unlock-1', { status: 'failed', error: '密码验证失败', actions: [] });
    await nextTick();
    expect(sectionLabels()).toEqual(['需要处理 1 项']);
    await click('展开 加密卷解锁 详情');
    expect(row('加密卷解锁').textContent).toContain('密码验证失败');
    await click('打开镜像信息');
    expect(actions.openResult).toHaveBeenCalledWith('dashboard', null);
  });

  it('groups live work, failures and history, and reveals internal tasks on request', async () => {
    await mount();
    expect(sectionLabels()).toEqual(['进行中 3 项', '需要处理 2 项', '已结束 2 项']);
    expect(rowNames()).toEqual(['任务 2', '任务 5', '任务 3', '任务 6', '任务 4', '任务 7', '任务 1']);
    expect(row('任务 2').querySelector('.task-badge')?.textContent).toBe('执行中 42%');
    expect(row('任务 7').querySelector('.task-badge')?.textContent).toBe('已取消');
    expect(row('任务 1').querySelector('.task-badge')).toBeNull();
    expect(row('任务 4').querySelector('[role="progressbar"]')).toBeNull();
    await click('内部 1');
    expect(sectionLabels()[0]).toBe('进行中 4 项');
    await click('收起进行中');
    expect(row('任务 2')).toBeNull();
    expect(host.querySelector('[aria-label="展开进行中"]')?.getAttribute('aria-expanded')).toBe('false');
  });

  it('combines built-in operations with plugin tasks and respects native capabilities', async () => {
    entries.value.push(
      builtin('dockerPrepare', { id: 'docker-1', actions: [] }),
      builtin('globalSearch', { id: 'search-1', progress: { unit: 'entries', done: 2, total: 10 } }),
      builtin('timelineBuild', { id: 'timeline-build-disk-a', progress: { unit: 'entries', done: 1, total: 4 } }),
    );
    await mount();
    expect(sectionLabels()[0]).toBe('进行中 6 项');
    expect(host.querySelector('[aria-label="Docker 分析进度"]')?.hasAttribute('aria-valuenow')).toBe(false);
    expect(row('Docker 分析').querySelector('.task-badge')?.textContent).toBe('处理中');
    expect(host.querySelector('[aria-label="取消 Docker 分析"]')).toBeNull();
    await click('取消 全局搜索');
    expect(actions.cancel).toHaveBeenCalledWith('search-1');
    update('search-1', { cancelling: true });
    await nextTick();
    expect(row('全局搜索').textContent).toContain('取消中');
    expect(host.querySelector('[aria-label="取消 全局搜索"]')).toBeNull();
    update('timeline-build-disk-a', { status: 'success', actions: [], finishedAtMs: Date.now(), view: view({ counts: { fileRows: 5, eventRows: 2 } }) });
    await nextTick();
    expect(sectionLabels()[2]).toBe('已结束 3 项');
    await click('展开 时间线重建 详情');
    expect(row('时间线重建').textContent).not.toContain('删除任务');
    expect(row('时间线重建').textContent).toContain('文件 5 条 · 事件 2 条');
    expect(host.querySelector('[aria-label="重新分析 时间线重建"]')).toBeNull();
    await click('打开时间线 时间线重建');
    expect(actions.openResult).toHaveBeenCalledWith('timeline', null);
  });

  it('shows carving progress and exposes pause, resume and cancellation', async () => {
    entries.value = [builtin('carveScan', { id: 'carve-task', scanId: 'scan-1', actions: ['cancel', 'pause'], progress: { unit: 'bytes', done: 256, total: 1024 },
      view: view({ subject: ['整个镜像'], stage: 'signature', counts: { found: 3 } }) })];
    await mount();
    expect(host.querySelector('[aria-label="文件恢复进度"]')?.getAttribute('aria-valuenow')).toBe('25');
    expect(row('文件恢复').textContent).toContain('发现 3 个文件');
    await click('暂停 文件恢复'); expect(actions.pause).toHaveBeenCalledWith('carve-task');
    update('carve-task', { status: 'pause', actions: ['cancel', 'resume'] }); await nextTick();
    expect(row('文件恢复').querySelector('.task-badge')?.textContent).toBe('已暂停');
    await click('继续 文件恢复'); expect(actions.resume).toHaveBeenCalledWith('carve-task');
    await click('展开 文件恢复 详情');
    await click('取消任务');
    expect(actions.cancel).toHaveBeenCalledWith('carve-task');
    await click('打开文件恢复');
    expect(actions.openResult).toHaveBeenCalledWith('carve', null);
  });

  it('lists platform detection with its conclusion and links to the image info page', async () => {
    entries.value = [builtin('autoDetect', { id: 'autodetect:disk-a', automatic: true, status: 'success', actions: [], finishedAtMs: Date.now(),
      view: view({ labels: { platform: 'windows', evidenceType: 'disk' } }) })];
    await mount();
    expect(row('自动识别平台').querySelector('.task-summary')?.textContent).toBe('Windows · 磁盘镜像');
    await click('打开镜像信息 自动识别平台');
    expect(actions.openResult).toHaveBeenCalledWith('dashboard', null);
    await click('展开 自动识别平台 详情');
    expect(row('自动识别平台').querySelector('.technical-details')?.textContent).toContain('触发自动');
  });

  it('shows evidence loading for every evidence and names its stage', async () => {
    entries.value = [builtin('imageLoad', { id: 'load-1', imageId: null, view: view({ subject: ['D:/phone.zip'], stage: 'decompressing', labels: { item: 'data.tar' } }),
      progress: { unit: 'bytes', done: 512, total: 2048 } })];
    imageId.value = null;
    await mount();
    expect(row('检材加载').textContent).toContain('解压 data.tar · 512 B / 2.0 KB');
  });

  it('folds long history until asked and searches every match', async () => {
    entries.value = Array.from({ length: 8 }, (_, index) => plugin(index + 1, 'success'));
    await mount();
    expect(rowNames()).toHaveLength(6);
    await click('显示其余 2 项');
    expect(rowNames()).toHaveLength(8);
    imageId.value = 'disk-b';
    await settle();
    imageId.value = 'disk-a';
    await settle();
    expect(rowNames()).toHaveLength(6);
    const input = host.querySelector<HTMLInputElement>('[aria-label="搜索任务"]')!;
    input.value = '任务';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    await nextTick();
    expect(rowNames()).toHaveLength(8);
  });
});

describe('task list scope and search', () => {
  it('scopes tasks to the evidence and clears the search when evidence changes', async () => {
    const other = job(9, 'failed');
    entries.value.push(pluginTaskEntry({ ...other, info: { ...other.info, imageId: 'disk-b' } }));
    await mount();
    const input = host.querySelector<HTMLInputElement>('[aria-label="搜索任务"]')!;
    input.value = '任务 2';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    await nextTick();
    expect(rowNames()).toEqual(['任务 2']);
    imageId.value = 'disk-b';
    await settle();
    expect(input.value).toBe('');
    expect(rowNames()).toEqual(['任务 9']);
    imageId.value = null;
    await settle();
    expect(rowNames()).toEqual([]);
    expect(host.textContent).toContain('当前检材还没有任务');
    await click('快速分析');
    expect(actions.quickAnalysis).toHaveBeenCalledOnce();
  });

  it('explains an empty search and clears it', async () => {
    await mount();
    const input = host.querySelector<HTMLInputElement>('[aria-label="搜索任务"]')!;
    input.value = '不存在';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    await nextTick();
    expect(host.textContent).toContain('没有匹配的任务');
    await click('清除搜索');
    expect(rowNames()).toHaveLength(7);
  });
});

describe('task actions', () => {
  it('runs the direct action without opening details and prevents duplicate clicks', async () => {
    let finish!: () => void;
    actions.cancel.mockImplementation(() => new Promise<void>(resolve => { finish = resolve; }));
    await mount();
    // Plugin tasks never pause: a running one offers cancellation.
    expect(host.querySelector('[aria-label="暂停 任务 2"]')).toBeNull();
    await click('取消 任务 2');
    await click('取消 任务 2');
    expect(actions.cancel).toHaveBeenCalledTimes(1);
    expect(actions.cancel).toHaveBeenCalledWith('plugin:2');
    expect(host.querySelector('.task-detail')).toBeNull();
    finish();
    await settle();
    await vi.waitFor(() => expect(host.querySelector<HTMLButtonElement>('[aria-label="取消 任务 2"]')?.disabled).toBe(false));
    // A task an older version paused starts again from the beginning.
    expect(row('任务 5').querySelector('.task-action')?.textContent?.trim()).toBe('开始');
    await click('从头重新分析 任务 5');
    expect(actions.start).toHaveBeenCalledWith('plugin:5');
  });

  it('names the analyzed system and the chosen options', async () => {
    const queued = job(1, 'queued');
    entries.value = [pluginTaskEntry({ ...queued,
      name: 'test/base', info: { ...queued.info, analyzer: 'translated' },
      params: { fileName: null, nodeId: null, systemId: 'linux@part:2', targetLabel: 'Kali · part:2', options: { items: ['users', 'groups'] } },
    })];
    await mount();
    expect(rowNames()).toEqual(['Translated analyzer']);
    expect(row('Translated analyzer').querySelector('.task-summary')?.textContent).toBe('Kali · part:2');
    await click('展开 Translated analyzer 详情');
    const details = row('Translated analyzer').querySelector('.task-detail')?.textContent ?? '';
    expect(details).toContain('系统Kali · part:2');
    expect(details).toContain('子项: 用户列表、用户组');
    expect(details).toContain('linux@part:2');
  });

  it('keeps restart confirmation and places deletion inside details', async () => {
    await mount();
    expect(host.textContent).not.toContain('删除任务');
    actions.confirm.mockResolvedValue(false);
    await click('重新分析 任务 4');
    expect(actions.restart).not.toHaveBeenCalled();
    actions.confirm.mockResolvedValue(true);
    await click('重新分析 任务 4');
    expect(actions.restart).toHaveBeenCalledWith('plugin:4');
    await click('展开 任务 4 详情');
    await click('删除任务');
    expect(actions.remove).toHaveBeenCalledWith('plugin:4');
  });

  it('links stored plugin results and moves re-analysis into the details', async () => {
    entries.value = [plugin(1, 'success', { result: { summary: null, recordCount: 12, timelineCount: 3, warningCount: 2, syntheticBytes: 0, runId: 'run-1' } })];
    await mount();
    expect(row('任务 1').textContent).toContain('12 条记录 · 3 个时间点 · 2 条告警');
    await click('查看分析结果 任务 1');
    expect(actions.openResult).toHaveBeenCalledWith('analysis', 'run-1');
    await click('展开 任务 1 详情');
    await click('重新分析');
    expect(actions.restart).toHaveBeenCalledWith('plugin:1');
  });

  it('shows the full error once and keeps interrupted progress in the details', async () => {
    const failed = job(4, 'failed');
    entries.value = [pluginTaskEntry({ ...failed, info: { ...failed.info, error: '数据库已加密' }, startAt: '2026-09-19T10:05:00Z' })];
    await mount();
    expect(row('任务 4').querySelector('.task-summary')?.textContent).toBe('数据库已加密');
    await click('展开 任务 4 详情');
    expect(row('任务 4').querySelector('.task-summary')).toBeNull();
    expect(row('任务 4').querySelector('.detail-error')?.textContent).toBe('数据库已加密');
    const details = row('任务 4').querySelector('.task-detail')?.textContent ?? '';
    expect(details).toContain('42%');
    expect(details).toContain('00:00:07');
    const started = new Date(Date.UTC(2026, 8, 19, 10, 5));
    const pad = (value: number) => String(value).padStart(2, '0');
    expect(details).toContain(`${started.getFullYear()}/${pad(started.getMonth() + 1)}/${pad(started.getDate())} ${pad(started.getHours())}:${pad(started.getMinutes())}:00`);
  });
});
