import { computed, watch, type Ref } from 'vue';
import { usePreviewDock } from '../../preview/api';
import { broadcastPreview, onPreviewBroadcast } from '../../preview/api';
import type { FsTypeIdent, StoredDigest } from '../../../lib/ipc/fs/derived';
import type { DiskFsNode } from '../model/model';
import type { FileByteFocus } from '../model/navigation';
import type { FilePreviewReadingState } from '../../../lib/models/filePreviewReadingState';

/** Connects the active preview tab to its inline or detached preview owner. */
export function useFilePreviewBridge(options: {
  imageId: () => string;
  indexOnly?: () => boolean;
  activate: (imageId: string) => boolean;
  /** The file in the active preview tab, its byte focus and the path it was opened at. */
  node: () => DiskFsNode | null;
  focusTarget: () => FileByteFocus | null;
  fullPath: () => string;
  visible: Ref<boolean>;
  /** Docking a detached preview shows its file again with the window's reading context. */
  restore: (node: DiskFsNode, focusTarget: FileByteFocus | null, readingState?: FilePreviewReadingState) => void;
  digest: (nodeId: string) => StoredDigest | undefined;
  ident: (nodeId: string) => FsTypeIdent | undefined;
  identified: (imageId: string, nodeId: string, ident: FsTypeIdent) => void;
  snapshotReadingState: () => FilePreviewReadingState | undefined;
}) {
  const { visible: previewVisible } = options;
  const { opening, detached, detach, sync, focus } = usePreviewDock<DiskFsNode>({
    imageId: options.imageId,
    activate: options.activate,
    closed: () => { previewVisible.value = false; },
    restore: (node, focusTarget, readingState) => {
      options.restore(node, focusTarget ? { nodeId: node.id, ...focusTarget } : null, readingState);
      previewVisible.value = true;
    },
  });
  const dockedPreviewVisible = computed(() => previewVisible.value && !detached.value);
  function togglePreview() {
    if (detached.value) void focus();
    else previewVisible.value = !previewVisible.value;
  }
  function selectionOf(node: DiskFsNode) {
    const fullPath = options.fullPath();
    const context = options.snapshotReadingState();
    const focusTarget = options.focusTarget();
    return {
      indexOnly: options.indexOnly?.(),
      imageId: options.imageId(), nodeId: node.id, title: node.name, fullPath,
      node: { ...node, path: fullPath },
      focusTarget: focusTarget?.nodeId === node.id ? focusTarget : undefined,
      ident: options.ident(node.id), digest: options.digest(node.id),
      readingState: context?.imageId === options.imageId() && context.nodeId === node.id ? context : undefined,
    };
  }
  async function detachPreview(position?: { x: number; y: number }) {
    const node = options.node();
    if (!node) return;
    const selection = selectionOf(node);
    const opened = await detach({ ...selection, position });
    if (opened && options.imageId() === selection.imageId && options.node()?.id === node.id) previewVisible.value = false;
  }
  watch(() => {
    const node = options.node();
    return [node, options.focusTarget(), options.imageId(), options.indexOnly?.(), node && options.ident(node.id), node && options.digest(node.id)];
  }, () => {
    const node = options.node();
    if (node) void sync(selectionOf(node));
  }, { flush: 'post' });

  function onDetailIdentified(payload: { nodeId: string; ident: FsTypeIdent }) {
    const imageId = options.imageId();
    options.identified(imageId, payload.nodeId, payload.ident);
    broadcastPreview({ kind: 'ident', imageId, ...payload });
  }
  onPreviewBroadcast(message => {
    if (message.imageId === options.imageId() && message.kind === 'ident') options.identified(message.imageId, message.nodeId, message.ident);
  });
  return { previewOpening: opening, previewDetached: detached, dockedPreviewVisible, togglePreview, detachPreview, onDetailIdentified };
}
