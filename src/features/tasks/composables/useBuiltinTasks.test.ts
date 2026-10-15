import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { effectScope, reactive, shallowRef } from 'vue';
import type { GlobalSearchHistoryEntry, GlobalSearchOptions, GlobalSearchProgress, GlobalSearchSummary } from '../../../lib/ipc/search';
import type { DockerIndexStatus } from '../../../lib/ipc/fs/types';
import type { TimelineBuildSummary, TimelineProgress } from '../../../lib/ipc/timeline';
import type { AutoDetectFailure, AutoDetectReport } from '../../../lib/ipc/autodetect';
import { resetBuiltinTasksForTests, trackedImageExport, trackedCarveStart, trackedFsChildren, trackedFsFlatPage, trackedGlobalSearchStart, trackedTimelineStart, trackedEncryptedVolumeUnlock, useBuiltinTasks } from './useBuiltinTasks';
import { IpcCommandError } from '../../../lib/ipc/client';
import type { EncryptedUnlockResult } from '../../../lib/ipc/encrypted';
import { resetAutoDetectForTests, runAutoDetect, useAutoDetectReport, ensureAutoDetectListener, isAutoDetectRunPending } from '../../evidence/composables/useAutoDetect';

import { registerTaskContext } from './taskContext';

const ipc = vi.hoisted(() => ({ nativeList: vi.fn(), lifecycle: vi.fn(), snapshotEvents: vi.fn(), children: vi.fn(), docker: vi.fn(), flat: vi.fn(), cancel: vi.fn(), search: vi.fn(), history: vi.fn(), searchEvents: vi.fn(), timeline: vi.fn(), timelineStatus: vi.fn(), timelineEvents: vi.fn(), carve: vi.fn(), carveLatest: vi.fn(), carveStatus: vi.fn(), carveEvents: vi.fn(), carvePause: vi.fn(), carveResume: vi.fn(), detect: vi.fn(), detectFinished: vi.fn(), detectFailed: vi.fn(), reportEvents: vi.fn() }));
vi.mock('../../../lib/ipc/carve', () => ({ carveStart: ipc.carve, carveLatestScan: ipc.carveLatest, carveScanStatus: ipc.carveStatus, onCarveScanProgress: ipc.carveEvents, carveScanPause: ipc.carvePause, carveScanResume: ipc.carveResume }));
vi.mock('../../../lib/ipc/fs/query', () => ({ fsChildren: ipc.children, fsDockerPrepare: ipc.docker, fsFlatPage: ipc.flat }));
vi.mock('../../../lib/ipc/taskRuntime', () => ({ taskCancel: ipc.cancel, taskList: ipc.nativeList, onTaskLifecycle: ipc.lifecycle }));
vi.mock('../../../lib/ipc/cases', () => ({ onSnapshotProgress: ipc.snapshotEvents }));
vi.mock('../../../lib/ipc/search', () => ({ globalSearchStart: ipc.search, globalSearchHistoryList: ipc.history, onGlobalSearchProgress: ipc.searchEvents }));
vi.mock('../../../lib/ipc/timeline', () => ({ timelineStart: ipc.timeline, timelineStatus: ipc.timelineStatus, onTimelineProgress: ipc.timelineEvents }));
vi.mock('../../../lib/ipc/autodetect', () => ({ AUTODETECT_FINISHED_EVENT: 'autodetect-finished', AUTODETECT_FAILED_EVENT: 'autodetect-failed', autodetectEvidence: ipc.detect, onAutoDetectFinished: ipc.detectFinished, onAutoDetectFailed: ipc.detectFailed }));
vi.mock('../../../lib/ipc/events', () => ({ onEvent: ipc.reportEvents }));
const activeCase = shallowRef<{ dir: string; evidence: { image_id: string }[] } | null>(null);
const encrypted = vi.hoisted(() => ({ unlock: vi.fn() }));
vi.mock('../../../lib/ipc/encrypted', () => ({ encryptedVolumeUnlock: encrypted.unlock }));
vi.mock('../../cases/composables/useCaseSession', () => ({ useCaseSession: () => ({ activeCase, refreshActive: async () => {} }) }));

