import { afterEach, describe, expect, it, vi } from 'vitest';
import { effectScope, nextTick, ref } from 'vue';
import type { FsSizeTreeResult, FsSizeTreeView } from '../../../lib/ipc/fs/sizeTree';
import { useSizeTree } from './useSizeTree';

const mocks = vi.hoisted(() => ({ sizeTree: vi.fn() }));
vi.mock('../../../lib/ipc/fs/sizeTree', () => ({ fsSizeTree: mocks.sizeTree }));

const scopes: ReturnType<typeof effectScope>[] = [];
afterEach(() => {
  scopes.splice(0).forEach(scope => scope.stop());
  vi.resetAllMocks();
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(finish => { resolve = finish; });
  return { promise, resolve };
}

/** A view tagged with the directory it was asked for. */
function ready(nodeId: string): FsSizeTreeResult {
  const view: FsSizeTreeView = {
    root: { node: null, bytes: 1, files: 1, dirs: 0, newest: null, children: [], others: null },
    truncated: false,
    newestWrite: null,
    builtAt: nodeId,
  };
  return { status: 'ready', view };
}

function setup() {
  const scope = effectScope();
  scopes.push(scope);
  const nodeId = ref('a');
  const tree = scope.run(() => useSizeTree({
    imageId: () => 'disk-a', nodeId: () => nodeId.value, enabled: () => true,
    mode: () => 'bytes', depth: () => 3, includeDeleted: () => false, snapshotSignature: () => 'ready',
  }))!;
  return { tree, nodeId };
}

const requested = () => mocks.sizeTree.mock.calls.map(([args]) => (args as { nodeId: string }).nodeId);
async function settle() {
  for (let index = 0; index < 6; index++) await nextTick();
}

describe('useSizeTree prefetch', () => {
  it('serves a directory it fetched ahead without asking again', async () => {
    mocks.sizeTree.mockImplementation(async ({ nodeId }: { nodeId: string }) => ready(nodeId));
    const { tree, nodeId } = setup();
    await settle();
    tree.prefetch('b');
    await settle();
    expect(requested()).toEqual(['a', 'b']);
    expect(tree.view.value?.builtAt).toBe('a');

    nodeId.value = 'b';
    await settle();
    expect(tree.view.value?.builtAt).toBe('b');
    expect(tree.status.value).toBe('ready');
    expect(requested()).toEqual(['a', 'b']);
  });

  it('joins a fetch still on its way instead of asking twice', async () => {
    const later = deferred<FsSizeTreeResult>();
    mocks.sizeTree.mockImplementation(({ nodeId }: { nodeId: string }) => (nodeId === 'b' ? later.promise : Promise.resolve(ready(nodeId))));
    const { tree, nodeId } = setup();
    await settle();
    tree.prefetch('b');
    nodeId.value = 'b';
    await settle();
    expect(tree.status.value).toBe('loading');
    later.resolve(ready('b'));
    await settle();
    expect(tree.view.value?.builtAt).toBe('b');
    expect(requested()).toEqual(['a', 'b']);
  });

  it('fetches one directory ahead at a time, then only the latest wanted', async () => {
    const first = deferred<FsSizeTreeResult>();
    mocks.sizeTree.mockImplementation(({ nodeId }: { nodeId: string }) => (nodeId === 'b' ? first.promise : Promise.resolve(ready(nodeId))));
    const { tree } = setup();
    await settle();
    tree.prefetch('b');
    tree.prefetch('c');
    tree.prefetch('d');
    expect(requested()).toEqual(['a', 'b']);
    first.resolve(ready('b'));
    await settle();
    expect(requested()).toEqual(['a', 'b', 'd']);
  });

  it('drops what arrives after the evidence changed', async () => {
    const stale = deferred<FsSizeTreeResult>();
    mocks.sizeTree.mockImplementation(({ nodeId }: { nodeId: string }) => (nodeId === 'b' && requested().length === 2 ? stale.promise : Promise.resolve(ready(nodeId))));
    const { tree, nodeId } = setup();
    await settle();
    tree.prefetch('b');
    tree.reset();
    stale.resolve(ready('b'));
    await settle();
    nodeId.value = 'b';
    await settle();
    expect(requested()).toEqual(['a', 'b', 'b']);
    expect(tree.view.value?.builtAt).toBe('b');
  });
});
