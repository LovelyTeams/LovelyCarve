import type { FilePreviewReadingState } from '../../../lib/models/filePreviewReadingState';
import type { DiskFsNode } from './model';
import type { FileByteFocus } from './navigation';

/** One file opened in the file list's preview. The single unpinned tab is the
 *  preview slot: a click on another row shows that row in it, so glancing
 *  through a directory does not pile up tabs. Pinned tabs stay until closed,
 *  also while the list moves to other directories. */
export interface FilePreviewTab {
  /** Stable while the tab exists, also when the preview slot shows another file;
   *  numbered in creation order. */
  id: string;
  node: DiskFsNode;
  /** The path when the file was opened; the list may have moved on since. */
  fullPath: string;
  pinned: boolean;
  focus: FileByteFocus | null;
  /** Reading context to restore when the tab's viewer is mounted again. */
  readingState?: FilePreviewReadingState;
  /** Activation order: larger is more recent. */
  usedAt: number;
}

export interface FilePreviewTabsState {
  tabs: FilePreviewTab[];
  activeId: string | null;
}

export interface FilePreviewRequest {
  node: DiskFsNode;
  fullPath: string;
  /** Keep the file open (double click, Enter, the menu) instead of showing it in the preview slot. */
  pin?: boolean;
  /** Bytes to show; `undefined` keeps the focus an already open tab has. */
  focus?: FileByteFocus | null;
  readingState?: FilePreviewReadingState;
}

/** Ids and activation stamps come from the owner so the model stays deterministic. */
export interface FilePreviewClock {
  nextId: () => string;
  now: () => number;
}

/** Tabs beyond this are dropped least recently used first; the active one never. */
export const PREVIEW_TAB_LIMIT = 40;
/** Viewers kept mounted (hidden) so switching back is instant and keeps their state. */
export const MOUNTED_PREVIEW_LIMIT = 6;
/** Files one "open the checked files" action opens at most. */
export const PREVIEW_BATCH_LIMIT = 12;

export const emptyPreviewTabs = (): FilePreviewTabsState => ({ tabs: [], activeId: null });

export function activePreviewTab(state: FilePreviewTabsState): FilePreviewTab | null {
  return state.tabs.find(tab => tab.id === state.activeId) ?? null;
}

function focusFor(node: DiskFsNode, focus: FileByteFocus | null | undefined): FileByteFocus | null {
  return focus && focus.nodeId === node.id ? focus : null;
}

/** Directories show their properties in the preview slot but are never kept open. */
const pinnable = (node: DiskFsNode) => node.kind !== 'dir';

function withLimit(state: FilePreviewTabsState): FilePreviewTabsState {
  if (state.tabs.length <= PREVIEW_TAB_LIMIT) return state;
  const removable = state.tabs.filter(tab => tab.id !== state.activeId)
    .sort((left, right) => left.usedAt - right.usedAt)
    .slice(0, state.tabs.length - PREVIEW_TAB_LIMIT)
    .map(tab => tab.id);
  return { ...state, tabs: state.tabs.filter(tab => !removable.includes(tab.id)) };
}

/** Show a file: activate its tab when it is open, otherwise reuse the preview
 *  slot, otherwise open a tab right after the active one. */
export function showPreview(state: FilePreviewTabsState, request: FilePreviewRequest, clock: FilePreviewClock): FilePreviewTabsState {
  const { node } = request;
  const pin = request.pin === true && pinnable(node);
  const usedAt = clock.now();
  const existing = state.tabs.find(tab => tab.node.id === node.id);
  if (existing) {
    const tab: FilePreviewTab = {
      ...existing, node, fullPath: request.fullPath, pinned: existing.pinned || pin, usedAt,
      focus: request.focus === undefined ? existing.focus : focusFor(node, request.focus),
      readingState: request.readingState ?? existing.readingState,
    };
    return { tabs: state.tabs.map(item => item.id === tab.id ? tab : item), activeId: tab.id };
  }
  const fresh = { node, fullPath: request.fullPath, pinned: pin, focus: focusFor(node, request.focus), readingState: request.readingState, usedAt };
  const slot = state.tabs.find(tab => !tab.pinned);
  if (slot) {
    const tab: FilePreviewTab = { ...fresh, id: slot.id };
    return { tabs: state.tabs.map(item => item.id === slot.id ? tab : item), activeId: slot.id };
  }
  const tab: FilePreviewTab = { ...fresh, id: clock.nextId() };
  const after = state.tabs.findIndex(item => item.id === state.activeId);
  const tabs = after < 0 ? [...state.tabs, tab] : [...state.tabs.slice(0, after + 1), tab, ...state.tabs.slice(after + 1)];
  return withLimit({ tabs, activeId: tab.id });
}

export function activatePreview(state: FilePreviewTabsState, id: string, clock: FilePreviewClock): FilePreviewTabsState {
  if (!state.tabs.some(tab => tab.id === id)) return state;
  const usedAt = clock.now();
  return { tabs: state.tabs.map(tab => tab.id === id ? { ...tab, usedAt } : tab), activeId: id };
}

/** Keep the preview slot's file open; a later click opens another slot. */
export function pinPreview(state: FilePreviewTabsState, id: string): FilePreviewTabsState {
  const tab = state.tabs.find(item => item.id === id);
  if (!tab || tab.pinned || !pinnable(tab.node)) return state;
  return { ...state, tabs: state.tabs.map(item => item.id === id ? { ...item, pinned: true } : item) };
}

/** Closing the active tab returns to the one used most recently before it. */
export function closePreviews(state: FilePreviewTabsState, ids: readonly string[]): FilePreviewTabsState {
  const tabs = state.tabs.filter(tab => !ids.includes(tab.id));
  if (tabs.length === state.tabs.length) return state;
  if (tabs.some(tab => tab.id === state.activeId)) return { ...state, tabs };
  const next = tabs.reduce<FilePreviewTab | null>((best, tab) => !best || tab.usedAt > best.usedAt ? tab : best, null);
  return { tabs, activeId: next?.id ?? null };
}

/** The list moved: a directory's properties in the preview slot described the
 *  row under the old cursor, so they go; files stay open. */
export function releaseCursorPreview(state: FilePreviewTabsState): FilePreviewTabsState {
  const slot = state.tabs.find(tab => !tab.pinned);
  return slot?.node.kind === 'dir' ? closePreviews(state, [slot.id]) : state;
}

export function updatePreview(state: FilePreviewTabsState, id: string, patch: Partial<Pick<FilePreviewTab, 'focus' | 'readingState'>>): FilePreviewTabsState {
  if (!state.tabs.some(tab => tab.id === id)) return state;
  return { ...state, tabs: state.tabs.map(tab => tab.id === id ? { ...tab, ...patch } : tab) };
}

/** The most recently used tabs whose viewers stay mounted, in creation order so
 *  mounted viewers never move in the document (moving would reset their scroll). */
export function mountedPreviews(state: FilePreviewTabsState, limit = MOUNTED_PREVIEW_LIMIT): FilePreviewTab[] {
  const kept = new Set([...state.tabs].sort((left, right) => right.usedAt - left.usedAt).slice(0, limit).map(tab => tab.id));
  return state.tabs.filter(tab => kept.has(tab.id)).sort((left, right) => creationOrder(left.id) - creationOrder(right.id));
}

function creationOrder(id: string): number {
  const order = Number(id.slice(id.lastIndexOf('-') + 1));
  return Number.isFinite(order) ? order : 0;
}