let snapshotProgress: (event: { payload: import('../../../lib/ipc/cases').SnapshotProgress }) => void;
let nativeLifecycle: (event: { payload: import('../../../lib/ipc/taskRuntime').BackendTaskLifecycleEvent }) => void;
let carveProgress: (event: { payload: import('../../../lib/ipc/carve').CarveScanSnapshot }) => void;
let searchProgress: (event: { payload: GlobalSearchProgress }) => void;
let timelineProgress: (event: { payload: TimelineProgress }) => void;
let detectFinished: (event: { payload: AutoDetectReport }) => void;
let detectFailed: (event: { payload: AutoDetectFailure }) => void;
const options: GlobalSearchOptions = { keys: ['password'], regexes: [], codepages: ['UTF-8'], match_case: false, search_deleted: false, search_empty_space: false, calc_hash: false, scope: { partitions: [], tags: [], tree_nodes: [] }, per_file_cap: 100, stop_after_first: false, global_cap: 1000 };
const searchArgs = { imageId: 'disk-a', searchId: 'query-1', options };
const summary: GlobalSearchSummary = { search_id: 'query-1', status: 'success', keyword_list: ['password'], regex_list: [], codepages: ['UTF-8'], matched_file_count: 2, total_matches: 4, record_count: 4, elapsed_ms: 1000, files_scanned: 10, files_total: 10, read_errors: 0, truncated: false };
const timelineSummary: TimelineBuildSummary = { status: 'success', files_total: 10, file_rows: 10, events_total: 3, event_rows: 3, skipped_invalid_time: 0 };
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
async function settle() { for (let index = 0; index < 12; index++) await Promise.resolve(); }
function history(status = 'success'): GlobalSearchHistoryEntry {
  return { search_id: 'query-1', image_id: 'disk-a', keywords: ['password'], regexes: [], codepages: [], match_case: false, search_deleted: false, search_empty_space: false, calc_hash: false, scope_json: '{}', status, error: null, started_ms: Date.now() - 1000, finished_ms: status === 'running' ? null : Date.now(), files_total: 10, files_scanned: 10, matched_file_count: 2, total_matches: 4, record_count: 4, read_errors: 0, truncated: false };
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-09-19T12:00:00Z'));
  vi.resetAllMocks();
  activeCase.value = { dir: 'case-a', evidence: [{ image_id: 'disk-a' }] };
  ipc.nativeList.mockResolvedValue([]);
  ipc.lifecycle.mockImplementation(async callback => { nativeLifecycle = callback; return vi.fn(); });
  ipc.snapshotEvents.mockImplementation(async callback => { snapshotProgress = callback; return vi.fn(); });
  ipc.carveLatest.mockResolvedValue(null);
  ipc.carveEvents.mockImplementation(async callback => { carveProgress = callback; return vi.fn(); });
  ipc.history.mockResolvedValue([]);
  ipc.timelineStatus.mockResolvedValue({ status: 'buildRequired', meta: null });
  ipc.cancel.mockResolvedValue(undefined);
  ipc.children.mockResolvedValue([]);
  ipc.docker.mockImplementation(async ({ mode }) => ({ ready: mode === 'build', persisted: mode === 'build', entryCount: 12 }));
  ipc.flat.mockResolvedValue({ items: [], total: 0 });
  ipc.searchEvents.mockImplementation(async callback => { searchProgress = callback; return vi.fn(); });
  ipc.timelineEvents.mockImplementation(async callback => { timelineProgress = callback; return vi.fn(); });
  ipc.detectFinished.mockImplementation(async callback => { detectFinished = callback; return vi.fn(); });
  ipc.detectFailed.mockImplementation(async callback => { detectFailed = callback; return vi.fn(); });
  ipc.reportEvents.mockResolvedValue(vi.fn());
  registerTaskContext({ activeCase: () => activeCase.value, ensureDetection: ensureAutoDetectListener, detectionPending: isAutoDetectRunPending });
});
afterEach(() => { resetBuiltinTasksForTests(); resetAutoDetectForTests(); vi.useRealTimers(); });

