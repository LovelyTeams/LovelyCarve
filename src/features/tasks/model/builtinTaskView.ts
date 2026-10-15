import { sourceText, type Translator } from '../../../i18n/translate';
import type { BuiltinTask } from './types';
import type { SchedTaskStatus } from '../../../lib/ipc/tasks';
import type { CarveScanSnapshot } from '../../../lib/ipc/carve';
import { formatBytes } from '../../../utils/format';

export function makeBuiltinTask(kind: BuiltinTask['kind'], id: string, imageId: string, subject: string, now: number, tr: Translator = sourceText): BuiltinTask {
  return { id, kind, imageId, subject, title: { docker: tr("Docker 分析"), search: tr("全局搜索"), timeline: tr("时间线重建"), carve: tr("文件雕刻"), filesystem: tr("文件系统加载"), index: tr("索引建立"), autodetect: tr("自动识别平台"), encryptedUnlock: tr("加密卷解锁"), imageExport: tr("镜像导出") }[kind],
    trigger: 'manually', status: 'running', progress: null, message: '', error: null, started: now, finished: null,
    cancelId: kind === 'search' ? id.slice('search:'.length) : kind === 'timeline' ? `timeline-build-${imageId}` : null,
    cancelling: false, results: 0, timelineRows: 0 };
}
export function taskPercent(done: number, total: number): number | null {
  return total > 0 ? Math.min(100, Math.max(0, Math.round(done / total * 100))) : null;
}
export function taskStatus(value: string): SchedTaskStatus {
  switch (value) {
    case 'running': return 'running';
    case 'success': return 'success';
    case 'cancelled': return 'cancel';
    default: return 'failed';
  }
}
export const activeCarveStatuses = ['queued', 'running', 'pause'];

export function projectCarveTask(snapshot: CarveScanSnapshot, previous: BuiltinTask | undefined, now: number, tr: Translator = sourceText): BuiltinTask | undefined {
  const id = `carve:${snapshot.scan_id}`;

  const mappedStatus: SchedTaskStatus = ({ queued: 'queued', running: 'running', paused: 'pause', completed: 'success', cancelled: 'cancel', failed: 'failed' } as const)[snapshot.status];
  const active = activeCarveStatuses.includes(mappedStatus);
  // A delayed status request must not revive a scan that already finished.
  if (previous && !activeCarveStatuses.includes(previous.status) && active) return;
  return { ...makeBuiltinTask('carve', id, snapshot.image_id, snapshot.scope_label, now, tr),
    status: mappedStatus, progress: snapshot.status === 'completed' ? 100 : taskPercent(snapshot.bytes_scanned, snapshot.bytes_total),
    started: snapshot.created_unix_ms, finished: active ? null : previous?.finished ?? now,
    cancelId: snapshot.task_id, cancelling: active && !!previous?.cancelling,
    error: snapshot.error, results: snapshot.artifacts_found,
    message: tr("已扫描 {0} / {1} · 发现 {2} 个文件{3}", { 0: formatBytes(snapshot.bytes_scanned), 1: formatBytes(snapshot.bytes_total), 2: snapshot.artifacts_found.toLocaleString(), 3: snapshot.truncated ? tr(" · 已达结果上限") : '' }),
  };
}
