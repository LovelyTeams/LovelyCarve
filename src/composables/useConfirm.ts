
import { tr } from './useLocale';
// Promise-based custom confirm dialog.
//
// Replaces `window.confirm()` — which in Tauri's webview is either
// silently no-op or blocking the renderer thread, depending on platform.
// All callers `await confirm(opts)` and receive a boolean. Wired through
// a module-level singleton so the dialog state is reachable from any
// component without prop-drilling; mounted by `<ConfirmDialogHost />`
// in the App root.
//
// Why not a stack: at most one confirm should be open at a time —
// stacking them is bad UX (which one is the "current" question?) and
// the previous request's promise would dangle. If a second `confirm()`
// fires while one is already showing, we resolve the prior one to
// `false` (cancel) and replace it. Cheap and predictable.

import { ref, readonly } from 'vue';

export interface ConfirmOptions {
  /** Bold heading row of the dialog. */
  title: string;
  /** Body text. Plain string; rendered as-is (no v-html). */
  message: string;
  /** Defaults to "确认". */
  confirmText?: string;
  /** Defaults to "取消". */
  cancelText?: string;
  /** Red confirm button + alert icon when true. Use for destructive
   *  actions ("删除", "清空", "丢弃修改"). */
  danger?: boolean;
  /** Optional checkbox for one-off preferences such as "don't ask again". */
  checkbox?: {
    label: string;
    initialChecked?: boolean;
  };
}

export interface ConfirmResult {
  confirmed: boolean;
  checkboxChecked: boolean;
}

interface ActiveConfirm extends Required<Omit<ConfirmOptions, 'danger' | 'checkbox'>> {
  danger: boolean;
  checkbox: NonNullable<ConfirmOptions['checkbox']> | null;
  checkboxChecked: boolean;
  /** Resolver bound at `confirm()` call time so the caller's await
   *  fires when the user clicks a button or presses Esc. */
  resolve: (result: ConfirmResult) => void;
}

const active = ref<ActiveConfirm | null>(null);

/**
 * Show a custom confirm dialog. Resolves `true` if the user clicks the
 * confirm button, `false` for cancel / Esc / overlay click / a newer
 * confirm replacing this one.
 *
 * Usage:
 *   if (await confirm({ title: '删除', message: '...', danger: true })) {
 *     doDelete();
 *   }
 */
function openConfirm(opts: ConfirmOptions): Promise<ConfirmResult> {
  // If something is already pending, resolve it `false` first so the
  // prior caller doesn't hang forever and so we never have two dialogs
  // visible at once.
  if (active.value) {
    active.value.resolve({ confirmed: false, checkboxChecked: active.value.checkboxChecked });
    active.value = null;
  }
  return new Promise<ConfirmResult>((resolve) => {
    active.value = {
      title: opts.title,
      message: opts.message,
      confirmText: opts.confirmText ?? tr("确认"),
      cancelText: opts.cancelText ?? tr("取消"),
      danger: Boolean(opts.danger),
      checkbox: opts.checkbox ?? null,
      checkboxChecked: Boolean(opts.checkbox?.initialChecked),
      resolve,
    };
  });
}

export function confirm(opts: ConfirmOptions): Promise<boolean> {
  return openConfirm(opts).then((result) => result.confirmed);
}

export function confirmWithResult(opts: ConfirmOptions): Promise<ConfirmResult> {
  return openConfirm(opts);
}

/** Resolve the active confirm with the given choice and clear state.
 *  Intended for `<ConfirmDialogHost>` to call from its button handlers. */
function resolveActive(ok: boolean) {
  const a = active.value;
  if (!a) return;
  active.value = null;
  a.resolve({ confirmed: ok, checkboxChecked: a.checkboxChecked });
}

function setCheckboxChecked(checked: boolean) {
  if (active.value?.checkbox) active.value.checkboxChecked = checked;
}

export function useConfirmHost() {
  return {
    active: readonly(active),
    onConfirm: () => resolveActive(true),
    onCancel: () => resolveActive(false),
    setCheckboxChecked,
  };
}