describe('built-in task lifecycle', () => {
  it('tracks background unlock completion after page disposal without retaining credentials', async () => {
    const operation = deferred<EncryptedUnlockResult>();
    encrypted.unlock.mockReturnValue(operation.promise);
    const page = effectScope();
    const hub = page.run(() => useBuiltinTasks())!;
    const options = { scheme: 'luks' as const, password: 'secret', partitionId: 2 };
    const running = trackedEncryptedVolumeUnlock({ imageId: 'disk-a', taskId: 'unlock-1', options }, 'LUKS · 分区 2');
    await settle();
    expect(options.password).toBe('');
    expect(encrypted.unlock).toHaveBeenCalledWith({ imageId: 'disk-a', taskId: 'unlock-1', options: { scheme: 'luks', password: 'secret', partitionId: 2 } });
    page.stop();
    nativeLifecycle({ payload: { phase: 'finished', task: { id: 'unlock-1', kind: 'encryptedVolumeUnlock', imageId: 'disk-a', status: 'running', resourceClasses: [], elapsedMs: 100 } } });
    expect(hub.tasks.value[0]).toMatchObject({ kind: 'encryptedUnlock', title: '加密卷解锁', status: 'running', progress: null, cancelId: 'unlock-1' });
    operation.resolve({ cipher: 'LUKS2 aes-xts-plain64' } as EncryptedUnlockResult);
    await running;
    expect(hub.tasks.value[0]).toMatchObject({ status: 'success', progress: 100, message: '已解锁 · LUKS2 aes-xts-plain64' });
  });

  it('cancels the real unlock task and records cancellation instead of failure', async () => {
    const operation = deferred<EncryptedUnlockResult>();
    encrypted.unlock.mockReturnValue(operation.promise);
    const hub = useBuiltinTasks();
    const running = trackedEncryptedVolumeUnlock({ imageId: 'disk-a', taskId: 'unlock-2', options: { scheme: 'luks', password: 'secret' } }, 'LUKS');
    await settle();
    await hub.cancel('encryptedUnlock:unlock-2');
    expect(ipc.cancel).toHaveBeenCalledExactlyOnceWith({ taskId: 'unlock-2' });
    expect(hub.tasks.value[0]).toMatchObject({ status: 'running', cancelling: true });
    operation.reject(new IpcCommandError({ code: 'cancelled', message: 'cancelled', context: null, retryable: false }));
    await expect(running).rejects.toThrow('cancelled');
    expect(hub.tasks.value[0]).toMatchObject({ status: 'cancel', cancelling: false, error: null });
  });

  it('preserves an unlock failure in the task center', async () => {
    encrypted.unlock.mockRejectedValue(new Error('authentication failed'));
    const hub = useBuiltinTasks();
    await expect(trackedEncryptedVolumeUnlock({ imageId: 'disk-a', taskId: 'unlock-3', options: { scheme: 'luks', password: 'wrong' } }, 'LUKS')).rejects.toThrow('authentication failed');
    await hub.refresh();
    expect(hub.tasks.value[0]).toMatchObject({ status: 'failed', error: 'Error: authentication failed' });
  });

  it('replays an early cancellation once the native unlock is registered', async () => {
    const operation = deferred<EncryptedUnlockResult>();
    encrypted.unlock.mockReturnValue(operation.promise);
    const hub = useBuiltinTasks();
    const running = trackedEncryptedVolumeUnlock({ imageId: 'disk-a', taskId: 'unlock-early', options: { scheme: 'luks', password: 'secret' } }, 'LUKS');
    await settle();
    await hub.cancel('encryptedUnlock:unlock-early');
    nativeLifecycle({ payload: { phase: 'queued', task: { id: 'unlock-early', kind: 'encryptedVolumeUnlock', imageId: 'disk-a', status: 'queued', resourceClasses: [], elapsedMs: 0 } } });
    await settle();
    expect(ipc.cancel.mock.calls).toEqual([[{ taskId: 'unlock-early' }], [{ taskId: 'unlock-early' }]]);
    expect(hub.tasks.value[0]).toMatchObject({ status: 'queued', cancelling: true });
    operation.reject(new IpcCommandError({ code: 'cancelled', message: 'cancelled', context: null, retryable: false }));
    await expect(running).rejects.toThrow('cancelled');
    expect(hub.tasks.value[0].status).toBe('cancel');
  });

  it('keeps progress and completion after the initiating page scope stops', async () => {
    const operation = deferred<GlobalSearchSummary>();
    ipc.search.mockReturnValue(operation.promise);
    const page = effectScope();
    const hub = page.run(() => useBuiltinTasks())!;
    const running = trackedGlobalSearchStart(searchArgs);
    await settle();
    page.stop();
    searchProgress({ payload: { search_id: 'query-1', image_id: 'disk-a', files_done: 3, files_total: 10, hits: 2, matched_files: 1 } });
    expect(hub.tasks.value[0]).toMatchObject({ title: '全局搜索', subject: 'password', status: 'running', progress: 30, results: 2 });
    operation.resolve(summary);
    await running;
    expect(hub.tasks.value[0]).toMatchObject({ status: 'success', progress: 100, results: 4, finished: Date.now() });
  });

  it('tracks timeline progress and waits for the real cancellation result', async () => {
    const operation = deferred<TimelineBuildSummary>();
    ipc.timeline.mockReturnValue(operation.promise);
    const hub = useBuiltinTasks();
    const running = trackedTimelineStart({ imageId: 'disk-a' });
    await settle();
    timelineProgress({ payload: { image_id: 'disk-a', done: 4, total: 0, rows: 4 } });
    expect(hub.tasks.value[0]).toMatchObject({ progress: null, message: '已处理 4 项 · 4 条时间线' });
    timelineProgress({ payload: { image_id: 'disk-a', done: 4, total: 10, rows: 4 } });
    await hub.cancel('timeline:disk-a');
    await hub.cancel('timeline:disk-a');
    expect(ipc.cancel).toHaveBeenCalledExactlyOnceWith({ taskId: 'timeline-build-disk-a' });
    expect(hub.tasks.value[0]).toMatchObject({ status: 'running', progress: 40, cancelling: true });
    operation.resolve({ ...timelineSummary, status: 'cancelled', file_rows: 4, event_rows: 0 });
    await running;
    expect(hub.tasks.value[0]).toMatchObject({ status: 'cancel', progress: 40, cancelling: false, timelineRows: 4 });
  });

  it('routes search cancellation to the original id and allows retry after a cancellation error', async () => {
    const operation = deferred<GlobalSearchSummary>();
    ipc.search.mockReturnValue(operation.promise);
    const hub = useBuiltinTasks();
    const running = trackedGlobalSearchStart(searchArgs);
    await settle();
    ipc.cancel.mockRejectedValueOnce(new Error('offline'));
    await expect(hub.cancel('search:query-1')).rejects.toThrow('offline');
    expect(hub.tasks.value[0].cancelling).toBe(false);
    await hub.cancel('search:query-1');
    expect(ipc.cancel).toHaveBeenLastCalledWith({ taskId: 'query-1' });
    operation.resolve({ ...summary, status: 'cancelled' });
    await running;
    expect(hub.tasks.value[0].status).toBe('cancel');
  });

  it('keeps an IPC failure visible even if the backend never created history', async () => {
    ipc.search.mockRejectedValue(new Error('读取失败'));
    const hub = useBuiltinTasks();
    await expect(trackedGlobalSearchStart(searchArgs)).rejects.toThrow('读取失败');
    await hub.refresh();
    expect(hub.tasks.value[0]).toMatchObject({ status: 'failed', error: 'Error: 读取失败' });
  });

  it('shares one Docker task across tree/list reads and skips ordinary directories', async () => {
    const hub = useBuiltinTasks();
    ipc.children.mockResolvedValueOnce([{ id: 'host-docker:container:', name: 'web · nginx' }]);
    await trackedFsChildren({ imageId: 'disk-a', parentId: 'host-docker:' });
    expect(hub.tasks.value).toHaveLength(0);
    const operation = deferred<DockerIndexStatus>();
    ipc.docker.mockResolvedValueOnce({ ready: false }).mockReturnValueOnce(operation.promise);
    const treeRead = trackedFsChildren({ imageId: 'disk-a', parentId: 'host-docker:container:' });
    await settle();
    const flatRead = trackedFsFlatPage({ imageId: 'disk-a', parentId: 'host-docker:container:', deepsearch: false, skip: 0, limit: 100 });
    expect(hub.tasks.value).toHaveLength(1);
    expect(hub.tasks.value[0]).toMatchObject({ title: 'Docker 分析', subject: 'web · nginx', status: 'running', progress: null, cancelId: null });
    expect(ipc.docker).toHaveBeenCalledTimes(2);
    expect(ipc.flat).not.toHaveBeenCalled();
    expect(ipc.children).toHaveBeenCalledTimes(1);
    operation.resolve({ ready: true, persisted: true, entryCount: 12 });
    await Promise.all([treeRead, flatRead]);
    ipc.docker.mockResolvedValue({ ready: true, persisted: true, entryCount: 12 });
    await trackedFsChildren({ imageId: 'disk-a', parentId: 'host-docker:container:/etc' });
    expect(hub.tasks.value).toHaveLength(1);
    expect(hub.tasks.value[0].status).toBe('success');
    expect(hub.tasks.value[0].message).toContain('索引已保存');
  });

  it('allows a failed Docker merge to retry', async () => {
    const hub = useBuiltinTasks();
    const args = { imageId: 'disk-a', parentId: 'host-docker:container:' };
    ipc.docker.mockResolvedValueOnce({ ready: false }).mockRejectedValueOnce(new Error('层读取失败'));
    await expect(trackedFsChildren(args)).rejects.toThrow('层读取失败');
    expect(hub.tasks.value[0].status).toBe('failed');
    await trackedFsChildren(args);
    expect(hub.tasks.value).toHaveLength(1);
    expect(hub.tasks.value[0]).toMatchObject({ status: 'success', error: null });
  });

  it('reuses a persisted container after reopening without starting an analysis task', async () => {
    const hub = useBuiltinTasks();
    ipc.docker.mockResolvedValue({ ready: true, persisted: true, entryCount: 12 });
    await trackedFsChildren({ imageId: 'disk-a', parentId: 'host-docker:container:' });
    expect(hub.tasks.value).toEqual([]);
    expect(ipc.docker).toHaveBeenCalledExactlyOnceWith({ imageId: 'disk-a', rootId: 'host-docker:container:', caseDir: 'case-a', mode: 'status' });
    expect(ipc.children).toHaveBeenCalledTimes(1);
  });

  it('does not start an old case build after its cache probe finishes', async () => {
    const probe = deferred<DockerIndexStatus>();
    const hub = useBuiltinTasks();
    ipc.docker.mockReturnValueOnce(probe.promise);
    const reading = trackedFsChildren({ imageId: 'disk-a', parentId: 'host-docker:container:' });
    activeCase.value = { dir: 'case-b', evidence: [{ image_id: 'disk-a' }] };
    probe.resolve({ ready: false, persisted: false, entryCount: 0 });
    await expect(reading).rejects.toThrow('案件已切换');
    expect(ipc.docker).toHaveBeenCalledTimes(1);
    expect(ipc.children).not.toHaveBeenCalled();
    expect(hub.tasks.value).toEqual([]);
  });

  it('does not report successful persistence when the index write fails', async () => {
    const hub = useBuiltinTasks();
    ipc.docker.mockResolvedValueOnce({ ready: false }).mockRejectedValueOnce(new Error('保存 Docker 查询索引失败'));
    await expect(trackedFsChildren({ imageId: 'disk-a', parentId: 'host-docker:container:' })).rejects.toThrow('查询索引失败');
    expect(hub.tasks.value[0]).toMatchObject({ status: 'failed', progress: null });
    expect(ipc.children).not.toHaveBeenCalled();
  });

  it('does not mix late completions across an A → B → A case switch', async () => {
    const operation = deferred<GlobalSearchSummary>();
    ipc.search.mockReturnValue(operation.promise);
    const hub = useBuiltinTasks();
    const running = trackedGlobalSearchStart(searchArgs);
    await settle();
    activeCase.value = { dir: 'case-b', evidence: [{ image_id: 'disk-b' }] };
    activeCase.value = { dir: 'case-a', evidence: [{ image_id: 'disk-a' }] };
    await settle();
    operation.resolve(summary);
    await running;
    expect(hub.tasks.value).toEqual([]);
  });

  it('rejects duplicate timeline starts without replacing the original task', async () => {
    const operation = deferred<TimelineBuildSummary>();
    ipc.timeline.mockReturnValue(operation.promise);
    const hub = useBuiltinTasks();
    const running = trackedTimelineStart({ imageId: 'disk-a' });
    await settle();
    await expect(trackedTimelineStart({ imageId: 'disk-a' })).rejects.toThrow('任务正在执行');
    expect(ipc.timeline).toHaveBeenCalledTimes(1);
    operation.resolve(timelineSummary);
    await running;
    expect(hub.tasks.value[0]).toMatchObject({ status: 'success', timelineRows: 13 });
  });

  it('restores history, removes deleted searches, and polls externally running tasks', async () => {
    ipc.history.mockResolvedValue([history('running')]);
    ipc.timelineStatus.mockResolvedValue({ meta: { eid: 'disk-a', status: 'success', error: null, started_ms: Date.now() - 1000, finished_ms: Date.now(), file_rows: 10, event_rows: 3 } });
    const hub = useBuiltinTasks();
    await settle();
    expect(hub.tasks.value.map(task => task.status)).toEqual(['running', 'success']);
    ipc.history.mockResolvedValue([history('success')]);
    await vi.advanceTimersByTimeAsync(2000);
    expect(hub.tasks.value.find(task => task.kind === 'search')?.status).toBe('success');
    ipc.history.mockResolvedValue([]);
    await hub.refresh();
    expect(hub.tasks.value.map(task => task.kind)).toEqual(['timeline']);
  });

  it('ignores a stale refresh that resolves after a locally started search finishes', async () => {
    const oldHistory = deferred<GlobalSearchHistoryEntry[]>();
    ipc.history.mockReturnValueOnce(oldHistory.promise);
    ipc.search.mockResolvedValue(summary);
    const hub = useBuiltinTasks();
    await trackedGlobalSearchStart(searchArgs);
    oldHistory.resolve([history('running')]);
    await settle();
    expect(hub.tasks.value[0].status).toBe('success');
  });

  it('hydrates the terminal result for an external timeline first discovered through progress', async () => {
    const hub = useBuiltinTasks();
    await settle();
    timelineProgress({ payload: { image_id: 'disk-a', done: 4, total: 10, rows: 4 } });
    ipc.timelineStatus.mockResolvedValue({ meta: { eid: 'disk-a', status: 'success', error: null, started_ms: Date.now() - 5000, finished_ms: Date.now(), file_rows: 10, event_rows: 3 } });
    await hub.refresh();
    expect(hub.tasks.value[0]).toMatchObject({ status: 'success', timelineRows: 13 });
  });

  it('does not restore an older successful timeline over a new failed rebuild', async () => {
    ipc.timelineStatus.mockResolvedValue({ meta: { eid: 'disk-a', status: 'success', error: null, started_ms: Date.now() - 5000, finished_ms: Date.now() - 1000, file_rows: 10, event_rows: 3 } });
    ipc.timeline.mockRejectedValue(new Error('构建失败'));
    const hub = useBuiltinTasks();
    await settle();
    await expect(trackedTimelineStart({ imageId: 'disk-a' })).rejects.toThrow('构建失败');
    await hub.refresh();
    expect(hub.tasks.value[0]).toMatchObject({ status: 'failed', error: 'Error: 构建失败' });
  });
});

