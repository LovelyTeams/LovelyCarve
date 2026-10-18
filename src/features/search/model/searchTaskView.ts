import { joinList, sourceText, type Translator } from '../../../i18n/translate';
import type { GlobalSearchHistoryEntry, GlobalSearchProgress } from '../../../lib/ipc/search';

export function taskFullTitle(task: GlobalSearchHistoryEntry, tr: Translator = sourceText): string {
  return joinList([...task.keywords, ...task.regexes.map(pattern => `/${pattern}`)], tr);
}

export function formatTaskTime(task: GlobalSearchHistoryEntry): string {
  const started = task.started_ms > 0 ? task.started_ms : task.finished_ms ?? 0;
  if (!started) return '—';
  const date = new Date(started);
  const pad = (value: number) => String(value).padStart(2, '0');
  return `${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

export function formatElapsedLabel(task: GlobalSearchHistoryEntry): string {
  const finished = task.finished_ms ?? 0;
  if (!finished || !task.started_ms) return '—';
  const seconds = Math.max(0, Math.round((finished - task.started_ms) / 1000));
  if (seconds < 60) return `${seconds}s`;
  return `${Math.floor(seconds / 60)}m${seconds % 60}s`;
}

export function statusLabel(status: string, tr: Translator = sourceText): string {
  return ({ running: tr("搜索中"), success: tr("已完成"), cancelled: tr("已取消"), failed: tr("失败") } as Record<string, string>)[status] ?? status;
}
export function searchProgressPercent(progress: GlobalSearchProgress | null): number | null {
  return progress?.files_total ? Math.min(100, Math.round(progress.files_done / progress.files_total * 100)) : null;
}
