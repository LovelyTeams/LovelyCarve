// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, h, nextTick, ref, type App } from 'vue';
import type { SchedTaskRecord, SchedTaskStatus } from '../../../lib/ipc/tasks';
import type { BuiltinTask } from '../model/types';
import TaskCenterPanel from './TaskCenterPanel.vue';

const actions = vi.hoisted(() => ({
  refresh: vi.fn(), cancel: vi.fn(), pause: vi.fn(), start: vi.fn(), restart: vi.fn(), remove: vi.fn(), confirm: vi.fn(), nativeCancel: vi.fn(), nativePause: vi.fn(), nativeStart: vi.fn(),
  openResult: vi.fn(), quickAnalysis: vi.fn(),
}));
const records = ref<SchedTaskRecord[]>([]);
const builtinRecords = ref<BuiltinTask[]>([]);
vi.mock('../composables/useBuiltinTasks', () => ({ useBuiltinTasks: () => ({ tasks: builtinRecords, now: ref(Date.now()), refresh: vi.fn(), cancel: actions.nativeCancel, pause: actions.nativePause, start: actions.nativeStart }) }));
vi.mock('../composables/useTaskCenter', async importOriginal => ({
  ...await importOriginal<typeof import('../composables/useTaskCenter')>(),
  useTaskCenter: () => ({ tasks: records, notices: ref([]), loading: ref(false), error: ref(null), ...actions }),
}));
vi.mock('../../../composables/useConfirm', () => ({ confirm: actions.confirm }));

let app: App | undefined;
let host: HTMLDivElement;
const imageId = ref<string | null>('disk-a');
function task(id: number, status: SchedTaskStatus, extra: Partial<SchedTaskRecord> = {}): SchedTaskRecord {
  return {
    id, status, name: `test/${id}`, displayName: `任务 ${id}`, typeName: '系统分析', evidenceId: 1,
    progress: 42, canPause: true, singleInstance: false, internal: false, dependence: '',
    spendTimeSecs: 7, remainTimeSecs: null, startAt: null, createAt: `2026-09-19 10:${String(id).padStart(2, '0')}:00`, updateAt: '',
    info: { imageId: 'disk-a', pluginId: 'test', analyzer: 'base', trigger: 'manually', parentTid: null, error: null },
    params: { fileName: null, nodeId: null, platform: null, subItems: [] }, result: null, ...extra,
  };
}
function builtin(kind: BuiltinTask['kind'], extra: Partial<BuiltinTask> = {}): BuiltinTask {
  return {
    id: `${kind}:1`, kind, imageId: 'disk-a', title: kind, subject: '测试对象', trigger: 'manually', status: 'running', progress: 30, message: '正在处理', error: null,
    started: Date.now(), finished: null, cancelId: 'native-1', cancelling: false, results: 0, timelineRows: 0, ...extra,
  };
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
  actions.pause.mockResolvedValue(null);
  imageId.value = 'disk-a';
  builtinRecords.value = [];
  records.value = [task(1, 'success'), task(2, 'running'), task(3, 'queued'), task(4, 'failed'), task(5, 'pause'), task(6, 'offline'), task(7, 'cancel'), task(8, 'created', { internal: true })];
  host = document.createElement('div');
  document.body.append(host);
});
afterEach(() => { app?.unmount(); app = undefined; host.remove(); });