const carveOptions = { format_ids: ['jpg'], scope: 'whole' as const, partition_id: null, max_file_size: 1024, max_results: 100 };
function carving(status: import('../../../lib/ipc/carve').CarveSessionStatus = 'running'): import('../../../lib/ipc/carve').CarveScanSnapshot {
  return { scan_id: 'carve-a', task_id: 'native-carve-a', image_id: 'disk-a', created_unix_ms: Date.now() - 1000,
    status, scope_label: '整个镜像', bytes_scanned: 256, bytes_total: 1024, artifacts_found: 3, signatures_found: 5,
    current_offset: 256, truncated: false, error: null };
}
describe('carving tasks', () => {
  it('does not mark launch as completion and keeps tracking after the page is disposed', async () => {
    ipc.carve.mockResolvedValue({ scan_id: 'carve-a', task_id: 'native-carve-a' });
    ipc.carveStatus.mockResolvedValue(carving('queued'));
    const page = effectScope(); const hub = page.run(() => useBuiltinTasks())!;
    await trackedCarveStart({ imageId: 'disk-a', options: carveOptions });
    expect(hub.tasks.value[0]).toMatchObject({ id: 'carve:carve-a', title: '文件雕刻', status: 'queued', progress: 25, finished: null });
    page.stop(); carveProgress({ payload: carving() });
    expect(hub.tasks.value[0]).toMatchObject({ status: 'running', results: 3, progress: 25 });
    carveProgress({ payload: { ...carving('completed'), artifacts_found: 8, truncated: true } });
    expect(hub.tasks.value[0]).toMatchObject({ status: 'success', results: 8, progress: 100, finished: Date.now() });
    expect(hub.tasks.value[0].message).toContain('已达结果上限');
    carveProgress({ payload: carving() });
    expect(hub.tasks.value[0].status).toBe('success');
  });
  it('restores an existing scan and routes pause, resume and cancellation to its actual IDs', async () => {
    ipc.carveLatest.mockResolvedValue(carving());
    const hub = useBuiltinTasks(); await settle();
    ipc.carvePause.mockResolvedValue(carving('paused'));
    await hub.pause('carve:carve-a');
    expect(ipc.carvePause).toHaveBeenCalledWith({ scanId: 'carve-a' });
    expect(hub.tasks.value[0].status).toBe('pause');
    ipc.carveResume.mockResolvedValue(carving());
    await hub.start('carve:carve-a');
    expect(ipc.carveResume).toHaveBeenCalledWith({ scanId: 'carve-a' });
    carveProgress({ payload: carving('paused') });
    await hub.cancel('carve:carve-a');
    expect(ipc.cancel).toHaveBeenCalledWith({ taskId: 'native-carve-a' });
    expect(hub.tasks.value[0]).toMatchObject({ status: 'pause', cancelling: true });
    carveProgress({ payload: carving('cancelled') });
    expect(hub.tasks.value[0]).toMatchObject({ status: 'cancel', cancelling: false, progress: 25 });
  });
  it('polls queued scans and preserves native failure details when an event is missed', async () => {
    ipc.carveLatest.mockResolvedValue(carving('queued')); ipc.carveStatus.mockResolvedValue(carving('queued'));
    const hub = useBuiltinTasks(); await settle();
    const failed = { ...carving('failed'), error: '读取镜像失败' };
    ipc.carveLatest.mockResolvedValue(failed); ipc.carveStatus.mockResolvedValue(failed);
    await vi.advanceTimersByTimeAsync(2000);
    expect(hub.tasks.value[0]).toMatchObject({ status: 'failed', error: '读取镜像失败', progress: 25 });
  });
  it('ignores old case completions and late status responses after a live progress update', async () => {
    const pending = deferred<import('../../../lib/ipc/carve').CarveScanSnapshot | null>();
    ipc.carveLatest.mockReturnValue(pending.promise);
    const hub = useBuiltinTasks(); await settle();
    carveProgress({ payload: carving('paused') });
    pending.resolve(carving()); await settle();
    expect(hub.tasks.value[0].status).toBe('pause');
    activeCase.value = { dir: 'case-b', evidence: [{ image_id: 'disk-a' }] }; await settle();
    carveProgress({ payload: carving('completed') });
    expect(hub.tasks.value).toEqual([]);
    ipc.carveLatest.mockResolvedValue(carving('completed'));
    activeCase.value = { dir: 'case-a', evidence: [{ image_id: 'disk-a' }] }; await settle();
    expect(hub.tasks.value[0]).toMatchObject({ id: 'carve:carve-a', status: 'success' });
  });
});


