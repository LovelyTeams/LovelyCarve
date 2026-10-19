import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { effectScope, ref, shallowRef } from 'vue';
import type { DiskFsNode } from '../model/model';
import type { Page, FsFlatPage } from '../../../lib/ipc/fs/types';
import { useDirectoryQuery } from './useDirectoryQuery';
import { useFlatFileQuery } from './useFlatFileQuery';

const mocks = vi.hoisted(() => ({ children: vi.fn(), flat: vi.fn(), counts: vi.fn(), error: vi.fn() }));
vi.mock('./containerReads', () => ({ containerFsChildrenPage: mocks.children, containerFsFlatPage: mocks.flat }));
vi.mock('../../../lib/ipc/fs/query', () => ({ fsFlatExtCounts: mocks.counts }));
vi.mock('../../../composables/useToast', () => ({ notifyError: mocks.error }));

const scopes: ReturnType<typeof effectScope>[] = [];
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(finish => { resolve = finish; });
  return { promise, resolve };
}
function node(id: string): DiskFsNode {
  return { id, name: `${id}.txt`, kind: 'file', parent_id: 'parent', size: 10, deleted: false, inode: null,
    timestamps: { created: null, modified: null, accessed: null, birth: null, mft_changed: null } };
}
function flatPage(ids: string[], total = ids.length): FsFlatPage {
  return { items: ids.map((id, index) => ({ node: node(id), path: `/folder-${index}/${id}.txt` })), total, truncated: false };
}
function setup() {
  const scope = effectScope();
  scopes.push(scope);
  const entries = shallowRef<DiskFsNode[]>([]);
  const loading = ref(false);
  const flatMode = ref(false);
  let generation = 1;
  let signature = 'initial';
  let clientFilter = false;
  const hydrated = vi.fn();
  const beforeAppend = vi.fn();
  const directory = scope.run(() => useDirectoryQuery({
    imageId: () => 'disk-a', parentId: () => 'parent', generation: () => generation,
    enabled: () => !flatMode.value, query: () => ({}), signature: () => signature,
    clientFilter: () => clientFilter, entries, loading, hydrated,
    firstPageLoaded: vi.fn().mockResolvedValue(undefined), beforeAppend, wantedThrough: () => 0,
  }))!;
  const flat = scope.run(() => useFlatFileQuery({
    imageId: () => 'disk-a', parentId: () => 'parent', pathPrefix: () => '/evidence',
    generation: () => generation, enabled: () => flatMode.value,
    request: () => ({ keyword: 'needle', filter: { extQuery: 'txt' }, sort: 'size', sortdesc: true }),
    signature: () => signature, entries, loading, hydrated, beforeAppend,
  }))!;
  return { scope, entries, loading, flatMode, directory, flat, hydrated, beforeAppend,
    reload: () => { generation++; }, filter: () => { signature = 'changed'; },
    enableClientFilter: () => { clientFilter = true; } };
}
beforeEach(() => {
  vi.clearAllMocks();
  mocks.counts.mockResolvedValue({ counts: { txt: 3 }, total: 3 });
});
afterEach(() => { for (const scope of scopes.splice(0)) scope.stop(); });

describe('cursor directory queries', () => {
  it('uses one bounded first page and appends without resetting scroll', async () => {
    const state = setup();
    mocks.children.mockResolvedValueOnce({ items: Array.from({ length: 500 }, (_, i) => node(String(i))), nextCursor: 'next', total: 10_000 })
      .mockResolvedValueOnce({ items: [node('later')], nextCursor: null, total: 10_000 });
    await state.directory.loadDirectoryFirstPage('parent', 1);
    expect(mocks.children).toHaveBeenCalledOnce();
    expect(state.directory.directoryTotal.value).toBe(10_000);
    await state.directory.loadDirectoryNextPage();
    expect(state.entries.value).toHaveLength(501);
    expect(state.beforeAppend).toHaveBeenCalledOnce();
    expect(mocks.children).toHaveBeenLastCalledWith(expect.objectContaining({ cursor: 'next', pageSize: 500 }));
  });

  it('follows the cursor until a result target is found', async () => {
    const state = setup();
    mocks.children.mockResolvedValueOnce({ items: [node('other')], nextCursor: 'next', total: 2 })
      .mockResolvedValueOnce({ items: [node('target')], nextCursor: null, total: 2 });
    await state.directory.loadDirectoryFirstPage('parent', 1, { targetId: 'target' });
    expect(state.entries.value.map(entry => entry.id)).toEqual(['other', 'target']);
    expect(state.directory.directoryCursor.value).toBeNull();
  });

  it('reads the complete directory only for predicates that require client data', async () => {
    const state = setup();
    state.enableClientFilter();
    mocks.children.mockResolvedValueOnce({ items: Array.from({ length: 500 }, (_, i) => node(String(i))), nextCursor: 'next', total: 501 })
      .mockResolvedValueOnce({ items: [node('last')], nextCursor: null, total: 501 });
    await state.directory.loadDirectoryFirstPage('parent', 1);
    expect(mocks.children).toHaveBeenCalledTimes(2);
    expect(state.entries.value).toHaveLength(501);
  });

  it('rejects a non-advancing cursor instead of looping forever', async () => {
    const state = setup();
    state.enableClientFilter();
    mocks.children.mockResolvedValueOnce({ items: [node('first')], nextCursor: 'stuck', total: 2 })
      .mockResolvedValueOnce({ items: [], nextCursor: 'stuck', total: 2 });
    await expect(state.directory.loadDirectoryFirstPage('parent', 1)).rejects.toThrow('目录分页未前进');
    expect(state.entries.value).toEqual([]);
  });

  it.each(['reload', 'filter', 'dispose'] as const)('discards a late page after %s', async reason => {
    const state = setup();
    const response = deferred<Page<DiskFsNode>>();
    mocks.children.mockReturnValueOnce(response.promise);
    const operation = state.directory.loadDirectoryFirstPage('parent', 1);
    if (reason === 'reload') state.reload();
    else if (reason === 'filter') state.filter();
    else state.scope.stop();
    response.resolve({ items: [node('old')], nextCursor: null, total: 1 });
    await operation;
    expect(state.entries.value).toEqual([]);
    expect(state.hydrated).not.toHaveBeenCalled();
  });
});

