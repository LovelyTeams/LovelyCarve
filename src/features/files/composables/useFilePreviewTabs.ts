import { computed, shallowRef, watch } from 'vue';
import type { FilePreviewReadingState } from '../../../lib/models/filePreviewReadingState';
import type { FileByteFocus } from '../model/navigation';
import {
  activatePreview, activePreviewTab, closePreviews, emptyPreviewTabs, mountedPreviews, pinPreview,
  releaseCursorPreview, showPreview, updatePreview, type FilePreviewClock, type FilePreviewRequest, type FilePreviewTabsState,
} from '../model/previewTabs';
import type { ImageId } from '../../../lib/ipc/ids';

/** The files one evidence has open in the file list's preview. Each evidence
 *  keeps its own set while the workspace switches between evidence tabs.
 *  A viewer that leaves the mounted set hands its reading context to its tab,
 *  so mounting it again returns to the same view, page and scroll position. */
export function useFilePreviewTabs(options: {
  imageId: () => ImageId;
  /** The reading context of a mounted viewer, if it has one. */
  readingStateOf: (tabId: string) => FilePreviewReadingState | undefined;
}) {
  const state = shallowRef<FilePreviewTabsState>(emptyPreviewTabs());
  const saved = new Map<string, FilePreviewTabsState>();
  let sequence = 0;
  let time = 0;
  const clock: FilePreviewClock = { nextId: () => `preview-${++sequence}`, now: () => ++time };

  const tabs = computed(() => state.value.tabs);
  const active = computed(() => activePreviewTab(state.value));
  const mounted = computed(() => mountedPreviews(state.value));

  /** Record the reading context of the viewers `next` no longer mounts. */
  function handOver(next: FilePreviewTabsState, keep: ReadonlySet<string>): FilePreviewTabsState {
    let result = next;
    for (const tab of mounted.value) {
      if (keep.has(tab.id) || !result.tabs.some(item => item.id === tab.id && item.node.id === tab.node.id)) continue;
      const readingState = options.readingStateOf(tab.id);
      if (readingState) result = updatePreview(result, tab.id, { readingState });
    }
    return result;
  }
  function commit(next: FilePreviewTabsState) {
    if (next === state.value) return;
    state.value = handOver(next, new Set(mountedPreviews(next).map(tab => tab.id)));
  }
  /** Every mounted viewer is about to go: the panel hides or the evidence changes. */
  function captureAll(): FilePreviewTabsState {
    return handOver(state.value, new Set());
  }

  watch(options.imageId, (next, previous) => {
    if (previous) saved.set(previous, captureAll());
    state.value = saved.get(next) ?? emptyPreviewTabs();
    saved.delete(next);
  }, { flush: 'sync' });

  return {
    tabs, active, mounted,
    show: (request: FilePreviewRequest) => commit(showPreview(state.value, request, clock)),
    activate: (id: string) => commit(activatePreview(state.value, id, clock)),
    pin: (id: string) => commit(pinPreview(state.value, id)),
    close: (id: string) => commit(closePreviews(state.value, [id])),
    closeOthers: (id: string) => commit(closePreviews(state.value, state.value.tabs.filter(tab => tab.id !== id).map(tab => tab.id))),
    closeAll: () => commit(closePreviews(state.value, state.value.tabs.map(tab => tab.id))),
    /** The list moved away from the cursor row. */
    release: () => commit(releaseCursorPreview(state.value)),
    setFocus(focus: FileByteFocus | null) {
      const tab = active.value;
      if (!tab || (focus && focus.nodeId !== tab.node.id)) return;
      commit(updatePreview(state.value, tab.id, { focus }));
    },
    /** Keep the mounted viewers' reading context before the panel unmounts them. */
    captureAll() { state.value = captureAll(); },
    /** Another case: none of the remembered evidence sets apply any more. */
    reset() {
      saved.clear();
      state.value = emptyPreviewTabs();
    },
  };
}
