
import { tr } from '../../../composables/useLocale';
import { joinList } from '../../../i18n/translate';

import { type BuiltinTask, type ImageExportTaskState } from '../model/types';
import { computed, effectScope, ref, shallowRef, watch } from 'vue';
import { fsChildren, fsChildrenPage, fsDockerPrepare, fsFlatPage } from '../../../lib/ipc/fs/query';
import { taskCancel, taskList, onTaskLifecycle, onTaskProgress } from '../../../lib/ipc/taskRuntime';
import { globalSearchHistoryList, globalSearchStart, onGlobalSearchProgress } from '../../../lib/ipc/search';
import { onTimelineProgress, timelineStart, timelineStatus } from '../../../lib/ipc/timeline';
import { carveLatestScan, carveScanStatus, carveStart, carveScanPause, carveScanResume, onCarveScanProgress, type CarveScanSnapshot } from '../../../lib/ipc/carve';
import { onSnapshotProgress } from '../../../lib/ipc/cases';
import { onAutoDetectFailed, onAutoDetectFinished } from '../../../lib/ipc/autodetect';
import { encryptedVolumeUnlock } from '../../../lib/ipc/encrypted';
import type { ImageExportResult } from '../../../lib/ipc/image';
import { formatBytes } from '../../../utils/format';
import { IpcCommandError } from '../../../lib/ipc/client';
import { applySnapshotTasks, recoverSnapshotTasks, isSnapshotTask } from '../model/filesystemTasks';
import { applyAutoDetectFailure, applyAutoDetectLifecycle, applyAutoDetectSuccess, autoDetectTaskId, isAutoDetectTask, recoverAutoDetectTask } from '../model/autodetectTasks';
import { taskCaseContext, ensureTaskDetection, taskDetectionPending } from './taskContext';

import { makeBuiltinTask, taskPercent as percent, taskStatus as status, projectCarveTask, activeCarveStatuses } from '../model/builtinTaskView';
import { createBuiltinTaskEvents } from './useBuiltinTaskEvents';

const records = shallowRef(new Map<string, BuiltinTask>());
const now = ref(Date.now());
const owned = new Map<string, symbol>();
const exportCancels = new Map<string, () => Promise<void>>();
const dockerNames = new Map<string, string>();
const dockerPreparations = new Map<string, Promise<void>>();
const persistedSearches = new Set<string>();
const carveCases = new Map<string, string | null>();
const snapshotCases = new Map<string, string | null>();
/** Case of each evidence's latest detection run, keyed by image id. */
const autodetectCases = new Map<string, string | null>();
const localTimelineStarts = new Map<string, number>();
let scope: ReturnType<typeof effectScope> | null = null;
let subscriptions: Promise<void> | null = null;
let generation = 0;
let caseRevision = 0;
let refreshing = false;
let timer: ReturnType<typeof setInterval> | null = null;
let eventBridge: ReturnType<typeof createBuiltinTaskEvents> | null = null;

function put(task: BuiltinTask) {
  records.value = new Map(records.value).set(task.id, task);
}
function patch(id: string, update: Partial<BuiltinTask>) {
  const task = records.value.get(id);
  if (task) put({ ...task, ...update });
}
function remove(id: string) {
  const next = new Map(records.value);
  next.delete(id);
  records.value = next;
}
function makeTask(kind: BuiltinTask['kind'], id: string, imageId: string, subject = '') {
  return makeBuiltinTask(kind, id, imageId, subject, Date.now(), tr);
}
function accepts(imageId: string) {
  return taskCaseContext()?.evidence?.some(item => item.image_id === imageId)
    || [...records.value.values()].some(task => task.imageId === imageId);
}
/** A detection registered while the image-info page awaits its own run is that manual run. */
function autoDetectTrigger(imageId: string): BuiltinTask['trigger'] {
  return taskDetectionPending(imageId) ? 'manually' : 'auto';
}
/** Detection outcomes only close runs that started under the current case. */
function autoDetectInCase(imageId: string) {
  return autodetectCases.get(imageId) === (taskCaseContext()?.dir ?? null);
}

