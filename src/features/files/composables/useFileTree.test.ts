// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { effectScope, nextTick, ref, type EffectScope } from 'vue';
import { useFileTree } from './useFileTree';
import type { FsNode, Page } from '../../../lib/ipc/fs/types';
const calls = vi.hoisted(() => ({ children: vi.fn(), page: vi.fn(), counts: vi.fn(), empty: vi.fn(), letters: vi.fn(), error: vi.fn() }));
vi.mock('./containerReads', () => ({ containerFsChildren: calls.children, containerFsChildrenPage: calls.page }));
vi.mock('../../../lib/ipc/fs/query', () => ({ fsChildCounts: calls.counts, fsSubtreeEmptyDirs: calls.empty }));
vi.mock('../../../lib/ipc/filesystemMetadata', () => ({ winDriveLetters: calls.letters }));
vi.mock('../../../composables/useToast', () => ({ notifyError: calls.error }));
const entry = (id: string, kind: FsNode['kind'] = 'dir'): FsNode => ({ id, name: id, kind, deleted: false } as FsNode);
const page = (items: FsNode[], total = items.length, nextCursor: string | null = null): Page<FsNode> => ({ items, nextCursor, total, truncated: false });
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; }
let scope: EffectScope; const image = ref('a'); const path = ref([{ id: 'root', name: '/' }]); const hide = ref(false);
const navigate = vi.fn(); const open = vi.fn();
function setup() { scope = effectScope(); return scope.run(() => useFileTree({ imageId: () => image.value, pathStack: () => path.value, caseId: () => null, hideEmptyDirs: hide, navigate, openFile: open }))!; }
async function settle() { for (let index = 0; index < 15; index++) await nextTick(); }
/** Expansion pages carry a `query`; count probes ask for one row and read `total`. */
function routePages(listing: (parentId: string) => FsNode[], probe: () => Promise<unknown> = async () => page([], 1)) {
  calls.page.mockImplementation(async (args: { parentId: string; query?: unknown }) => args.query ? page(listing(args.parentId)) : probe());
}
const countProbes = () => calls.page.mock.calls.filter(([args]) => !args.query);
beforeEach(() => {
  vi.resetAllMocks(); image.value = 'a'; path.value = [{ id: 'root', name: '/' }]; hide.value = false;
  calls.children.mockImplementation(async ({ parentId }: { parentId: string }) => parentId === 'root' ? [entry('part:1')] : []);
  routePages(() => []);
  calls.counts.mockImplementation(async ({ parentIds }: { parentIds: string[] }) => Object.fromEntries(parentIds.map(id => [id, 1])));
  calls.empty.mockResolvedValue({}); calls.letters.mockResolvedValue({ assignments: [], ambiguous: false });
});
afterEach(() => scope?.stop());
it('keeps directory expansion bounded and sends the overflow leaf to the parent listing', async () => {
  routePages(() => Array.from({ length: 301 }, (_, index) => entry(`file:${index}`, 'file')));
  const tree = setup(); await settle(); await tree.toggleExpand(tree.visibleRows.value[0].node);
  expect(tree.visibleRows.value).toHaveLength(302);
  const last = tree.visibleRows.value.at(-1)!.node; expect(last.name).toContain('其余 1 项'); tree.onRowClick(last);
  expect(navigate).toHaveBeenCalledExactlyOnceWith([{ id: 'root', name: '/' }, { id: 'part:1', name: 'part:1' }]); expect(open).not.toHaveBeenCalled();
});
it('reads only the first page of a huge directory and lets the backend drop deleted entries', async () => {
  const items = [entry('dir:a'), entry('dir:b'), ...Array.from({ length: 498 }, (_, index) => entry(`file:${index}`, 'file'))];
  calls.page.mockResolvedValue(page(items, 100_000, 'next'));
  const tree = setup(); await settle(); await tree.toggleExpand(tree.visibleRows.value[0].node);
  expect(calls.page).toHaveBeenCalledExactlyOnceWith({ imageId: 'a', parentId: 'part:1', cursor: null, pageSize: 500, query: { filter: { deleted: false } } });
  const rows = tree.visibleRows.value.map(row => row.node.id);
  expect(rows.slice(0, 3)).toEqual(['part:1', 'dir:a', 'dir:b']); expect(rows).toHaveLength(1 + 2 + 300 + 1);
  expect(tree.visibleRows.value.at(-1)!.node.name).toContain('其余 99698 项');
  expect(calls.children).toHaveBeenCalledExactlyOnceWith({ imageId: 'a', parentId: 'root' });
});
it('does not restore an old A→B→A root and drops old drive-letter probes', async () => {
  const old = deferred<FsNode[]>(); const letters = deferred<{ assignments: { partition_id: number; drive_letter: string }[]; ambiguous: boolean }>();
  calls.children.mockReturnValueOnce(old.promise); calls.letters.mockReturnValueOnce(letters.promise);
  const tree = setup(); image.value = 'b'; await settle(); image.value = 'a'; await settle();
  old.resolve([entry('old')]); letters.resolve({ assignments: [{ partition_id: 1, drive_letter: 'Z:' }], ambiguous: false }); await settle();
  expect(tree.visibleRows.value.map(row => row.node.id)).toEqual(['part:1']); expect(tree.visibleRows.value[0].node.driveLetter).toBeNull();
});
it('stops a pending path expansion after disposal', async () => {
  const children = deferred<Page<FsNode>>(); calls.page.mockReturnValue(children.promise);
  const tree = setup(); await settle(); path.value = [{ id: 'root', name: '/' }, { id: 'part:1', name: 'part:1' }]; await settle();
  scope.stop(); children.resolve(page([entry('late')])); await settle();
  expect(tree.visibleRows.value.map(row => row.node.id)).toEqual(['part:1']); expect(tree.visibleRows.value[0].node.expanded).toBe(false);
});

