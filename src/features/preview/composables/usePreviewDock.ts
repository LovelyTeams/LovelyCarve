
import { tr } from '../../../composables/useLocale';
import { computed, nextTick, onBeforeUnmount, ref, shallowRef } from 'vue';
import { onPreviewDockRequest, onPreviewSelectionRequest, publishPreviewSelection } from '../../../lib/ipc/previewDock';
import { focusFilePreviewWindow, openFilePreviewWindow, type FilePreviewWindowArgs } from '../../../lib/ipc/windows';
import type { UnlistenFn } from '../../../lib/ipc/events';
import { notifyError } from '../../../composables/useToast';
import type { FilePreviewReadingState } from '../../../lib/models/filePreviewReadingState';

/** Tracks ownership so returning a preview cannot target another evidence tab. */
export function usePreviewDock<T>(options: {
  imageId: () => string;
  activate?: (imageId: string) => boolean;
  closed?: () => void;
  restore: (node: T, focusTarget?: FilePreviewWindowArgs['focusTarget'], readingState?: FilePreviewReadingState) => void;
}) {
  const ownerId = crypto.randomUUID();
  const opening = ref(false);
  const preview = shallowRef<{
    id: string;
    args: FilePreviewWindowArgs & { node: T };
    revision: number;
    opened: boolean;
  } | null>(null);
  const detached = computed(() => preview.value?.opened === true);
  let disposed = false;
  const listeners: UnlistenFn[] = [];
  const ready = onPreviewDockRequest(ownerId, async (previewId, readingState) => {
    const current = preview.value;
    if (disposed || !current || current.id !== previewId || current.args.imageId !== options.imageId()) return false;
    if (options.activate && !options.activate(current.args.imageId)) return false;
    // KeepAlive activation may clear the old selection while reloading a list.
    await nextTick();
    const latest = preview.value;
    if (disposed || latest?.id !== previewId || latest.args.imageId !== options.imageId()) return false;
    if (readingState && (readingState.imageId !== latest.args.imageId || readingState.nodeId !== latest.args.nodeId)) return false;
    preview.value = null;
    const context = readingState ?? latest.args.readingState;
    if (context) options.restore(latest.args.node, latest.args.focusTarget, context);
    else if (latest.args.focusTarget) options.restore(latest.args.node, latest.args.focusTarget);
    else options.restore(latest.args.node);
    return true;
  }).then(async cleanup => {
    if (disposed) cleanup();
    else listeners.push(cleanup);
    const selectionCleanup = await onPreviewSelectionRequest(ownerId, async previewId => {
      const current = preview.value;
      if (!disposed && current?.id === previewId) {
        await publishPreviewSelection({ ownerId, previewId, revision: current.revision, args: current.args });
      }
    });
    if (disposed) selectionCleanup();
    else listeners.push(selectionCleanup);
  });
  void ready.catch(error => {
    for (const cleanup of listeners.splice(0)) cleanup();
    notifyError(tr("预览停靠初始化失败"), String(error));
  });

  async function sync(args: FilePreviewWindowArgs & { node: T }): Promise<void> {
    const current = preview.value;
    if (disposed || !current) return;
    const updated = { ...current, args, revision: current.revision + 1 };
    preview.value = updated;
    if (!updated.opened) return;
    try {
      await publishPreviewSelection({ ownerId, previewId: updated.id, revision: updated.revision, args });
    } catch (error) {
      notifyError(tr("同步独立预览失败"), String(error));
    }
  }

  async function focus(): Promise<boolean> {
    const current = preview.value;
    if (disposed || !current?.opened) return false;
    try {
      const focused = await focusFilePreviewWindow(current.id);
      if (!focused && preview.value?.id === current.id) {
        preview.value = null;
        options.closed?.();
      }
      return focused;
    } catch (error) {
      notifyError(tr("聚焦独立预览失败"), String(error));
      return false;
    }
  }

  async function detach(args: FilePreviewWindowArgs & { node: T }): Promise<boolean> {
    if (opening.value || disposed) return false;
    if (preview.value?.opened) {
      await sync(args);
      return focus();
    }
    opening.value = true;
    const previewId = crypto.randomUUID();
    preview.value = { id: previewId, args, revision: 0, opened: false };
    try {
      await ready;
      if (disposed || preview.value?.id !== previewId) return false;
      await openFilePreviewWindow({ ...preview.value.args, ownerId, previewId }, () => {
        if (preview.value?.id === previewId) {
          preview.value = null;
          options.closed?.();
        }
      });
      if (disposed || preview.value?.id !== previewId) return false;
      preview.value = { ...preview.value, opened: true };
      await sync(preview.value.args);
      return true;
    } catch (error) {
      if (preview.value?.id === previewId) preview.value = null;
      notifyError(tr("打开独立预览失败"), String(error));
      return false;
    } finally {
      opening.value = false;
    }
  }

  onBeforeUnmount(() => {
    disposed = true;
    for (const cleanup of listeners.splice(0)) cleanup();
    preview.value = null;
  });
  return { opening, detached, detach, sync, focus };
}