function applyCarveSnapshot(snapshot: CarveScanSnapshot) {
  const caseDir = taskCaseContext()?.dir ?? null;
  if (carveCases.has(snapshot.scan_id) && carveCases.get(snapshot.scan_id) !== caseDir) return;
  carveCases.set(snapshot.scan_id, caseDir);
  const task = projectCarveTask(snapshot, records.value.get(`carve:${snapshot.scan_id}`), Date.now(), tr);
  if (task) put(task);
}

/** The command only launches a scan; completion is driven by snapshots, not its return. */
export async function trackedCarveStart(args: Parameters<typeof carveStart>[0]) {
  const ready = ensureTracking();
  const epoch = caseRevision;
  const caseDir = taskCaseContext()?.dir ?? null;
  await ready;
  if (epoch !== caseRevision) throw new Error(tr("案件已切换，任务未启动"));
  const result = await carveStart(args);
  carveCases.set(result.scan_id, caseDir);
  if (epoch !== caseRevision) {
    throw new Error(tr("案件已切换，请回到发起扫描的检材查看扫描任务"));
  }
  const id = `carve:${result.scan_id}`;
  if (!records.value.has(id)) put({ ...makeTask('carve', id, args.imageId), status: 'queued', cancelId: result.task_id, message: tr("等待扫描") });
  const previous = records.value.get(id);
  try {
    const snapshot = await carveScanStatus({ scanId: result.scan_id });
    if (epoch === caseRevision && records.value.get(id) === previous) applyCarveSnapshot(snapshot);
  } catch (error) { console.warn('读取雕刻任务状态失败，将继续同步', error); }
  return result;
}

async function controlCarve(id: string, action: 'pause' | 'start') {
  const task = records.value.get(id);
  if (!task || task.kind !== 'carve' || task.cancelling) return;
  if (task.status !== (action === 'pause' ? 'running' : 'pause')) return;
  const epoch = caseRevision;
  const snapshot = await (action === 'pause' ? carveScanPause : carveScanResume)({ scanId: id.slice('carve:'.length) });
  if (epoch === caseRevision && records.value.get(id) === task) applyCarveSnapshot(snapshot);
}

