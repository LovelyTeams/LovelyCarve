// Window-management IPC: spawning detached previews and focusing the main window.
//
// This is the only module besides the rest of lib/ipc allowed to import
// `@tauri-apps/api`. The file-preview pop-out opens a fresh OS window that
// re-renders the detail panel standalone; it shares the same backend process
// (and therefore the same loaded image / AppState), so the standalone window
// fetches bytes through the normal fs commands.
import { WebviewWindow } from '@tauri-apps/api/webviewWindow';
import { invokeCmd } from './client';
import { onWindowDestroyed } from './events';
import type { FilePreviewReadingState } from '../models/filePreviewReadingState';

export interface FilePreviewWindowArgs {
  indexOnly?: boolean;
  ownerId?: string;
  previewId?: string;
  position?: { x: number; y: number };
  imageId: string;
  nodeId: string;
  fullPath: string;
  /** Window title (the file name). */
  title: string;
  /** The FsNode, serialized into the URL so the new window can render the
   *  metadata pane without an extra round-trip. */
  node: unknown;
  /** Already-known Magika identification (from the list cache / in-pane
   *  identify), serialized so the pop-out opens showing the real type instead
   *  of an un-identified "识别真实类型". Live updates after open ride the
   *  cross-window storage channel (see usePreviewSync). */
  ident?: unknown;
  /** Already-computed content hashes (from the list digestCache), serialized so
   *  the pop-out opens with the hash rows filled. Live updates ride the same
   *  cross-window channel. */
  digest?: unknown;
  focusTarget?: { offset: number; length: number };
  readingState?: FilePreviewReadingState;
}

// Window labels must be unique and limited to [a-zA-Z0-9-/:_]. node ids can
// contain other characters, so scrub them to '_'.
function sanitizeLabel(raw: string): string {
  return raw.replace(/[^a-zA-Z0-9/:_-]/g, '_');
}

// Native background painted before the pop-out's first frame. Match the
// active theme's page background so dark-theme users don't get a light flash.
function themedWindowBackground(): string {
  const color = getComputedStyle(document.documentElement).getPropertyValue('--bg-primary').trim();
  return /^#[0-9a-f]{6}$/i.test(color) ? color : '#fcfcfb';
}

/**
 * Open (or focus, if already open) a detached window showing the given file's
 * preview. Owned previews use a stable previewId; other callers use (image, node).
 * Re-invoking with the same identity focuses the existing window.
 */
const pendingPreviews = new Map<string, Promise<void>>();

export async function openFilePreviewWindow(args: FilePreviewWindowArgs, onClosed?: () => void): Promise<void> {
  const label = sanitizeLabel(args.previewId
    ? `file-preview-${args.previewId}`
    : `file-preview-${args.imageId}-${args.nodeId}`);
  const pending = pendingPreviews.get(label);
  if (pending) return pending;
  const opening = createFilePreviewWindow(label, args, onClosed);
  pendingPreviews.set(label, opening);
  try {
    await opening;
  } finally {
    pendingPreviews.delete(label);
  }
}

async function createFilePreviewWindow(label: string, args: FilePreviewWindowArgs, onClosed?: () => void): Promise<void> {
  const existing = await WebviewWindow.getByLabel(label);
  if (existing) {
    await existing.unminimize();
    await existing.setFocus();
    return;
  }

  const params = new URLSearchParams({
    view: 'file-preview',
    imageId: args.imageId,
    nodeId: args.nodeId,
    fullPath: args.fullPath,
    node: JSON.stringify(args.node),
  });
  if (args.ident != null) params.set('ident', JSON.stringify(args.ident));
  if (args.indexOnly) params.set('indexOnly', 'true');
  if (args.digest != null) params.set('digest', JSON.stringify(args.digest));
  if (args.focusTarget) params.set('focusTarget', JSON.stringify(args.focusTarget));
  if (args.readingState) params.set('readingState', JSON.stringify(args.readingState));
  if (args.ownerId) params.set('ownerId', args.ownerId);
  if (args.previewId) params.set('previewId', args.previewId);

  const unlisten = onClosed ? await onWindowDestroyed(label, onClosed) : undefined;
  try {
    await invokeCmd<void>('plugin:preview-window|open', {
      options: {
        label,
        url: `index.html?${params.toString()}`,
        title: args.title || '文件预览',
        position: args.position,
        backgroundColor: themedWindowBackground(),
      },
    });
  } catch (error) {
    unlisten?.();
    throw error;
  }
}

export async function focusMainWindow(): Promise<void> {
  const main = await WebviewWindow.getByLabel('main');
  if (main) {
    await main.unminimize();
    await main.setFocus();
  }
}

/** Focus an owned preview without creating a replacement after it has closed. */
export async function focusFilePreviewWindow(previewId: string): Promise<boolean> {
  const preview = await WebviewWindow.getByLabel(sanitizeLabel(`file-preview-${previewId}`));
  if (!preview) return false;
  await preview.unminimize();
  await preview.setFocus();
  return true;
}
