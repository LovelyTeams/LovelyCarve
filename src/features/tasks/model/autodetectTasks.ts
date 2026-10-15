import { sourceText, type Translator } from '../../../i18n/translate';
import type { BuiltinTask } from './types';
import { evidenceTypeLabel, platformLabel } from '../../evidence/model/autodetectLabels';
import type { AutoDetectFailure, AutoDetectReport } from '../../../lib/ipc/autodetect';
import type { BackendTaskLifecycleEvent, BackendTaskSnapshot } from '../../../lib/ipc/taskRuntime';

const ACTIVE = ['queued', 'running'];
/** `offline` closed without an outcome; the outcome may still arrive and replaces it. */
const AWAITING_OUTCOME = [...ACTIVE, 'offline'];
const RUNNING_MESSAGE = '正在按文件系统类型与标志文件判定平台';

export function isAutoDetectTask(task: BackendTaskSnapshot) {
  return task.kind === 'autoDetect';
}

/** One record per evidence, holding its latest run. */
export function autoDetectTaskId(imageId: string) {
  return `autodetect:${imageId}`;
}

/** 平台(含厂商与备份工具) · 检材类型。 */
export function autoDetectSummary(report: AutoDetectReport, tr: Translator = sourceText): string {
  const platform = report.platform ? report.display_platform_name || platformLabel(report.platform, tr) : tr("未识别到平台");
  return `${platform} · ${evidenceTypeLabel(report.evidence_type, tr)}`;
}

function runTask(native: BackendTaskSnapshot, imageId: string, trigger: BuiltinTask['trigger'], now: number, tr: Translator = sourceText): BuiltinTask {
  const queued = native.status === 'queued';
  return {
    id: autoDetectTaskId(imageId), kind: 'autodetect', imageId, title: tr("自动识别平台"), subject: '', trigger,
    status: queued ? 'queued' : 'running', progress: null, message: queued ? tr("等待识别") : tr(RUNNING_MESSAGE),
    error: null, started: now - native.elapsedMs, finished: null,
    cancelId: native.id, cancelling: native.status === 'cancellationRequested', results: 0, timelineRows: 0,
  };
}

/** Open the run of a live registration (lifecycle event or `task_list` row).
 *  A queued registration is always a new run. Otherwise a closed record is
 *  only reopened by a registration that started after it finished: a second
 *  caller shares the registration of the run already shown. */
export function recoverAutoDetectTask(previous: Map<string, BuiltinTask>, native: BackendTaskSnapshot, trigger: BuiltinTask['trigger'], now: number, restart = false, tr: Translator = sourceText) {
  if (!native.imageId) return previous;
  const id = autoDetectTaskId(native.imageId);
  const current = previous.get(id);
  if (current && !restart) {
    if (ACTIVE.includes(current.status)) {
      const status = native.status === 'queued' ? current.status : 'running';
      const cancelling = current.cancelling || native.status === 'cancellationRequested';
      if (status === current.status && cancelling === current.cancelling) return previous;
      return new Map(previous).set(id, { ...current, status, cancelling, message: status === 'running' ? tr(RUNNING_MESSAGE) : current.message });
    }
    if (current.finished !== null && now - native.elapsedMs <= current.finished) return previous;
  }
  return new Map(previous).set(id, runTask(native, native.imageId, trigger, now, tr));
}

/** Registry removal is not an outcome: a run still waiting for its result goes offline. */
export function applyAutoDetectLifecycle(previous: Map<string, BuiltinTask>, event: BackendTaskLifecycleEvent, trigger: BuiltinTask['trigger'], now: number, tr: Translator = sourceText) {
  const native = event.task;
  if (event.phase === 'queued' || event.phase === 'running') return recoverAutoDetectTask(previous, native, trigger, now, event.phase === 'queued', tr);
  if (!native.imageId) return previous;
  const id = autoDetectTaskId(native.imageId);
  const current = previous.get(id);
  if (!current || !ACTIVE.includes(current.status)) return previous;
  if (event.phase === 'cancellationRequested') return new Map(previous).set(id, { ...current, cancelling: true });
  return new Map(previous).set(id, { ...current, finished: now, cancelling: false,
    ...(native.status === 'cancellationRequested'
      ? { status: 'cancel' as const, message: tr("已取消") }
      : { status: 'offline' as const, error: tr("任务已结束，未收到识别结果，请检查日志") }) });
}

export function applyAutoDetectSuccess(previous: Map<string, BuiltinTask>, report: AutoDetectReport, now: number, tr: Translator = sourceText) {
  const id = autoDetectTaskId(report.image_id);
  const current = previous.get(id);
  if (!current || !AWAITING_OUTCOME.includes(current.status)) return previous;
  return new Map(previous).set(id, { ...current, status: 'success', progress: 100, finished: now, cancelling: false, error: null, message: autoDetectSummary(report, tr) });
}

export function applyAutoDetectFailure(previous: Map<string, BuiltinTask>, failure: AutoDetectFailure, now: number, tr: Translator = sourceText) {
  const id = autoDetectTaskId(failure.image_id);
  const current = previous.get(id);
  if (!current || !AWAITING_OUTCOME.includes(current.status)) return previous;
  const cancelled = failure.error.code === 'cancelled';
  return new Map(previous).set(id, { ...current, status: cancelled ? 'cancel' : 'failed', finished: now, cancelling: false,
    error: cancelled ? null : failure.error.message, message: cancelled ? tr("已取消") : tr("识别失败") });
}