/** Recover saved tasks and current runtime carving sessions. */
async function refresh() {
  const activeCase = taskCaseContext();
  if (refreshing) return;
  const epoch = caseRevision;
  refreshing = true;
  try {
    const previous = records.value;
    try {
      const nativeTasks = await taskList();
      if (epoch !== caseRevision) return;
      for (const task of nativeTasks) {
        if (!task.imageId) continue;
        const caseDir = activeCase?.dir ?? null;
        if (isAutoDetectTask(task)) {
          const id = autoDetectTaskId(task.imageId);
          if (records.value.get(id) !== previous.get(id)) continue;
          if (autodetectCases.has(task.imageId) && autodetectCases.get(task.imageId) !== caseDir) continue;
          autodetectCases.set(task.imageId, caseDir);
          records.value = recoverAutoDetectTask(records.value, task, autoDetectTrigger(task.imageId), Date.now(), undefined, tr);
          continue;
        }
        if (!isSnapshotTask(task)) continue;
        const id = `filesystem:${task.id}`;
        if (records.value.get(id) !== previous.get(id)) continue;
        if (snapshotCases.has(task.id) && snapshotCases.get(task.id) !== caseDir) continue;
        snapshotCases.set(task.id, caseDir);
        records.value = recoverSnapshotTasks(records.value, task, Date.now(), undefined, tr);
      }
    } catch (error) { console.warn('读取文件系统任务状态失败', error); }
    for (const evidence of activeCase?.evidence ?? []) {
      if (epoch !== caseRevision) return;
      const snapshot = records.value;
      const unchanged = (id: string) => !owned.has(id) && records.value.get(id) === snapshot.get(id);
      const activeScans = [...snapshot.values()].filter(task => task.kind === 'carve' && task.imageId === evidence.image_id && activeCarveStatuses.includes(task.status));
      const [searches, timeline, latestCarve, ...carveStates] = await Promise.allSettled([
        globalSearchHistoryList({ imageId: evidence.image_id, limit: 100 }),
        timelineStatus({ imageId: evidence.image_id }),
        carveLatestScan({ imageId: evidence.image_id }),
        ...activeScans.map(task => carveScanStatus({ scanId: task.id.slice('carve:'.length) })),
      ]);
      if (epoch !== caseRevision) return;
      if (latestCarve.status === 'fulfilled' && latestCarve.value && unchanged(`carve:${latestCarve.value.scan_id}`)) applyCarveSnapshot(latestCarve.value);
      else if (latestCarve.status === 'rejected') console.warn('读取雕刻会话失败', latestCarve.reason);
      for (const state of carveStates) {
        if (state.status === 'fulfilled' && unchanged(`carve:${state.value.scan_id}`)) applyCarveSnapshot(state.value);
        else if (state.status === 'rejected') console.warn('读取雕刻任务状态失败', state.reason);
      }
      if (searches.status === 'fulfilled') {
        const present = new Set(searches.value.map(row => `search:${row.search_id}`));
        for (const task of records.value.values()) {
          if (task.kind === 'search' && task.imageId === evidence.image_id && persistedSearches.has(task.id) && unchanged(task.id) && !present.has(task.id)) remove(task.id);
        }
        for (const row of searches.value) {
          const id = `search:${row.search_id}`;
          persistedSearches.add(id);
          if (!unchanged(id)) continue;
          put({
            ...makeTask('search', id, row.image_id, joinList([...row.keywords, ...row.regexes], tr)),
            status: status(row.status), error: row.error, started: row.started_ms, finished: row.finished_ms,
            cancelling: row.status === 'running' && !!snapshot.get(id)?.cancelling,
            progress: row.status === 'success' ? 100 : percent(row.files_scanned, row.files_total),
            message: tr("已扫描 {0} 个文件 · {1} 处命中", { 0: row.files_scanned.toLocaleString(), 1: row.total_matches.toLocaleString() }), results: row.total_matches
          });
        }
      } else console.warn('读取搜索任务历史失败', searches.reason);
      const id = `timeline:${evidence.image_id}`;
      if (timeline.status === 'fulfilled' && unchanged(id)) {
        const row = timeline.value.meta;
        const localStart = localTimelineStarts.get(id);
        if (localStart !== undefined && (!row || row.started_ms < localStart)) continue;
        if (row) put({
          ...makeTask('timeline', id, evidence.image_id), status: status(row.status),
          error: row.error, started: row.started_ms, finished: row.finished_ms,
          cancelling: row.status === 'running' && !!snapshot.get(id)?.cancelling,
          progress: row.status === 'success' ? 100 : records.value.get(id)?.progress ?? null,
          timelineRows: row.file_rows + row.event_rows,
          message: tr("文件 {0} 条 · 事件 {1} 条", { 0: row.file_rows.toLocaleString(), 1: row.event_rows.toLocaleString() })
        });
        else remove(id);
      } else if (timeline.status === 'rejected') console.warn('读取时间线任务状态失败', timeline.reason);
    }
  } finally {
    if (epoch === caseRevision) refreshing = false;
  }
}

