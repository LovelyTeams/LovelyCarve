import { emitEvent, onEvent, type UnlistenFn } from './events';
import type { FilePreviewWindowArgs } from './windows';
import { parseFilePreviewReadingState, type FilePreviewReadingState } from '../models/filePreviewReadingState';

const REQUEST = 'file-preview-dock-request';
const RESPONSE = 'file-preview-dock-response';
const SELECTION = 'file-preview-selection';
const SELECTION_REQUEST = 'file-preview-selection-request';

interface DockRequest { ownerId: string; previewId: string; requestId: string; readingState?: FilePreviewReadingState }
interface DockResponse { requestId: string; accepted: boolean }

export interface PreviewSelection {
  ownerId: string;
  previewId: string;
  revision: number;
  args: FilePreviewWindowArgs;
}

/** Deliver the current file to the preview belonging to this list. */
export function publishPreviewSelection(selection: PreviewSelection): Promise<void> {
  return emitEvent(SELECTION, selection);
}

/** Subscribe before requesting a snapshot, so changes during window startup are retained. */
export function onPreviewSelection(
  ownerId: string,
  previewId: string,
  handler: (selection: PreviewSelection) => void,
): Promise<UnlistenFn> {
  return onEvent<PreviewSelection>(SELECTION, ({ payload }) => {
    if (payload.ownerId === ownerId && payload.previewId === previewId) handler(payload);
  });
}

export function requestPreviewSelection(ownerId: string, previewId: string): Promise<void> {
  return emitEvent(SELECTION_REQUEST, { ownerId, previewId });
}

/** A newly mounted preview asks its owner for the latest selection. */
export function onPreviewSelectionRequest(
  ownerId: string,
  respond: (previewId: string) => Promise<void>,
): Promise<UnlistenFn> {
  return onEvent<{ ownerId: string; previewId: string }>(SELECTION_REQUEST, ({ payload }) => {
    if (payload.ownerId === ownerId) return respond(payload.previewId);
  });
}

/** Only the originating file list may accept a detached preview back. */
export function onPreviewDockRequest(
  ownerId: string,
  accept: (previewId: string, readingState?: FilePreviewReadingState) => boolean | Promise<boolean>,
): Promise<UnlistenFn> {
  return onEvent<DockRequest>(REQUEST, async ({ payload }) => {
    if (payload.ownerId !== ownerId) return;
    const context = parseFilePreviewReadingState(payload.readingState);
    await emitEvent<DockResponse>(RESPONSE, {
      requestId: payload.requestId,
      accepted: context ? await accept(payload.previewId, context) : await accept(payload.previewId),
    });
  });
}

/** Wait for acknowledgement before closing the independent window. */
export async function requestPreviewDock(ownerId: string, previewId: string, readingState?: FilePreviewReadingState): Promise<void> {
  const requestId = crypto.randomUUID();
  let timer: ReturnType<typeof setTimeout> | undefined;
  let finish: (response: DockResponse) => void = () => {};
  const response = new Promise<void>((resolve, reject) => {
    finish = result => result.accepted
      ? resolve()
      : reject(new Error('原文件列表已切换到其他检材或已关闭，请返回原来的检材后再停靠。'));
    timer = setTimeout(() => reject(new Error('原文件列表未响应，请确认主窗口仍然打开。')), 3000);
  });
  // Attach the rejection handler immediately, even if registering IPC is slow.
  void response.catch(() => {});
  let unlisten: UnlistenFn | undefined;
  try {
    unlisten = await onEvent<DockResponse>(RESPONSE, ({ payload }) => {
      if (payload.requestId === requestId) finish(payload);
    });
    await emitEvent<DockRequest>(REQUEST, { ownerId, previewId, requestId, readingState });
    await response;
  } finally {
    clearTimeout(timer);
    unlisten?.();
  }
}
