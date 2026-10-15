import { sourceText, type Translator, type Locale } from '../../../i18n/translate';
import type { CarveCategory, CarveScanSnapshot } from '../../../lib/ipc/carve';
const MIB = 1024 * 1024;
const GIB = 1024 * MIB;
export const categories: { id: CarveCategory; label: string }[] = [
  { id: 'image', label: '图像' }, { id: 'document', label: '文档' },
  { id: 'archive', label: '归档' }, { id: 'audio', label: '音频' },
  { id: 'video', label: '视频' }, { id: 'database', label: '数据库' },
];
export const categoryFilterOptions: { value: 'all' | CarveCategory; label: string }[] = [
  { value: 'all', label: '全部类别' },
  ...categories.map(category => ({ value: category.id, label: category.label })),
];
export const MAX_FILE_SIZE_OPTIONS = [
  { value: 128 * MIB, label: '128 MB' }, { value: 512 * MIB, label: '512 MB' }, { value: 2 * GIB, label: '2 GB' },
  { value: 4 * GIB, label: '4 GB' }, { value: 10 * GIB, label: '10 GB' },
];
export const MAX_RESULTS_OPTIONS = [
  { value: 5000, label: '5,000' }, { value: 20_000, label: '20,000' }, { value: 50_000, label: '50,000' }, { value: 100_000, label: '100,000' },
];
export function isFinished(task: CarveScanSnapshot) {
  return ['completed', 'cancelled', 'failed'].includes(task.status);
}
export function statusLabel(status: CarveScanSnapshot['status'], tr: Translator = sourceText) {
  return { queued: tr("排队中"), running: tr("扫描中"), paused: tr("已暂停"), completed: tr("已完成"), cancelled: tr("已取消"), failed: tr("失败") }[status];
}
export function scanPercent(task: CarveScanSnapshot) {
  return task.bytes_total > 0 ? Math.min(100, task.bytes_scanned / task.bytes_total * 100) : 0;
}
export function formatScanTime(task: CarveScanSnapshot, locale: Locale = 'zh-CN') {
  return new Date(task.created_unix_ms).toLocaleString(locale, { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' });
}
export function categoryTone(category: CarveCategory): string {
  return {
    image: 'var(--chart-5)',
    document: 'var(--chart-1)',
    archive: 'var(--chart-3)',
    database: 'var(--chart-2)',
    audio: 'var(--chart-6)',
    video: 'var(--chart-4)',
  }[category];
}

export function confidenceTone(confidence: number): 'high' | 'mid' | 'low' {
  if (confidence >= 85) return 'high';
  if (confidence >= 60) return 'mid';
  return 'low';
}

export function formatOffset(offset: number): string {
  return `0x${Math.max(0, offset).toString(16).toUpperCase().padStart(16, '0')}`;
}
