// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { effectScope, nextTick, ref, type EffectScope } from 'vue';
import { useFileTree } from './useFileTree';
import type { FsNode } from '../../../lib/ipc/fs/types';
const calls = vi.hoisted(() => ({ children: vi.fn(), counts: vi.fn(), empty: vi.fn(), letters: vi.fn(), error: vi.fn() }));
vi.mock('./containerReads', () => ({ containerFsChildren: calls.children }));
vi.mock('../../../lib/ipc/fs/query', () => ({ fsChildCounts: calls.counts, fsSubtreeEmptyDirs: calls.empty }));
vi.mock('../../../lib/ipc/filesystemMetadata', () => ({ winDriveLetters: calls.letters }));
vi.mock('../../../composables/useToast', () => ({ notifyError: calls.error }));
const entry = (id: string, kind: FsNode['kind'] = 'dir'): FsNode => ({ id, name: id, kind, deleted: false } as FsNode);
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; }
let scope: EffectScope; const image = ref('a'); const path = ref([{ id: 'root', name: '/' }]); const hide = ref(false);
const navigate = vi.fn(); const open = vi.fn();
function setup() { scope = effectScope(); return scope.run(() => useFileTree({ imageId: () => image.value, pathStack: () => path.value, caseId: () => null, hideEmptyDirs: hide, navigate, openFile: open }))!; }
async function settle() { for (let index = 0; index < 15; index++) await nextTick(); }
beforeEach(() => {
  vi.resetAllMocks(); image.value = 'a'; path.value = [{ id: 'root', name: '/' }]; hide.value = false;
  calls.children.mockImplementation(async ({ parentId }: { parentId: string }) => parentId === 'root' ? [entry('part:1')] : []);
  calls.counts.mockImplementation(async ({ parentIds }: { parentIds: string[] }) => Object.fromEntries(parentIds.map(id => [id, 1])));
  calls.empty.mockResolvedValue({}); calls.letters.mockResolvedValue({ assignments: [], ambiguous: false });
});
afterEach(() => scope?.stop());
it('keeps directory expansion bounded and sends the overflow leaf to the parent listing', async () => {
  calls.children.mockImplementation(async ({ parentId }: { parentId: string }) => parentId === 'root' ? [entry('part:1')] : Array.from({ length: 301 }, (_, index) => entry(`file:${index}`, 'file')));
  const tree = setup(); await settle(); await tree.toggleExpand(tree.visibleRows.value[0].node);
  expect(tree.visibleRows.value).toHaveLength(302);
  const last = tree.visibleRows.value.at(-1)!.node; expect(last.name).toContain('其余 1 项'); tree.onRowClick(last);
  expect(navigate).toHaveBeenCalledExactlyOnceWith([{ id: 'root', name: '/' }, { id: 'part:1', name: 'part:1' }]); expect(open).not.toHaveBeenCalled();
});
it('does not restore an old A→B→A root and drops old drive-letter probes', async () => {
  const old = deferred<FsNode[]>(); const letters = deferred<{ assignments: { partition_id: number; drive_letter: string }[]; ambiguous: boolean }>();
  calls.children.mockReturnValueOnce(old.promise); calls.letters.mockReturnValueOnce(letters.promise);
  const tree = setup(); image.value = 'b'; await settle(); image.value = 'a'; await settle();
  old.resolve([entry('old')]); letters.resolve({ assignments: [{ partition_id: 1, drive_letter: 'Z:' }], ambiguous: false }); await settle();
  expect(tree.visibleRows.value.map(row => row.node.id)).toEqual(['part:1']); expect(tree.visibleRows.value[0].node.driveLetter).toBeNull();
});
it('stops a pending path expansion after disposal', async () => {
  const children = deferred<FsNode[]>(); calls.children.mockImplementation(({ parentId }: { parentId: string }) => parentId === 'root' ? Promise.resolve([entry('part:1')]) : children.promise);
  const tree = setup(); await settle(); path.value = [{ id: 'root', name: '/' }, { id: 'part:1', name: 'part:1' }]; await settle();
  scope.stop(); children.resolve([entry('late')]); await settle();
  expect(tree.visibleRows.value.map(row => row.node.id)).toEqual(['part:1']); expect(tree.visibleRows.value[0].node.expanded).toBe(false);
});