it('counts only viewport directories instead of probing every child of a large directory', async () => {
  routePages(() => Array.from({ length: 23_536 }, (_, index) => entry(`dir:${index}`)));
  calls.counts.mockResolvedValue({});
  const tree = setup(); await settle();
  await tree.toggleExpand(tree.visibleRows.value[0].node);
  expect(tree.visibleRows.value).toHaveLength(23_537);
  expect(countProbes()).toHaveLength(0);
  const viewport = tree.visibleRows.value.slice(100, 120);
  tree.hydrateRows(viewport); await settle();
  expect(countProbes()).toHaveLength(20);
  expect(countProbes().every(([args]) => args.pageSize === 1)).toBe(true);
  expect(calls.children).toHaveBeenCalledTimes(1);
  tree.hydrateRows(viewport); await settle();
  expect(countProbes()).toHaveLength(20);
});

it('replaces queued offscreen count probes and does not retry failed probes in a loop', async () => {
  routePages(() => Array.from({ length: 100 }, (_, index) => entry(`dir:${index}`)), async () => { throw new Error('unreadable directory'); });
  const pending = deferred<Record<string, number>>();
  calls.counts.mockReturnValueOnce(pending.promise).mockResolvedValue({});
  const tree = setup(); await settle();
  await tree.toggleExpand(tree.visibleRows.value[0].node);
  tree.hydrateRows(tree.visibleRows.value.slice(1, 3)); await settle();
  tree.hydrateRows(tree.visibleRows.value.slice(20, 23));
  tree.hydrateRows(tree.visibleRows.value.slice(90, 93));
  pending.resolve({}); await settle();
  expect(calls.counts.mock.calls.map(([args]) => args.parentIds)).toEqual([
    ['dir:0', 'dir:1'], ['dir:89', 'dir:90', 'dir:91'],
  ]);
  expect(countProbes()).toHaveLength(5);
  tree.hydrateRows(tree.visibleRows.value.slice(90, 93)); await settle();
  expect(countProbes()).toHaveLength(5);
});

it('starts new-image count probes without waiting for an old image request', async () => {
  const pending = deferred<Record<string, number>>();
  calls.counts.mockReturnValueOnce(pending.promise);
  const tree = setup(); await settle();
  tree.hydrateRows(tree.visibleRows.value); await settle();
  image.value = 'b'; await settle();
  tree.hydrateRows(tree.visibleRows.value); await settle();
  expect(calls.counts.mock.calls.map(([args]) => args.imageId)).toEqual(['a', 'b']);
  pending.resolve({ 'part:1': 99 }); await settle();
  expect(tree.visibleRows.value[0].node.childCount).toBe(1);
});

it('uses snapshot empty-subtree classification before requesting live counts for filtering', async () => {
  routePages(parentId => parentId === 'part:1' ? [entry('empty'), entry('nonempty')] : []);
  calls.empty.mockResolvedValue({ empty: true, nonempty: false });
  calls.counts.mockResolvedValue({});
  const tree = setup(); await settle();
  await tree.toggleExpand(tree.visibleRows.value[0].node);
  hide.value = true; await settle();
  expect(tree.visibleRows.value.map(row => row.node.id)).toEqual(['part:1', 'nonempty']);
  expect(countProbes()).toHaveLength(0);
});