function ensureTracking(): Promise<void> {
  if (subscriptions) return eventBridge?.ensure() ?? subscriptions;
  const epoch = generation;
  // The image-info page reads reports from useAutoDetect; without its listener
  // a first task finishing before that page opens would be run a second time.
  ensureTaskDetection();
  eventBridge = createBuiltinTaskEvents([
    () => onSnapshotProgress(({ payload }) => {
      if (epoch !== generation || !payload.task_id) return;
      const caseDir = taskCaseContext()?.dir ?? null;
      if (snapshotCases.has(payload.task_id) && snapshotCases.get(payload.task_id) !== caseDir) return;
      snapshotCases.set(payload.task_id, caseDir);
      records.value = applySnapshotTasks(records.value, payload, Date.now(), tr);
    }),
    () => onTaskLifecycle(({ payload }) => {
      const task = payload.task;
      if (epoch === generation && task.kind === 'imageLoad') {
        const id = `imageLoad:${task.id}`;
        const current = records.value.get(id);
        if (!current || !owned.has(id) || payload.phase === 'finished') return;
        patch(id, { status: payload.phase === 'queued' ? 'queued' : 'running',
          cancelling: current.cancelling || payload.phase === 'cancellationRequested' });
        if (payload.phase === 'queued' && current.cancelling) {
          void taskCancel({ taskId: task.id }).catch(error => console.warn('取消证据加载失败', error));
        }
        return;
      }
      if (epoch !== generation || !task.imageId) return;
      if (task.kind === 'encryptedVolumeUnlock') {
        const id = `encryptedUnlock:${task.id}`;
        const current = records.value.get(id);
        if (!current || !owned.has(id) || payload.phase === 'finished') return;
        patch(id, {
          status: payload.phase === 'queued' ? 'queued' : 'running',
          cancelling: current.cancelling || payload.phase === 'cancellationRequested',
        });
        // A cancel click may precede registration while the command loads its source.
        if (payload.phase === 'queued' && current.cancelling) {
          void taskCancel({ taskId: task.id }).catch(error => {
            if (owned.has(id)) patch(id, { cancelling: false, error: String(error) });
            console.warn('取消加密卷解锁失败', error);
          });
        }
        return;
      }
      const caseDir = taskCaseContext()?.dir ?? null;
      if (isAutoDetectTask(task)) {
        // The first task registers while `image_load` still runs, before the
        // case DTO lists the evidence, so this cannot filter by `accepts`.
        if (payload.phase === 'queued') autodetectCases.set(task.imageId, caseDir);
        else if (autodetectCases.has(task.imageId) && autodetectCases.get(task.imageId) !== caseDir) return;
        records.value = applyAutoDetectLifecycle(records.value, payload, autoDetectTrigger(task.imageId), Date.now(), tr);
        return;
      }
      if (!isSnapshotTask(task)) return;
      if (payload.phase === 'queued') snapshotCases.set(task.id, caseDir);
      else if (snapshotCases.has(task.id) && snapshotCases.get(task.id) !== caseDir) return;
      if (payload.phase !== 'finished') {
        records.value = recoverSnapshotTasks(records.value, task, Date.now(), payload.phase === 'queued', tr);
      } else {
        // Registry removal is not proof of success. Domain completion carries the outcome.
        for (const kind of ['filesystem', 'index']) {
          const id = `${kind}:${task.id}`;
          const current = records.value.get(id);
          if (current && ['queued', 'running'].includes(current.status)) patch(id, {
            status: task.status === 'cancellationRequested' ? 'cancel' : 'offline', finished: Date.now(), cancelling: false,
            error: task.status === 'cancellationRequested' ? null : tr("任务已结束，未收到完成结果，请检查日志"),
          });
        }
      }
    }),
    () => onTaskProgress(({ payload }) => {
      const id = `imageLoad:${payload.task.id}`;
      if (epoch !== generation || payload.task.kind !== 'imageLoad' || !owned.has(id)) return;
      patch(id, { progress: payload.total ? percent(payload.done, payload.total) : null,
        message: `${payload.message ?? ''}${payload.total ? ` · ${formatBytes(payload.done)} / ${formatBytes(payload.total)}` : ''}`, status: 'running' });
    }),
    () => onAutoDetectFinished(({ payload }) => {
      if (epoch !== generation || !autoDetectInCase(payload.image_id)) return;
      records.value = applyAutoDetectSuccess(records.value, payload, Date.now(), tr);
    }),
    () => onAutoDetectFailed(({ payload }) => {
      if (epoch !== generation || !autoDetectInCase(payload.image_id)) return;
      records.value = applyAutoDetectFailure(records.value, payload, Date.now(), tr);
    }),
    () => onCarveScanProgress(({ payload }) => {
      if (epoch !== generation || !accepts(payload.image_id)) return;
      applyCarveSnapshot(payload);
    }),
    () => onGlobalSearchProgress(({ payload }) => {
      if (epoch !== generation || !accepts(payload.image_id)) return;
      const id = `search:${payload.search_id}`;
      if (!records.value.has(id)) put(makeTask('search', id, payload.image_id));
      if (records.value.get(id)?.status !== 'running') return;
      patch(id, {
        progress: percent(payload.files_done, payload.files_total), results: payload.hits,
        message: tr("已扫描 {0} / {1} 个文件 · {2} 处命中", { 0: payload.files_done.toLocaleString(), 1: payload.files_total.toLocaleString(), 2: payload.hits.toLocaleString() })
      });
    }),
    () => onTimelineProgress(({ payload }) => {
      if (epoch !== generation || !accepts(payload.image_id)) return;
      const id = `timeline:${payload.image_id}`;
      if (!records.value.has(id)) put(makeTask('timeline', id, payload.image_id));
      if (records.value.get(id)?.status !== 'running') return;
      patch(id, {
        progress: percent(payload.done, payload.total), timelineRows: payload.rows,
        message: tr("已处理 {0}{1} 项 · {2} 条时间线", { 0: payload.done.toLocaleString(), 1: payload.total > 0 ? ` / ${payload.total.toLocaleString()}` : '', 2: payload.rows.toLocaleString() })
      });
    }),
  ]);
  subscriptions = eventBridge.ensure();
  scope = effectScope(true);
  scope.run(() => {
    watch(() => taskCaseContext()?.dir ?? null, () => {
      // Subscription callbacks use the current case filter. Command completions
      // carry this generation so a closed case cannot repopulate the new one.
      caseRevision++;
      refreshing = false;
      records.value = new Map();
      owned.clear();
      exportCancels.clear();
      dockerNames.clear();
      dockerPreparations.clear();
      persistedSearches.clear();
      localTimelineStarts.clear();
      void refresh();
    }, { flush: 'sync', immediate: true });
  });
  timer = setInterval(() => {
    now.value = Date.now();
    if ([...records.value.values()].some(task => (task.status === 'running' || task.kind === 'carve' && activeCarveStatuses.includes(task.status)) && !owned.has(task.id))) void refresh();
  }, 2000);
  return subscriptions;
}