describe('task list sections', () => {
  it('shows image export progress, cancellation and the retained destination', async () => {
    records.value = [];
    builtinRecords.value = [builtin('imageExport', {
      title: '镜像导出', subject: 'D:/exports/disk.dd', progress: 25,
      message: '1 KiB / 4 KiB', cancelId: 'export-1',
    })];
    await mount();
    expect(row('镜像导出').textContent).toContain('D:/exports/disk.dd');
    expect(host.querySelector('[aria-label="镜像导出进度"]')?.getAttribute('aria-valuenow')).toBe('25');
    await click('取消 镜像导出');
    expect(actions.nativeCancel).toHaveBeenCalledWith('imageExport:1');
    builtinRecords.value[0].cancelling = true;
    await nextTick();
    expect(row('镜像导出').textContent).toContain('正在取消，请稍候');
    builtinRecords.value[0].status = 'cancel';
    builtinRecords.value[0].cancelling = false;
    builtinRecords.value[0].message = '导出已取消（保留部分文件）';
    await nextTick();
    expect(sectionLabels()).toEqual(['已结束 1 项']);
    expect(row('镜像导出').textContent).toContain('D:/exports/disk.dd');
    expect(row('镜像导出').textContent).toContain('保留部分文件');
  });

  it('shows encrypted unlocks as cancellable background work and retains the outcome', async () => {
    records.value = [];
    builtinRecords.value = [builtin('encryptedUnlock', {
      title: '加密卷解锁', subject: 'LUKS · 分区 2', progress: null,
      message: '正在派生密钥并解锁', cancelId: 'unlock-native',
    })];
    await mount();
    expect(row('加密卷解锁').textContent).toContain('LUKS · 分区 2');
    expect(host.querySelector('[aria-label="加密卷解锁进度"]')?.hasAttribute('aria-valuenow')).toBe(false);
    await click('取消 加密卷解锁');
    expect(actions.nativeCancel).toHaveBeenCalledWith('encryptedUnlock:1');
    builtinRecords.value[0].status = 'failed';
    builtinRecords.value[0].error = '密码验证失败';
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
    builtinRecords.value = [
      builtin('docker', { id: 'docker:1', title: 'Docker 分析', progress: null, cancelId: null }),
      builtin('search', { id: 'search:1', title: '全局搜索' }),
      builtin('timeline', { id: 'timeline:1', title: '时间线重建', cancelId: 'timeline-build-disk-a' }),
    ];
    await mount();
    expect(sectionLabels()[0]).toBe('进行中 6 项');
    expect(host.querySelector('[aria-label="Docker 分析进度"]')?.hasAttribute('aria-valuenow')).toBe(false);
    expect(row('Docker 分析').querySelector('.task-badge')?.textContent).toBe('处理中');
    expect(host.querySelector('[aria-label="取消 Docker 分析"]')).toBeNull();
    await click('取消 全局搜索');
    expect(actions.nativeCancel).toHaveBeenCalledWith('search:1');
    expect(actions.cancel).not.toHaveBeenCalled();
    builtinRecords.value[1].cancelling = true;
    await nextTick();
    expect(row('全局搜索').textContent).toContain('取消中');
    expect(host.querySelector('[aria-label="取消 全局搜索"]')).toBeNull();
    builtinRecords.value[2].status = 'success';
    await nextTick();
    expect(sectionLabels()[2]).toBe('已结束 3 项');
    await click('展开 时间线重建 详情');
    expect(row('时间线重建').textContent).not.toContain('删除任务');
    expect(host.querySelector('[aria-label="重新分析 时间线重建"]')).toBeNull();
    await click('打开时间线 时间线重建');
    expect(actions.openResult).toHaveBeenCalledWith('timeline', null);
  });

  it('shows carving progress and exposes native pause, resume and cancellation', async () => {
    builtinRecords.value = [builtin('carve', { id: 'carve:scan', title: '文件雕刻', subject: '整个镜像', progress: 25,
      message: '已扫描 256 B / 1 KB · 发现 3 个文件', cancelId: 'native-scan', results: 3 })];
    await mount();
    expect(host.querySelector('[aria-label="文件雕刻进度"]')?.getAttribute('aria-valuenow')).toBe('25');
    await click('暂停 文件雕刻'); expect(actions.nativePause).toHaveBeenCalledWith('carve:scan');
    builtinRecords.value[0].status = 'pause'; await nextTick();
    expect(row('文件雕刻').querySelector('.task-badge')?.textContent).toBe('已暂停');
    await click('继续 文件雕刻'); expect(actions.nativeStart).toHaveBeenCalledWith('carve:scan');
    await click('展开 文件雕刻 详情');
    await click('取消任务');
    expect(actions.nativeCancel).toHaveBeenCalledWith('carve:scan');
    await click('打开文件雕刻');
    expect(actions.openResult).toHaveBeenCalledWith('carve', null);
  });

  it('lists platform detection with its conclusion and links to the image info page', async () => {
    builtinRecords.value = [builtin('autodetect', { id: 'autodetect:disk-a', title: '自动识别平台', subject: '', trigger: 'auto', status: 'success',
      progress: 100, message: 'Windows · 磁盘镜像', cancelId: 'autodetect:disk-a', finished: Date.now() })];
    await mount();
    expect(row('自动识别平台').querySelector('.task-summary')?.textContent).toBe('Windows · 磁盘镜像');
    await click('打开镜像信息 自动识别平台');
    expect(actions.openResult).toHaveBeenCalledWith('dashboard', null);
    await click('展开 自动识别平台 详情');
    expect(row('自动识别平台').querySelector('.technical-details')?.textContent).toContain('触发自动');
  });

  it('folds long history until asked and searches every match', async () => {
    records.value = Array.from({ length: 8 }, (_, index) => task(index + 1, 'success'));
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
    records.value.push(task(9, 'failed', { info: { ...task(9, 'failed').info, imageId: 'disk-b' } }));
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
    actions.pause.mockImplementation(() => new Promise<void>(resolve => { finish = resolve; }));
    await mount();
    await click('暂停 任务 2');
    await click('暂停 任务 2');
    expect(actions.pause).toHaveBeenCalledTimes(1);
    expect(actions.pause).toHaveBeenCalledWith(2);
    expect(host.querySelector('.task-detail')).toBeNull();
    finish();
    await settle();
    await vi.waitFor(() => expect(host.querySelector<HTMLButtonElement>('[aria-label="暂停 任务 2"]')?.disabled).toBe(false));
    await click('继续 任务 5');
    expect(actions.start).toHaveBeenCalledWith(5);
  });

  it('keeps restart confirmation and places deletion inside details', async () => {
    await mount();
    expect(host.textContent).not.toContain('删除任务');
    actions.confirm.mockResolvedValue(false);
    await click('重新分析 任务 4');
    expect(actions.restart).not.toHaveBeenCalled();
    actions.confirm.mockResolvedValue(true);
    await click('重新分析 任务 4');
    expect(actions.restart).toHaveBeenCalledWith(4);
    await click('展开 任务 4 详情');
    await click('删除任务');
    expect(actions.remove).toHaveBeenCalledWith(4);
  });

  it('links stored plugin results and moves re-analysis into the details', async () => {
    records.value = [task(1, 'success', { result: { summary: null, artifactCount: 12, timelineCount: 3, findingCount: 0, runId: 'run-1' } })];
    await mount();
    expect(row('任务 1').textContent).toContain('12 条结果 · 3 条时间线');
    await click('查看分析结果 任务 1');
    expect(actions.openResult).toHaveBeenCalledWith('analysis', 'base');
    await click('展开 任务 1 详情');
    await click('重新分析');
    expect(actions.restart).toHaveBeenCalledWith(1);
  });

  it('shows the full error once and keeps interrupted progress in the details', async () => {
    records.value = [task(4, 'failed', { info: { ...task(4, 'failed').info, error: '数据库已加密' }, startAt: '2026-09-19 10:05:00' })];
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