const snapshotEvent = { task_id: 'snapshot:disk-a:part:0', image_id: 'disk-a', volume_key: 'part:0', entries: 0, truncated: false, message: null };
const nativeSnapshot: import('../../../lib/ipc/taskRuntime').BackendTaskSnapshot = { id: snapshotEvent.task_id, kind: 'volumeSnapshot', imageId: 'disk-a', status: 'running', resourceClasses: [], elapsedMs: 1000 };
describe('filesystem and indexing tasks', () => {
  it('tracks both stages, routes cancellation to the shared backend task, and does not turn cancellation into success', async () => {
    const hub = useBuiltinTasks(); await settle();
    snapshotProgress({ payload: { ...snapshotEvent, phase: 'started' } });
    snapshotProgress({ payload: { ...snapshotEvent, phase: 'walking', entries: 345 } });
    expect(hub.tasks.value.map(task => [task.title, task.status])).toEqual([['文件系统加载', 'running'], ['索引建立', 'queued']]);
    snapshotProgress({ payload: { ...snapshotEvent, phase: 'indexing', entries: 40 } });
    expect(hub.tasks.value.map(task => [task.status, task.results])).toEqual([['success', 345], ['running', 40]]);
    await hub.cancel(`index:${snapshotEvent.task_id}`);
    expect(ipc.cancel).toHaveBeenLastCalledWith({ taskId: snapshotEvent.task_id });
    snapshotProgress({ payload: { ...snapshotEvent, phase: 'cancelled' } });
    snapshotProgress({ payload: { ...snapshotEvent, phase: 'done', entries: 500 } });
    expect(hub.tasks.value.map(task => task.status)).toEqual(['success', 'cancel']);
  });

  it('records completion without counting registry removal as success and recovers active work on attach', async () => {
    ipc.nativeList.mockResolvedValue([nativeSnapshot]);
    const hub = useBuiltinTasks(); await settle();
    expect(hub.tasks.value).toHaveLength(2);
    expect(hub.tasks.value[0]).toMatchObject({ subject: '分区 0', status: 'running', started: Date.now() - 1000 });
    nativeLifecycle({ payload: { phase: 'finished', task: nativeSnapshot } });
    expect(hub.tasks.value.every(task => task.status === 'offline')).toBe(true);
    snapshotProgress({ payload: { ...snapshotEvent, phase: 'done', entries: 123 } });
    expect(hub.tasks.value.every(task => task.status === 'success' && task.results === 123)).toBe(true);
  });

  it('keeps index errors visible, resets them only on a new build, and rejects late events from a closed case', async () => {
    const hub = useBuiltinTasks(); await settle();
    snapshotProgress({ payload: { ...snapshotEvent, phase: 'indexing' } });
    snapshotProgress({ payload: { ...snapshotEvent, phase: 'error', message: 'disk full' } });
    snapshotProgress({ payload: { ...snapshotEvent, phase: 'done' } });
    expect(hub.tasks.value[1]).toMatchObject({ status: 'failed', error: 'disk full' });
    nativeLifecycle({ payload: { phase: 'queued', task: { ...nativeSnapshot, status: 'queued' } } });
    snapshotProgress({ payload: { ...snapshotEvent, phase: 'started' } });
    expect(hub.tasks.value.map(task => task.status)).toEqual(['running', 'queued']);
    activeCase.value = { dir: 'case-b', evidence: [{ image_id: 'disk-b' }] }; await settle();
    snapshotProgress({ payload: { ...snapshotEvent, phase: 'done' } });
    expect(hub.tasks.value).toHaveLength(0);
  });

  it('tracks fast and caseless loads even before evidence metadata refreshes', async () => {
    activeCase.value = null;
    const hub = useBuiltinTasks(); await settle();
    snapshotProgress({ payload: { ...snapshotEvent, task_id: 'snapshot-session:disk-a:root', volume_key: 'root', phase: 'done', entries: 20 } });
    expect(hub.tasks.value.map(task => [task.subject, task.status])).toEqual([['根目录', 'success'], ['根目录', 'success']]);
  });
});


