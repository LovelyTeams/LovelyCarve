import type { SchedTaskStatus } from '../../../lib/ipc/tasks';

/** 任务展示分组:created/queued 归为 todo。 */
export type SchedTaskGroup = 'todo' | 'running' | 'success' | 'failed' | 'cancel' | 'pause' | 'offline';

const TASK_GROUP: Record<SchedTaskStatus, SchedTaskGroup> = {
  created: 'todo',
  queued: 'todo',
  running: 'running',
  success: 'success',
  failed: 'failed',
  cancel: 'cancel',
  pause: 'pause',
  offline: 'offline',
};

export function taskGroupOf(status: SchedTaskStatus): SchedTaskGroup {
  return TASK_GROUP[status];
}

export const TASK_STATUS_LABEL: Record<SchedTaskStatus, string> = {
  created: '待派发',
  queued: '已排队',
  running: '执行中',
  success: '成功',
  failed: '失败',
  cancel: '已取消',
  pause: '已暂停',
  offline: '已离线',
};

export function isTerminalStatus(status: SchedTaskStatus): boolean {
  return status === 'success' || status === 'failed' || status === 'cancel';
}

export function canCancelStatus(status: SchedTaskStatus): boolean {
  return status === 'created' || status === 'queued' || status === 'running' || status === 'pause' || status === 'offline';
}

/** Built-in recoveries pause and resume; plugin tasks do not pause. */
export function canPauseStatus(status: SchedTaskStatus): boolean {
  return status === 'created' || status === 'queued' || status === 'running';
}

/** Plugin tasks offline, or paused by an older version, start again from the beginning. */
export function canStartStatus(status: SchedTaskStatus): boolean {
  return status === 'pause' || status === 'offline';
}

export function canRestartStatus(status: SchedTaskStatus): boolean {
  return isTerminalStatus(status) || status === 'offline';
}
