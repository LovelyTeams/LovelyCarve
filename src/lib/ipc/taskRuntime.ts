// 任务中心与后台任务 IPC。`task_list` 列出插件任务与内置任务(加载、哈希、导出、
// 解锁、平台识别、快照与索引、全局搜索、时间线、文件恢复、Docker 合并),变更走
// 统一的 `task-entry` 事件;`task_cancel` 按任务 id 取消(插件任务为
// `plugin:<id>`)。Interfaces mirror `app::workflows::task_center` and
// `task_manager` (camelCase on the wire).
import { onEvent, type UnlistenFn } from './events';
import { invokeCmd } from './client';
import type { SchedNotice, SchedTaskRecord, SchedTaskStatus } from './tasks';

export type BackendTaskKind =
  | 'autoDetect'
  | 'bitlockerExport'
  | 'bitlockerScan'
  | 'bitlockerUnlock'
  | 'encryptedVolumeUnlock'
  | 'filevaultInspect'
  | 'carveRecovery'
  | 'carveScan'
  | 'directoryExport'
  | 'dockerPrepare'
  | 'flatList'
  | 'imageExport'
  | 'imageHash'
  | 'imageLoad'
  | 'iosBackupAudit'
  | 'iosBackupUnlock'
  | 'globalSearch'
  | 'mediaMaterialization'
  | 'ocrScan'
  | 'qrScan'
  | 'sessionSnapshot'
  | 'shadowCopyOpen'
  | 'timelineBuild'
  | 'volumeSnapshot';

export type BackendTaskStatus = 'queued' | 'running' | 'cancellationRequested';

export type BackendTaskProgressUnit = 'attempts' | 'bytes' | 'entries' | 'percent';

export type BackendTaskResourceClass =
  | 'globalBackground'
  | 'imageSequentialIo'
  | 'imageRandomIo'
  | 'cpu'
  | 'gpu';

/** Rust `TaskCount`:任务报告的计数。 */
export type TaskCount =
  | 'hits'
  | 'matchedFiles'
  | 'rows'
  | 'fileRows'
  | 'eventRows'
  | 'entries'
  | 'found'
  | 'files'
  | 'syntheticBytes'
  | 'badRegions';

/** Rust `TaskLabel`:原样展示的结果值(平台 id、厂商名、检材类型、加密方案、算法、压缩包成员)。 */
export type TaskLabel = 'platform' | 'platformName' | 'evidenceType' | 'scheme' | 'cipher' | 'item';

export type TaskFlag = 'truncated' | 'persisted' | 'reused' | 'backup';

export type TaskStage =
  | 'opening'
  | 'decompressing'
  | 'detectingPartitions'
  | 'detectingFilesystems'
  | 'filesystem'
  | 'index'
  | 'signature';

export interface TaskAmount {
  unit: BackendTaskProgressUnit;
  done: number;
  /** 未知时为 null。 */
  total: number | null;
}

/** 内置任务的任务中心视图(Rust `TaskView`)。 */
export interface TaskView {
  /** 任务对象的原始片段:路径、关键词、卷键、容器名,由界面格式化。 */
  subject: string[];
  stage: TaskStage | null;
  progress: TaskAmount | null;
  counts: Partial<Record<TaskCount, number>>;
  labels: Partial<Record<TaskLabel, string>>;
  flags: TaskFlag[];
}

export type TaskOutcome =
  | { state: 'succeeded' }
  | { state: 'failed'; error: string }
  | { state: 'cancelled' }
  | { state: 'interrupted' };

export interface BackendTaskSnapshot {
  id: string;
  kind: BackendTaskKind;
  imageId: string | null;
  status: BackendTaskStatus;
  resourceClasses: BackendTaskResourceClass[];
  elapsedMs: number;
  automatic: boolean;
  createdAtMs: number;
  finishedAtMs: number | null;
  view: TaskView;
  outcome: TaskOutcome | null;
}

export interface BackendTaskProgressEvent {
  task: BackendTaskSnapshot;
  unit: BackendTaskProgressUnit;
  done: number;
  total: number | null;
  message: string | null;
}

/** 任务中心对任务提供的操作:暂停 / 继续只用于文件恢复扫描,开始 / 重新分析 / 删除只用于插件任务。 */
export type TaskAction = 'cancel' | 'pause' | 'resume' | 'start' | 'restart' | 'remove';

interface TaskEntryBase {
  /** 内置任务为任务 id;插件任务为 `plugin:<任务 id>`。 */
  id: string;
  /** 尚未属于任何检材的任务(打开检材)为 null。 */
  imageId: string | null;
  /** 调度器的状态词:`pause` 为暂停的文件恢复扫描(或旧版本暂停的插件任务),`offline` 为没有报告结局就消失的任务。 */
  status: SchedTaskStatus;
  /** 已请求取消,任务正在停止。 */
  cancelling: boolean;
  /** 由应用发起(检材加入案件)而非用户。 */
  automatic: boolean;
  /** Unix 毫秒。 */
  createdAtMs: number;
  startedAtMs: number | null;
  finishedAtMs: number | null;
  /** 工作量未知时为 null。 */
  progress: TaskAmount | null;
  error: string | null;
  actions: TaskAction[];
}

export interface PluginTaskEntry extends TaskEntryBase {
  source: 'plugin';
  job: SchedTaskRecord;
}

export interface BuiltinTaskEntry extends TaskEntryBase {
  source: 'builtin';
  kind: BackendTaskKind;
  view: TaskView;
  /** 文件恢复扫描的会话 id。 */
  scanId?: string;
}

export type TaskEntry = PluginTaskEntry | BuiltinTaskEntry;

export type TaskEntryEvent =
  | { kind: 'changed'; entry: TaskEntry }
  | { kind: 'removed'; id: string }
  | { kind: 'notice'; notice: SchedNotice };

export const TASK_ENTRY_EVENT = 'task-entry';

/** 取消任务:内置任务给任务 id,插件任务给 `plugin:<id>`。取消是协作式的。 */
export function taskCancel(args: { taskId: string }): Promise<void> {
  return invokeCmd<void>('task_cancel', args);
}

export function taskStatus(args: { taskId: string }): Promise<TaskEntry | null> {
  return invokeCmd<TaskEntry | null>('task_status', args);
}

/** 任务中心的任务;`imageId` 省略时列出全部检材。不属于任何检材的任务总会列出。 */
export function taskList(args: { imageId?: string | null } = {}): Promise<TaskEntry[]> {
  return invokeCmd<TaskEntry[]>('task_list', { imageId: args.imageId ?? null });
}

export function onTaskEntry(
  handler: Parameters<typeof onEvent<TaskEntryEvent>>[1],
): Promise<UnlistenFn> {
  return onEvent<TaskEntryEvent>(TASK_ENTRY_EVENT, handler);
}

export function onTaskProgress(
  handler: Parameters<typeof onEvent<BackendTaskProgressEvent>>[1],
): Promise<UnlistenFn> {
  return onEvent<BackendTaskProgressEvent>('task-progress', handler);
}