/** Completion belongs to the operation, not the page that happened to start it. */
async function run<T>(task: BuiltinTask, operation: () => Promise<T>, summarize: (result: T) => Partial<BuiltinTask>): Promise<T> {
  const ready = ensureTracking();
  const caseDir = taskCaseContext()?.dir;
  const epoch = caseRevision;
  if (owned.has(task.id)) throw new Error(tr("任务正在执行，请等待完成"));
  const owner = Symbol(task.id);
  put(task);
  if (task.kind === 'timeline') localTimelineStarts.set(task.id, task.started);
  owned.set(task.id, owner);
  const current = () => epoch === caseRevision && taskCaseContext()?.dir === caseDir && owned.get(task.id) === owner;
  try {
    await ready;
    if (!current()) throw new Error(tr("案件已切换，任务未启动"));
    const result = await operation();
    if (current()) patch(task.id, { status: 'success', progress: 100, finished: Date.now(), cancelling: false, ...summarize(result) });
    return result;
  } catch (error) {
    const cancelled = (task.kind === 'encryptedUnlock' || task.kind === 'imageExport' || task.kind === 'imageLoad')
      && (error instanceof IpcCommandError && error.code === 'cancelled'
        || (task.kind === 'imageExport' || task.kind === 'imageLoad') && (records.value.get(task.id)?.cancelling || /cancel/i.test(String(error))));
    if (current()) patch(task.id, { status: cancelled ? 'cancel' : 'failed', error: cancelled ? null : String(error), message: cancelled ? (task.kind === 'imageLoad' ? tr("加载已取消") : task.kind === 'imageExport' ? tr("导出已取消（保留部分文件）") : tr("已取消解锁")) : task.message, finished: Date.now(), cancelling: false });
    throw error;
  } finally {
    if (current()) owned.delete(task.id);
  }
}

/** Container opening owns its outcome even while another evidence tab is active. */
export function trackedImageLoad<T extends { id: string }>(taskId: string, path: string, operation: () => Promise<T>, onReady: () => Promise<void>) {
  const task = makeTask('imageLoad', `imageLoad:${taskId}`, '', path);
  task.cancelId = taskId;
  task.status = 'queued';
  return run(task, async () => {
    const result = await operation();
    await onReady();
    return result;
  }, result => ({ imageId: result.id, message: tr("检材已就绪") }));
}