const report: AutoDetectReport = { image_id: 'disk-a', route: 'disk_image', platform: 'windows', platforms: ['windows'], evidence_type: 'disk', is_backup: false,
  manufactor: null, display_platform_name: 'Windows', friend_platform_name: 'Windows', partitions: [], backup: null, warnings: [] };
const detection: import('../../../lib/ipc/taskRuntime').BackendTaskSnapshot = { id: 'autodetect:disk-a', kind: 'autoDetect', imageId: 'disk-a', status: 'running', resourceClasses: [], elapsedMs: 0 };
const queuedDetection = { phase: 'queued' as const, task: { ...detection, status: 'queued' as const } };
function detectionError(code: 'cancelled' | 'taskPanicked', message: string): AutoDetectFailure {
  return { image_id: 'disk-a', error: { code, message, context: null, retryable: false } };
}
describe('platform detection tasks', () => {
  it('follows the first task from registration to its report without counting registry removal as success', async () => {
    const hub = useBuiltinTasks(); await settle();
    nativeLifecycle({ payload: queuedDetection });
    expect(hub.tasks.value[0]).toMatchObject({ id: 'autodetect:disk-a', title: '自动识别平台', trigger: 'auto', status: 'queued', cancelId: 'autodetect:disk-a' });
    nativeLifecycle({ payload: { phase: 'running', task: detection } });
    expect(hub.tasks.value[0]).toMatchObject({ status: 'running', progress: null });
    detectFinished({ payload: report });
    nativeLifecycle({ payload: { phase: 'finished', task: detection } });
    expect(hub.tasks.value[0]).toMatchObject({ status: 'success', progress: 100, message: 'Windows · 磁盘镜像', finished: Date.now() });
  });

  it('keeps one record for a manual run sharing the registration and reopens it only for a new one', async () => {
    const hub = useBuiltinTasks(); await settle();
    const manual = deferred<AutoDetectReport>();
    ipc.detect.mockReturnValue(manual.promise);
    const running = runAutoDetect('disk-a');
    nativeLifecycle({ payload: queuedDetection });
    expect(hub.tasks.value[0].trigger).toBe('manually');
    detectFinished({ payload: report });
    // A second caller joined the same registration before it was released.
    nativeLifecycle({ payload: { phase: 'running', task: { ...detection, elapsedMs: 50 } } });
    expect(hub.tasks.value.map(task => task.status)).toEqual(['success']);
    manual.resolve(report);
    await running;
    nativeLifecycle({ payload: queuedDetection });
    expect(hub.tasks.value).toHaveLength(1);
    expect(hub.tasks.value[0]).toMatchObject({ status: 'queued', trigger: 'auto', finished: null });
  });

  it('shows failures, waits for the real cancellation and lets a late outcome replace an offline guess', async () => {
    const hub = useBuiltinTasks(); await settle();
    nativeLifecycle({ payload: queuedDetection });
    detectFailed({ payload: detectionError('taskPanicked', 'autodetect_evidence task crashed') });
    expect(hub.tasks.value[0]).toMatchObject({ status: 'failed', error: 'autodetect_evidence task crashed' });

    nativeLifecycle({ payload: queuedDetection });
    await hub.cancel('autodetect:disk-a');
    expect(ipc.cancel).toHaveBeenCalledExactlyOnceWith({ taskId: 'autodetect:disk-a' });
    nativeLifecycle({ payload: { phase: 'cancellationRequested', task: { ...detection, status: 'cancellationRequested' } } });
    expect(hub.tasks.value[0]).toMatchObject({ status: 'queued', cancelling: true });
    detectFailed({ payload: detectionError('cancelled', '自动识别已取消') });
    expect(hub.tasks.value[0]).toMatchObject({ status: 'cancel', cancelling: false, error: null });

    nativeLifecycle({ payload: queuedDetection });
    nativeLifecycle({ payload: { phase: 'finished', task: detection } });
    expect(hub.tasks.value[0]).toMatchObject({ status: 'offline', error: '任务已结束，未收到识别结果，请检查日志' });
    detectFinished({ payload: report });
    expect(hub.tasks.value[0]).toMatchObject({ status: 'success', error: null });
  });

  it('caches a first-task report that arrives before the image-info page opens', async () => {
    useBuiltinTasks(); await settle();
    const [, onReport] = ipc.reportEvents.mock.calls.find(([name]) => name === 'autodetect-finished')!;
    onReport({ payload: report });
    expect(useAutoDetectReport(() => 'disk-a').value).toEqual(report);
  });

  it('recovers a run already in progress and ignores it once another case is open', async () => {
    ipc.nativeList.mockResolvedValue([{ ...detection, elapsedMs: 1500 }]);
    const hub = useBuiltinTasks(); await settle();
    expect(hub.tasks.value[0]).toMatchObject({ id: 'autodetect:disk-a', status: 'running', trigger: 'auto', started: Date.now() - 1500 });
    activeCase.value = { dir: 'case-b', evidence: [{ image_id: 'disk-a' }] }; await settle();
    detectFinished({ payload: report });
    expect(hub.tasks.value).toEqual([]);
  });
});

