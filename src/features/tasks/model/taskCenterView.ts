import type { SchedTaskRecord, SchedTaskStatus } from '../../../lib/ipc/tasks';

const PRIORITY: Record<SchedTaskStatus, number> = {
  running: 0, failed: 1, offline: 1, pause: 2, queued: 3, created: 3, success: 4, cancel: 4,
};

/** The scheduler stores `YYYY-MM-DD HH:MM:SS` in UTC without a zone suffix
 *  (`core::carve::now_datetime`); built-in tasks carry ISO strings. */
export function parseTaskTime(value: string | null | undefined): number {
  if (!value) return Number.NaN;
  const naiveUtc = /^(\d{4}-\d{2}-\d{2})[ T](\d{2}:\d{2}:\d{2}(?:\.\d+)?)$/.exec(value);
  return Date.parse(naiveUtc ? `${naiveUtc[1]}T${naiveUtc[2]}Z` : value);
}

/** Active work and actionable failures precede completed history; newest first within a group. */
export function compareTaskPriority(left: { id: string | number; status: SchedTaskStatus; createAt?: string }, right: { id: string | number; status: SchedTaskStatus; createAt?: string }): number {
  return PRIORITY[left.status] - PRIORITY[right.status]
    || (parseTaskTime(right.createAt) - parseTaskTime(left.createAt) || 0)
    || String(right.id).localeCompare(String(left.id), undefined, { numeric: true });
}

/** Successful tasks are complete; interrupted tasks retain their last progress. */
export function taskProgressPercent(task: Pick<SchedTaskRecord, 'status' | 'progress'>): number {
  if (task.status === 'success') return 100;
  return Number.isFinite(task.progress) ? Math.max(0, Math.min(100, task.progress)) : 0;
}

export function formatTaskDuration(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds) || seconds < 0) return '—';
  const total = Math.floor(seconds);
  return [Math.floor(total / 3600), Math.floor(total / 60) % 60, total % 60]
    .map(part => String(part).padStart(2, '0')).join(':');
}

const pad = (value: number) => String(value).padStart(2, '0');

/** List time: only the clock today, the date within this year, the full date otherwise. */
export function formatTaskClock(value: string | null | undefined, now = new Date()): string {
  const time = parseTaskTime(value);
  if (!Number.isFinite(time)) return value || '—';
  const date = new Date(time);
  const clock = `${pad(date.getHours())}:${pad(date.getMinutes())}`;
  if (date.toDateString() === now.toDateString()) return clock;
  const day = `${pad(date.getMonth() + 1)}/${pad(date.getDate())}`;
  return date.getFullYear() === now.getFullYear() ? `${day} ${clock}` : `${date.getFullYear()}/${day} ${clock}`;
}

/** Detail time with seconds, in local time. */
export function formatTaskDateTime(value: string | null | undefined): string {
  const time = parseTaskTime(value);
  if (!Number.isFinite(time)) return value || '—';
  const date = new Date(time);
  return `${date.getFullYear()}/${pad(date.getMonth() + 1)}/${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

/** Status colour of a task (progress bar and badge tone), as a theme token. */
export const TASK_STATUS_TONE: Record<SchedTaskStatus, string> = {
  running: 'var(--primary-color)',
  created: 'var(--sev-warn-fg)', queued: 'var(--sev-warn-fg)', pause: 'var(--sev-warn-fg)',
  failed: 'var(--sev-critical-fg)', offline: 'var(--sev-critical-fg)',
  success: 'var(--sev-success-fg)', cancel: 'var(--text-secondary)',
};

/** 进行中 = running, waiting or paused; 需要处理 = failed or offline; 已结束 = success or cancelled. */
export type TaskSectionKey = 'active' | 'attention' | 'finished';

const SECTION_OF: Record<SchedTaskStatus, TaskSectionKey> = {
  created: 'active', queued: 'active', running: 'active', pause: 'active',
  failed: 'attention', offline: 'attention',
  success: 'finished', cancel: 'finished',
};
export const TASK_SECTION_LABEL: Record<TaskSectionKey, string> = { active: '进行中', attention: '需要处理', finished: '已结束' };
const SECTION_ORDER: TaskSectionKey[] = ['active', 'attention', 'finished'];

export function taskSectionOf(status: SchedTaskStatus): TaskSectionKey {
  return SECTION_OF[status];
}

/** Non-empty sections in display order; each list keeps the priority order. */
export function groupTaskSections<T extends { id: string | number; status: SchedTaskStatus; createAt?: string }>(tasks: readonly T[]): { key: TaskSectionKey; label: string; tasks: T[] }[] {
  const sorted = [...tasks].sort(compareTaskPriority);
  return SECTION_ORDER
    .map(key => ({ key, label: TASK_SECTION_LABEL[key], tasks: sorted.filter(task => SECTION_OF[task.status] === key) }))
    .filter(section => section.tasks.length > 0);
}

/** Workspace pages a task's results open in (sidebar panel ids). */
export type TaskResultPanel = 'analysis' | 'search-results' | 'timeline' | 'carve' | 'files' | 'dashboard';

export const TASK_RESULT_LABEL: Record<TaskResultPanel, string> = {
  analysis: '查看分析结果', 'search-results': '打开全局搜索', timeline: '打开时间线', carve: '打开文件恢复', files: '打开文件列表', dashboard: '打开镜像信息',
};

const BUILTIN_PANEL: Record<string, TaskResultPanel> = {
  search: 'search-results', timeline: 'timeline', carve: 'carve', docker: 'files', filesystem: 'files', index: 'files', autodetect: 'dashboard', encryptedUnlock: 'dashboard',
};

/** Built-in operations always lead back to their page; plugin tasks only once
 *  a successful run stored records in the analysis results. */
export function taskResultPanel(task: { builtin: boolean; status: SchedTaskStatus; info: { analyzer: string }; result: { artifactCount: number } | null }): TaskResultPanel | null {
  if (task.builtin) return BUILTIN_PANEL[task.info.analyzer] ?? null;
  return task.status === 'success' && (task.result?.artifactCount ?? 0) > 0 ? 'analysis' : null;
}