/** Share live export state and its cancellation path with the task center. */
export async function trackedImageExport<T extends number | ImageExportResult>(
  job: ImageExportTaskState,
  operation: () => Promise<T>,
  cancelExport: () => Promise<void>,
): Promise<T> {
  const task = makeTask('imageExport', `imageExport:${job.taskId}`, job.imageId, job.destination);
  task.started = job.startedAtMs;
  task.cancelId = job.taskId;
  const completion = run(task, operation, outcome => {
    const written = typeof outcome === 'number' ? outcome : outcome.bytes_written;
    const warning = typeof outcome !== 'number' && outcome.read_faults_detected
      ? tr(" · 警告：含 {0} 补零字节（{1} 个坏区）", { 0: formatBytes(outcome.synthetic_bytes), 1: outcome.bad_regions.length })
      : '';
    return { message: tr("已导出 {0}{1}", { 0: formatBytes(written), 1: warning }) };
  });
  const owner = owned.get(task.id);
  exportCancels.set(task.id, cancelExport);
  const progress = effectScope(true);
  progress.run(() => watch(
    () => [job.bytesDone, job.bytesTotal, job.cancelling],
    () => {
      if (owned.get(task.id) !== owner) return;
      patch(task.id, {
        progress: percent(job.bytesDone, job.bytesTotal), cancelling: job.cancelling,
        message: `${formatBytes(job.bytesDone)} / ${formatBytes(job.bytesTotal)}`,
      });
    },
    { flush: 'sync', immediate: true },
  ));
  try { return await completion; }
  finally {
    progress.stop();
    if (exportCancels.get(task.id) === cancelExport) exportCancels.delete(task.id);
  }
}

export function trackedEncryptedVolumeUnlock(args: Parameters<typeof encryptedVolumeUnlock>[0] & { taskId: string }, subject: string) {
  const task = makeTask('encryptedUnlock', `encryptedUnlock:${args.taskId}`, args.imageId, subject);
  task.cancelId = args.taskId;
  task.message = tr("正在派生密钥并解锁；取消将在当前派生阶段结束后生效");
  return run(task, () => {
    // IPC may defer transport; clearing local credentials must not mutate its payload.
    try { return encryptedVolumeUnlock({ ...args, options: { ...args.options } }); }
    finally { args.options.password = ''; }
  }, result => ({ message: tr("已解锁 · {0}", { 0: result.cipher }) }));
}

export function trackedGlobalSearchStart(args: Parameters<typeof globalSearchStart>[0]) {
  return run(makeTask('search', `search:${args.searchId}`, args.imageId, joinList([...args.options.keys, ...args.options.regexes], tr)),
    () => globalSearchStart(args), result => ({
      status: status(result.status), progress: result.status === 'success' ? 100 : percent(result.files_scanned, result.files_total),
      results: result.total_matches, message: tr("{0} 个命中文件 · {1} 处命中", { 0: result.matched_file_count.toLocaleString(), 1: result.total_matches.toLocaleString() })
    }));
}
export function trackedTimelineStart(args: Parameters<typeof timelineStart>[0]) {
  const task = makeTask('timeline', `timeline:${args.imageId}`, args.imageId);
  return run(task, () => timelineStart(args), result => ({
    status: status(result.status),
    progress: result.status === 'success' ? 100 : records.value.get(task.id)?.progress ?? null,
    timelineRows: result.file_rows + result.event_rows, message: tr("文件 {0} 条 · 事件 {1} 条", { 0: result.file_rows.toLocaleString(), 1: result.event_rows.toLocaleString() })
  }));
}

