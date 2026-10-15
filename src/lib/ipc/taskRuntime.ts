import { onEvent, type UnlistenFn } from './events';
import { invokeCmd } from './client';



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

export type BackendTaskLifecyclePhase = 'queued' | 'running' | 'cancellationRequested' | 'finished';

export type BackendTaskProgressUnit = 'attempts' | 'bytes' | 'entries';

export type BackendTaskResourceClass =
  | 'globalBackground'
  | 'imageSequentialIo'
  | 'imageRandomIo'
  | 'cpu'
  | 'gpu';

export interface BackendTaskSnapshot {
  id: string;
  kind: BackendTaskKind;
  imageId: string | null;
  status: BackendTaskStatus;
  resourceClasses: BackendTaskResourceClass[];
  elapsedMs: number;
}

export interface BackendTaskLifecycleEvent {
  phase: BackendTaskLifecyclePhase;
  task: BackendTaskSnapshot;
}

export interface BackendTaskProgressEvent {
  task: BackendTaskSnapshot;
  unit: BackendTaskProgressUnit;
  done: number;
  total: number | null;
  message: string | null;
}

export function taskCancel(args: { taskId: string }): Promise<void> {
  return invokeCmd<void>('task_cancel', args);
}

export function taskStatus(args: { taskId: string }): Promise<BackendTaskSnapshot | null> {
  return invokeCmd<BackendTaskSnapshot | null>('task_status', args);
}

export function taskList(): Promise<BackendTaskSnapshot[]> {
  return invokeCmd<BackendTaskSnapshot[]>('task_list');
}

export function onTaskLifecycle(
  handler: Parameters<typeof onEvent<BackendTaskLifecycleEvent>>[1],
): Promise<UnlistenFn> {
  return onEvent<BackendTaskLifecycleEvent>('task-lifecycle', handler);
}

export function onTaskProgress(
  handler: Parameters<typeof onEvent<BackendTaskProgressEvent>>[1],
): Promise<UnlistenFn> {
  return onEvent<BackendTaskProgressEvent>('task-progress', handler);
}