it('deduplicates cancellation across queued indexing and its running filesystem stage', async () => {
  const hub = useBuiltinTasks(); await settle();
  snapshotProgress({ payload: { ...snapshotEvent, phase: 'started' } });
  const cancelled = hub.cancel(`filesystem:${snapshotEvent.task_id}`);
  await hub.cancel(`index:${snapshotEvent.task_id}`);
  await cancelled;
  expect(ipc.cancel).toHaveBeenCalledExactlyOnceWith({ taskId: snapshotEvent.task_id });
  expect(hub.tasks.value.every(task => task.cancelling)).toBe(true);
  snapshotProgress({ payload: { ...snapshotEvent, phase: 'cancelled' } });
  expect(hub.tasks.value.every(task => task.status === 'cancel' && !task.cancelling)).toBe(true);
});


function exportJob(taskId = 'export-1') {
  return reactive({ imageId: 'disk-a', taskId, destination: 'D:/exports/disk.dd',
    bytesDone: 0, bytesTotal: 0, startedAtMs: Date.now(), cancelling: false });
}

describe('image export task ownership', () => {
  it('tracks shared progress after page disposal and retains read-fault provenance', async () => {
    const operation = deferred<import('../../../lib/ipc/image').ImageExportResult>();
    const job = exportJob();
    const page = effectScope();
    const hub = page.run(() => useBuiltinTasks())!;
    const running = trackedImageExport(job, () => operation.promise, vi.fn());
    await settle();
    page.stop();
    job.bytesTotal = 4096;
    job.bytesDone = 1024;
    expect(hub.tasks.value[0]).toMatchObject({ id: 'imageExport:export-1', kind: 'imageExport',
      title: '镜像导出', subject: job.destination, progress: 25, status: 'running', cancelId: 'export-1' });
    operation.resolve({ bytes_written: 4096, bad_regions: [{ offset: 0, length: 512 }],
      synthetic_bytes: 512, read_faults_detected: true });
    await running;
    expect(hub.tasks.value[0]).toMatchObject({ status: 'success', progress: 100, cancelling: false });
    expect(hub.tasks.value[0].message).toContain('512 B');
    expect(hub.tasks.value[0].message).toContain('1 个坏区');
    job.bytesDone = 2048;
    await hub.refresh();
    expect(hub.tasks.value[0].progress).toBe(100);
  });

  it('uses the initiating workflow cancellation path and waits for backend confirmation', async () => {
    const operation = deferred<number>();
    const job = exportJob();
    const hub = useBuiltinTasks();
    const cancelExport = vi.fn(async () => {
      job.cancelling = true;
      await ipc.cancel({ taskId: job.taskId });
    });
    const running = trackedImageExport(job, () => operation.promise, cancelExport);
    await settle();
    await hub.cancel('imageExport:export-1');
    await hub.cancel('imageExport:export-1');
    expect(cancelExport).toHaveBeenCalledOnce();
    expect(ipc.cancel).toHaveBeenCalledExactlyOnceWith({ taskId: 'export-1' });
    expect(hub.tasks.value[0]).toMatchObject({ status: 'running', cancelling: true });
    operation.reject(new Error('export cancelled'));
    await expect(running).rejects.toThrow('export cancelled');
    expect(hub.tasks.value[0]).toMatchObject({ status: 'cancel', cancelling: false, error: null });
  });

  it('retains a failure and allows another export of the same evidence', async () => {
    const hub = useBuiltinTasks();
    await expect(trackedImageExport(exportJob(), async () => { throw new Error('permission denied'); }, vi.fn()))
      .rejects.toThrow('permission denied');
    await trackedImageExport(exportJob('export-2'), async () => 4096, vi.fn());
    expect(hub.tasks.value.map(task => [task.id, task.status, task.error])).toEqual([
      ['imageExport:export-1', 'failed', 'Error: permission denied'],
      ['imageExport:export-2', 'success', null],
    ]);
  });

  it('does not publish late export progress or completion after an A to B to A case switch', async () => {
    const operation = deferred<number>();
    const job = exportJob();
    const hub = useBuiltinTasks();
    const running = trackedImageExport(job, () => operation.promise, vi.fn());
    await settle();
    activeCase.value = { dir: 'case-b', evidence: [{ image_id: 'disk-a' }] };
    activeCase.value = { dir: 'case-a', evidence: [{ image_id: 'disk-a' }] };
    job.bytesDone = 1024;
    operation.resolve(4096);
    await running;
    expect(hub.tasks.value).toEqual([]);
  });
});
