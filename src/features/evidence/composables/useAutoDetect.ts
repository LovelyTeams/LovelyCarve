
import { tr } from '../../../composables/useLocale';
import { computed, reactive, type ComputedRef } from 'vue';
import {
  AUTODETECT_FAILED_EVENT,
  AUTODETECT_FINISHED_EVENT,
  autodetectEvidence,
  type AutoDetectFailure,
  type AutoDetectReport,
} from '../../../lib/ipc/autodetect';
import { IpcCommandError } from '../../../lib/ipc/client';
import { onEvent, type UnlistenFn } from '../../../lib/ipc/events';
import { notifyError } from '../../../composables/useToast';

// 自动识别平台的前端状态。
// Module-scoped like useImageHash: the backend runs the detection as a
// background first task whenever evidence first joins a case and broadcasts
// the report or the failure; manual re-runs go through `runAutoDetect`.
// Reports are keyed by image id so switching panels never loses a result.
// The task center lists the same runs from the backend (`autodetect:<image id>`).

const reports = reactive(new Map<string, AutoDetectReport>());
const pending = reactive(new Set<string>());
const errors = reactive(new Map<string, string>());

let unlisten: UnlistenFn[] = [];
let listenerStarted = false;
let generation = 0;
const imageGenerations = new Map<string, number>();
let reportPublished: ((report: AutoDetectReport) => void) | null = null;
export function registerAutoDetectReportPublished(handler: (report: AutoDetectReport) => void): () => void {
  reportPublished = handler;
  return () => { if (reportPublished === handler) reportPublished = null; };
}

/** Started with the task tracking at app start, so a first task that finishes
 *  before the image-info page opens still leaves its report here. */
export function ensureAutoDetectListener() {
  if (listenerStarted) return;
  listenerStarted = true;
  const epoch = generation;
  Promise.allSettled([
    onEvent<AutoDetectReport>(AUTODETECT_FINISHED_EVENT, event => {
      if (epoch === generation) applyAutoDetectReport(event.payload);
    }),
    onEvent<AutoDetectFailure>(AUTODETECT_FAILED_EVENT, ({ payload }) => {
      if (epoch === generation && payload.error.code !== 'cancelled') errors.set(payload.image_id, payload.error.message);
    }),
  ]).then(results => {
    const stops = results.flatMap(result => result.status === 'fulfilled' ? [result.value] : []);
    if (epoch !== generation || results.some(result => result.status === 'rejected')) {
      for (const stop of stops) stop();
      if (epoch === generation) listenerStarted = false;
    } else unlisten = stops;
  });
}

/** Store a report (from the finished event or a manual run). The backend has
 *  already written `platform` / `type` onto the case evidence row, so the
 *  active-case DTO is refreshed to pick up the new pill in the case manager. */
export function applyAutoDetectReport(report: AutoDetectReport) {
  reports.set(report.image_id, report);
  errors.delete(report.image_id);
  reportPublished?.(report);
}

export function useAutoDetectReport(
  imageId: () => string | null | undefined,
): ComputedRef<AutoDetectReport | undefined> {
  ensureAutoDetectListener();
  return computed(() => {
    const id = imageId();
    return id ? reports.get(id) : undefined;
  });
}

export function useAutoDetectPending(imageId: () => string | null | undefined): ComputedRef<boolean> {
  return computed(() => {
    const id = imageId();
    return !!id && pending.has(id);
  });
}

export function useAutoDetectError(
  imageId: () => string | null | undefined,
): ComputedRef<string | undefined> {
  return computed(() => {
    const id = imageId();
    return id ? errors.get(id) : undefined;
  });
}

/** 手动触发识别。 */
export async function runAutoDetect(imageId: string): Promise<AutoDetectReport | null> {
  if (pending.has(imageId)) return reports.get(imageId) ?? null;
  pending.add(imageId);
  const epoch = generation;
  const imageEpoch = imageGenerations.get(imageId) ?? 0;
  const current = () => epoch === generation && imageEpoch === (imageGenerations.get(imageId) ?? 0);
  try {
    const report = await autodetectEvidence({ imageId });
    if (!current()) return null;
    applyAutoDetectReport(report);
    return report;
  } catch (e) {
    if (!current()) return null;
    // Cancelling from the task center or unloading the image is not a failure.
    if (e instanceof IpcCommandError && e.code === 'cancelled') return null;
    const message = String(e);
    errors.set(imageId, message);
    notifyError(tr("自动识别失败"), message);
    return null;
  } finally {
    if (current()) pending.delete(imageId);
  }
}

/** Drop cached state for an image — call when it is unloaded. */
export function clearAutoDetectReport(imageId: string) {
  imageGenerations.set(imageId, (imageGenerations.get(imageId) ?? 0) + 1);
  pending.delete(imageId);
  reports.delete(imageId);
  errors.delete(imageId);
}

/** Test hook: reset module state and detach the event listener. */
export function disposeAutoDetect() {
  generation++;
  imageGenerations.clear();
  reportPublished = null;
  reports.clear();
  pending.clear();
  errors.clear();
  for (const stop of unlisten.splice(0)) stop();
  listenerStarted = false;
}

export function resetAutoDetectForTests() { disposeAutoDetect(); }