async function trackDockerRead<T>(args: { imageId: string; parentId: string | null }, operation: () => Promise<T>): Promise<T> {
  const root = args.parentId?.match(/^host-docker:[^:]+:/)?.[0];
  if (!root) return operation();
  void ensureTracking();
  const epoch = caseRevision;
  const caseDir = taskCaseContext()?.dir ?? null;
  const id = `docker:${args.imageId}:${root}`;
  let preparation = dockerPreparations.get(id);
  if (!preparation) {
    preparation = (async () => {
      const request = { imageId: args.imageId, rootId: root, caseDir };
      const cached = await fsDockerPrepare({ ...request, mode: 'status' });
      if (epoch !== caseRevision) throw new Error(tr("案件已切换，请重新打开容器"));
      if (cached.ready) return;
      const task = makeTask('docker', id, args.imageId, dockerNames.get(`${args.imageId}:${root}`) || root);
      task.message = caseDir ? tr("正在合并容器并保存索引…") : tr("正在合并容器文件系统…");
      await run(task, async () => {
        const result = await fsDockerPrepare({ ...request, mode: 'build' });
        if (!result.ready) throw new Error(tr("容器索引尚未完成，请重试"));
        return result;
      }, result => ({
        results: result.entryCount, message: result.persisted
          ? tr("索引已保存 · {0} 项，下次直接打开", { 0: result.entryCount.toLocaleString() })
          : tr("容器已就绪 · {0} 项（当前会话）", { 0: result.entryCount.toLocaleString() })
      }));
    })();
    dockerPreparations.set(id, preparation);
  }
  try {
    await preparation;
    if (epoch !== caseRevision) throw new Error(tr("案件已切换，请重新打开容器"));
    return await operation();
  } finally {
    if (dockerPreparations.get(id) === preparation) dockerPreparations.delete(id);
  }
}
export async function trackedFsChildrenPage(args: Parameters<typeof fsChildrenPage>[0]) {
  void ensureTracking();
  const epoch = caseRevision;
  const page = await trackDockerRead(args, () => fsChildrenPage(args));
  if (epoch === caseRevision) {
    for (const node of page.items) if (/^host-docker:[^:]+:$/.test(node.id)) dockerNames.set(`${args.imageId}:${node.id}`, node.name);
  }
  return page;
}
export async function trackedFsChildren(args: Parameters<typeof fsChildren>[0]) {
  void ensureTracking();
  const epoch = caseRevision;
  const children = await trackDockerRead(args, () => fsChildren(args));
  if (epoch === caseRevision) {
    for (const node of children) if (/^host-docker:[^:]+:$/.test(node.id)) dockerNames.set(`${args.imageId}:${node.id}`, node.name);
  }
  return children;
}
export function trackedFsFlatPage(args: Parameters<typeof fsFlatPage>[0]) {
  return trackDockerRead(args, () => fsFlatPage(args));
}

async function cancel(id: string) {
  const task = records.value.get(id);
  if (!task?.cancelId || !(task.kind === 'carve' ? activeCarveStatuses.includes(task.status) : ['queued', 'running'].includes(task.status)) || task.cancelling) return;
  const cancelExport = exportCancels.get(id);
  if (cancelExport) return cancelExport();
  const epoch = caseRevision;
  const related = [...records.value.values()].filter(item => item.cancelId === task.cancelId && ['queued', 'running', 'pause'].includes(item.status));
  if (related.some(item => item.cancelling)) return;
  for (const item of related) patch(item.id, { cancelling: true });
  try {
    // A click can arrive while native progress listeners are still attaching.
    // Start the command before sending its cancellation request.
    await ensureTracking();
    if (epoch !== caseRevision || (task.kind !== 'carve' && records.value.get(id)?.started !== task.started) || !activeCarveStatuses.includes(records.value.get(id)?.status ?? '')) return;
    await taskCancel({ taskId: task.cancelId });
  }
  catch (error) {
    if (epoch === caseRevision) for (const item of related) {
      if (records.value.get(item.id)?.started === item.started) patch(item.id, { cancelling: false });
    }
    throw error;
  }
}

export function useBuiltinTasks() {
  void ensureTracking();
  return { tasks: computed(() => [...records.value.values()]), now, refresh, cancel, pause: (id: string) => controlCarve(id, 'pause'), start: (id: string) => controlCarve(id, 'start') };
}

export function disposeBuiltinTasks() {
  generation++;
  caseRevision++;
  scope?.stop(); scope = null;
  eventBridge?.dispose(); eventBridge = null;
  if (timer) clearInterval(timer);
  timer = null; subscriptions = null; refreshing = false;
  records.value = new Map(); owned.clear(); exportCancels.clear(); dockerNames.clear(); dockerPreparations.clear(); persistedSearches.clear(); localTimelineStarts.clear(); carveCases.clear(); snapshotCases.clear(); autodetectCases.clear();
}

export function resetBuiltinTasksForTests() { disposeBuiltinTasks(); }