describe('recursive server queries', () => {
  it('preserves hardlink row identity, source paths and server filter/sort', async () => {
    const state = setup(); state.flatMode.value = true;
    mocks.flat.mockResolvedValueOnce(flatPage(['same', 'same']));
    await state.flat.loadFlatFirstPage('parent', 1);
    expect(state.entries.value.map(entry => [entry.id, entry.rowKey, entry.path])).toEqual([
      ['same', 'flat:0:same', '/evidence/folder-0/same.txt'],
      ['same', 'flat:1:same', '/evidence/folder-1/same.txt'],
    ]);
    expect(mocks.flat).toHaveBeenCalledWith({ imageId: 'disk-a', parentId: 'parent', deepsearch: true,
      keyword: 'needle', filter: { extQuery: 'txt' }, sort: 'size', sortdesc: true,
      skip: 0, limit: 100, refresh: false });
  });

  it('serializes concurrent scroll demands and catches up with bounded larger pages', async () => {
    const state = setup(); state.flatMode.value = true;
    const next = deferred<FsFlatPage>();
    mocks.flat.mockResolvedValueOnce(flatPage(['first'], 5000)).mockReturnValueOnce(next.promise)
      .mockResolvedValueOnce(flatPage(Array.from({ length: 2000 }, (_, i) => `catch-${i}`), 5000));
    await state.flat.loadFlatFirstPage('parent', 1);
    const operation = state.flat.ensureFlatLoadedThrough(1);
    await state.flat.ensureFlatLoadedThrough(2000);
    expect(mocks.flat).toHaveBeenCalledTimes(2);
    next.resolve(flatPage(Array.from({ length: 100 }, (_, i) => `next-${i}`), 5000));
    await operation;
    expect(mocks.flat).toHaveBeenCalledTimes(3);
    expect(mocks.flat).toHaveBeenLastCalledWith(expect.objectContaining({ skip: 101, limit: 2000 }));
    expect(state.entries.value).toHaveLength(2101);
    expect(state.beforeAppend).toHaveBeenCalledTimes(2);
  });

  it('ignores an in-flight append when filters change before the debounce reload', async () => {
    const state = setup(); state.flatMode.value = true;
    const next = deferred<FsFlatPage>();
    mocks.flat.mockResolvedValueOnce(flatPage(['first'], 10)).mockReturnValueOnce(next.promise);
    await state.flat.loadFlatFirstPage('parent', 1);
    const operation = state.flat.ensureFlatLoadedThrough(1);
    state.filter();
    next.resolve(flatPage(['stale'], 10));
    await operation;
    expect(state.entries.value.map(entry => entry.id)).toEqual(['first']);
    expect(mocks.flat).toHaveBeenCalledTimes(2);
  });

  it.each(['reload', 'filter', 'dispose'] as const)('discards a late first page after %s', async reason => {
    const state = setup(); state.flatMode.value = true;
    const response = deferred<FsFlatPage>();
    mocks.flat.mockReturnValueOnce(response.promise);
    const operation = state.flat.loadFlatFirstPage('parent', 1);
    if (reason === 'reload') state.reload();
    else if (reason === 'filter') state.filter();
    else state.scope.stop();
    response.resolve(flatPage(['old']));
    await operation;
    expect(state.entries.value).toEqual([]);
    expect(state.hydrated).not.toHaveBeenCalled();
  });

  it('consumes refresh once and refreshes cached category counts with it', async () => {
    const state = setup(); state.flatMode.value = true;
    mocks.flat.mockResolvedValue(flatPage(['first']));
    await state.flat.loadFlatFirstPage('parent', 1);
    await state.flat.loadFlatFirstPage('parent', 1);
    expect(mocks.counts).toHaveBeenCalledOnce();
    state.flat.requestRefresh();
    await state.flat.loadFlatFirstPage('parent', 1);
    await state.flat.loadFlatFirstPage('parent', 1);
    expect(mocks.flat.mock.calls.map(([request]) => request.refresh)).toEqual([false, false, true, false]);
    expect(mocks.counts).toHaveBeenCalledTimes(2);
  });
});
