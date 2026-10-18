
import { tr } from './useLocale';
import { readonly, ref } from 'vue';

export type ToastKind = 'success' | 'error' | 'info';

export interface ToastAction {
  label: string;
  onClick: () => void;
  disabled?: boolean;
}

export interface ToastItem {
  id: number;
  kind: ToastKind;
  title: string;
  detail?: string;
  /** When set, the toast does NOT auto-dismiss. Caller controls lifetime
   *  via the handle returned by `pushPersistentToast`. */
  persistent?: boolean;
  /** Optional 0..100 progress bar. Omit for indeterminate/no-bar style. */
  progress?: number;
  /** Optional action button (e.g. "取消") rendered next to the close button. */
  action?: ToastAction;
}

const toasts = ref<ToastItem[]>([]);
let nextToastId = 1;

/** Auto-dismiss clock of a timed toast. `timer` is unset while it is paused. */
interface DismissClock {
  timer: number | undefined;
  remainingMs: number;
  startedAt: number;
}

const dismissClocks = new Map<number, DismissClock>();

/** Never let a toast vanish the instant the pointer or focus leaves it. */
const RESUME_GRACE_MS = 1500;
const MAX_DURATION_MS = 10_000;

/** Long messages stay up long enough to read: ~90 ms per character on top
 *  of a short floor, never below the kind's base duration, capped at 10 s. */
export function toastDuration(baseMs: number, title: string, detail?: string): number {
  const characters = title.length + (detail?.length ?? 0);
  return Math.min(MAX_DURATION_MS, Math.max(baseMs, 1800 + characters * 90));
}

function startDismissClock(id: number, durationMs: number) {
  dismissClocks.set(id, {
    timer: window.setTimeout(() => removeToast(id), durationMs),
    remainingMs: durationMs,
    startedAt: Date.now(),
  });
}

function removeToast(id: number) {
  const clock = dismissClocks.get(id);
  if (clock?.timer !== undefined) window.clearTimeout(clock.timer);
  dismissClocks.delete(id);
  const index = toasts.value.findIndex(toast => toast.id === id);
  if (index >= 0) {
    toasts.value.splice(index, 1);
  }
}

/** Hold a timed toast while the user points at or focuses it, so it never
 *  disappears mid-read or under the cursor. Persistent toasts have no clock. */
export function pauseToast(id: number) {
  const clock = dismissClocks.get(id);
  if (!clock || clock.timer === undefined) return;
  window.clearTimeout(clock.timer);
  clock.timer = undefined;
  clock.remainingMs = Math.max(0, clock.remainingMs - (Date.now() - clock.startedAt));
}

/** Restart a paused toast's clock with what was left of it (plus a grace period). */
export function resumeToast(id: number) {
  const clock = dismissClocks.get(id);
  if (!clock || clock.timer !== undefined) return;
  clock.remainingMs = Math.max(clock.remainingMs, RESUME_GRACE_MS);
  clock.startedAt = Date.now();
  clock.timer = window.setTimeout(() => removeToast(id), clock.remainingMs);
}

function pushToast(kind: ToastKind, title: string, detail?: string, baseMs = 3200) {
  const id = nextToastId++;
  toasts.value.push({ id, kind, title, detail });
  startDismissClock(id, toastDuration(baseMs, title, detail));
  return id;
}

export function notifySuccess(title: string, detail?: string) {
  return pushToast('success', title, detail);
}

export function notifyError(title: string, detail?: string) {
  return pushToast('error', title, detail, 4200);
}

export function notifyInfo(title: string, detail?: string) {
  return pushToast('info', title, detail);
}

export function notifyExportSuccess(path: string, label = tr("导出成功")) {
  return notifySuccess(label, tr("已导出到：{0}", { 0: path }));
}

export interface PersistentToastOptions {
  kind?: ToastKind;
  title: string;
  detail?: string;
  progress?: number;
  action?: ToastAction;
}

export interface PersistentToastHandle {
  id: number;
  /** Patch any subset of the toast's fields. Useful for live progress
   *  updates (detail + progress) without re-pushing. */
  update(patch: Partial<Omit<ToastItem, 'id'>>): void;
  /** Remove the toast immediately, or after `delayMs` if provided
   *  (useful for "success" / "cancelled" tail-state with auto-dismiss). */
  dismiss(delayMs?: number): void;
}

export function pushPersistentToast(options: PersistentToastOptions): PersistentToastHandle {
  const id = nextToastId++;
  toasts.value.push({
    id,
    kind: options.kind ?? 'info',
    title: options.title,
    detail: options.detail,
    progress: options.progress,
    action: options.action,
    persistent: true,
  });
  return {
    id,
    update(patch) {
      const idx = toasts.value.findIndex(t => t.id === id);
      if (idx < 0) return;
      toasts.value[idx] = { ...toasts.value[idx], ...patch };
    },
    dismiss(delayMs?: number) {
      if (delayMs == null) {
        removeToast(id);
      } else {
        window.setTimeout(() => removeToast(id), delayMs);
      }
    },
  };
}

export function useToast() {
  return {
    toasts: readonly(toasts),
    removeToast,
    pauseToast,
    resumeToast,
    notifySuccess,
    notifyError,
    notifyInfo,
    notifyExportSuccess,
    pushPersistentToast,
  };
}
