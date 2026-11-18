import { describe, expect, it } from 'vitest';
import type { DiskFsNode } from './model';
import {
  PREVIEW_TAB_LIMIT, activatePreview, activePreviewTab, closePreviews, emptyPreviewTabs, mountedPreviews,
  pinPreview, releaseCursorPreview, showPreview, updatePreview, type FilePreviewClock, type FilePreviewTabsState,
} from './previewTabs';

const file = (id: string, kind: DiskFsNode['kind'] = 'file'): DiskFsNode => ({
  id, name: `${id}.bin`, kind, size: 1, parent_id: 'root', deleted: false, inode: null,
  timestamps: { modified: null, accessed: null, created: null, mft_changed: null, birth: null },
});
function clock(): FilePreviewClock {
  let id = 0;
  let time = 0;
  return { nextId: () => `preview-${++id}`, now: () => ++time };
}
const summary = (state: FilePreviewTabsState) => ({
  tabs: state.tabs.map(tab => `${tab.id}:${tab.node.id}${tab.pinned ? '' : '*'}`),
  active: activePreviewTab(state)?.node.id ?? null,
});

describe('file preview tabs', () => {
  it('reuses one preview slot for clicks and keeps files that were opened', () => {
    const time = clock();
    let state = showPreview(emptyPreviewTabs(), { node: file('a'), fullPath: '/a.bin' }, time);
    state = showPreview(state, { node: file('b'), fullPath: '/b.bin' }, time);
    expect(summary(state)).toEqual({ tabs: ['preview-1:b*'], active: 'b' });

    state = showPreview(state, { node: file('b'), fullPath: '/b.bin', pin: true }, time);
    state = showPreview(state, { node: file('c'), fullPath: '/dir/c.bin' }, time);
    state = showPreview(state, { node: file('d'), fullPath: '/dir/d.bin', pin: true }, time);
    expect(summary(state)).toEqual({ tabs: ['preview-1:b', 'preview-2:d'], active: 'd' });

    // A file that is already open is activated rather than opened again.
    state = showPreview(state, { node: file('b'), fullPath: '/b.bin' }, time);
    expect(summary(state)).toEqual({ tabs: ['preview-1:b', 'preview-2:d'], active: 'b' });
    state = showPreview(state, { node: file('e'), fullPath: '/e.bin' }, time);
    expect(summary(state)).toEqual({ tabs: ['preview-1:b', 'preview-3:e*', 'preview-2:d'], active: 'e' });
  });

  it('keeps byte focus with its file and drops it with the replaced one', () => {
    const time = clock();
    const focus = { nodeId: 'a', offset: 4096, length: 8 };
    let state = showPreview(emptyPreviewTabs(), { node: file('a'), fullPath: '/a.bin', focus }, time);
    state = showPreview(state, { node: file('a'), fullPath: '/a.bin' }, time);
    expect(activePreviewTab(state)?.focus).toEqual(focus);
    state = showPreview(state, { node: file('b'), fullPath: '/b.bin', focus }, time);
    expect(activePreviewTab(state)?.focus).toBeNull();
    state = updatePreview(state, 'preview-1', { focus: { nodeId: 'b', offset: 1, length: 1 } });
    expect(activePreviewTab(state)?.focus).toEqual({ nodeId: 'b', offset: 1, length: 1 });
  });

  it('never keeps a directory open and lets it go when the list moves', () => {
    const time = clock();
    let state = showPreview(emptyPreviewTabs(), { node: file('a'), fullPath: '/a.bin', pin: true }, time);
    state = showPreview(state, { node: file('docs', 'dir'), fullPath: '/docs', pin: true }, time);
    expect(summary(state)).toEqual({ tabs: ['preview-1:a', 'preview-2:docs*'], active: 'docs' });
    expect(pinPreview(state, 'preview-2')).toBe(state);
    state = releaseCursorPreview(state);
    expect(summary(state)).toEqual({ tabs: ['preview-1:a'], active: 'a' });

    state = showPreview(state, { node: file('b'), fullPath: '/b.bin' }, time);
    expect(releaseCursorPreview(state)).toBe(state);
    expect(summary(pinPreview(state, 'preview-3'))).toEqual({ tabs: ['preview-1:a', 'preview-3:b'], active: 'b' });
  });

  it('returns to the most recently used tab when the active one closes', () => {
    const time = clock();
    let state = emptyPreviewTabs();
    for (const id of ['a', 'b', 'c']) state = showPreview(state, { node: file(id), fullPath: `/${id}.bin`, pin: true }, time);
    state = activatePreview(state, 'preview-1', time);
    state = activatePreview(state, 'preview-3', time);
    state = closePreviews(state, ['preview-3']);
    expect(summary(state)).toEqual({ tabs: ['preview-1:a', 'preview-2:b'], active: 'a' });
    expect(closePreviews(state, ['missing'])).toBe(state);
    state = closePreviews(state, ['preview-2']);
    expect(summary(state)).toEqual({ tabs: ['preview-1:a'], active: 'a' });
    expect(closePreviews(state, ['preview-1'])).toEqual({ tabs: [], activeId: null });
  });

  it('mounts the recently used viewers in creation order and caps the open tabs', () => {
    const time = clock();
    let state = emptyPreviewTabs();
    for (let index = 0; index < PREVIEW_TAB_LIMIT + 2; index++) {
      state = showPreview(state, { node: file(`f${index}`), fullPath: `/f${index}.bin`, pin: true }, time);
    }
    expect(state.tabs).toHaveLength(PREVIEW_TAB_LIMIT);
    expect(state.tabs.some(tab => tab.node.id === 'f0' || tab.node.id === 'f1')).toBe(false);
    state = activatePreview(state, 'preview-5', time);
    expect(mountedPreviews(state, 3).map(tab => tab.id)).toEqual(['preview-5', `preview-${PREVIEW_TAB_LIMIT + 1}`, `preview-${PREVIEW_TAB_LIMIT + 2}`]);
  });
});
