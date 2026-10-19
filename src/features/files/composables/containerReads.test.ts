import { beforeEach, describe, expect, it, vi } from 'vitest';
import { shallowRef } from 'vue';
import { containerFsChildren, containerFsFlatPage } from './containerReads';

const ipc = vi.hoisted(() => ({ prepare: vi.fn(), children: vi.fn(), page: vi.fn(), flat: vi.fn() }));
const activeCase = shallowRef<{ dir: string } | null>({ dir: 'case-a' });
vi.mock('../../../lib/ipc/fs/query', () => ({ fsDockerPrepare: ipc.prepare, fsChildren: ipc.children, fsChildrenPage: ipc.page, fsFlatPage: ipc.flat }));
vi.mock('../../cases/api', () => ({ useCaseSession: () => ({ activeCase }) }));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}

beforeEach(() => {
  vi.resetAllMocks();
  activeCase.value = { dir: 'case-a' };
  ipc.children.mockResolvedValue([]);
  ipc.flat.mockResolvedValue({ items: [], total: 0 });
});

describe('container reads', () => {
  it('reads outside containers directly', async () => {
    await containerFsChildren({ imageId: 'disk-a', parentId: 'host-docker:' });
    expect(ipc.prepare).not.toHaveBeenCalled();
    expect(ipc.children).toHaveBeenCalledOnce();
  });

  it('merges a container once for concurrent reads, then reads', async () => {
    const build = deferred<{ ready: boolean; persisted: boolean; entryCount: number }>();
    ipc.prepare.mockImplementation(async (args: { mode: string }) => args.mode === 'status'
      ? { ready: false, persisted: false, entryCount: 0 } : build.promise);
    const tree = containerFsChildren({ imageId: 'disk-a', parentId: 'host-docker:abc:' });
    const flat = containerFsFlatPage({ imageId: 'disk-a', parentId: 'host-docker:abc:2f657463', deepsearch: false, skip: 0, limit: 100 });
    await vi.waitFor(() => expect(ipc.prepare).toHaveBeenCalledTimes(2));
    expect(ipc.prepare).toHaveBeenLastCalledWith({ imageId: 'disk-a', rootId: 'host-docker:abc:', caseDir: 'case-a', mode: 'build' });
    expect(ipc.children).not.toHaveBeenCalled();
    build.resolve({ ready: true, persisted: true, entryCount: 3 });
    await Promise.all([tree, flat]);
    expect(ipc.prepare).toHaveBeenCalledTimes(2);
    expect(ipc.children).toHaveBeenCalledOnce();
    expect(ipc.flat).toHaveBeenCalledOnce();
  });

  it('skips the build of a merged container and fails reads the build could not finish', async () => {
    ipc.prepare.mockResolvedValueOnce({ ready: true, persisted: true, entryCount: 3 });
    await containerFsChildren({ imageId: 'disk-a', parentId: 'host-docker:abc:' });
    expect(ipc.prepare).toHaveBeenCalledOnce();
    ipc.prepare.mockResolvedValueOnce({ ready: false, persisted: false, entryCount: 0 }).mockResolvedValueOnce({ ready: false, persisted: false, entryCount: 0 });
    await expect(containerFsChildren({ imageId: 'disk-a', parentId: 'host-docker:abc:' })).rejects.toThrow('容器索引尚未完成，请重试');
  });

  it('abandons a read when the case changes during the merge', async () => {
    const status = deferred<{ ready: boolean; persisted: boolean; entryCount: number }>();
    ipc.prepare.mockReturnValueOnce(status.promise);
    const read = containerFsChildren({ imageId: 'disk-a', parentId: 'host-docker:abc:' });
    activeCase.value = { dir: 'case-b' };
    status.resolve({ ready: true, persisted: true, entryCount: 3 });
    await expect(read).rejects.toThrow('案件已切换，请重新打开容器');
    expect(ipc.children).not.toHaveBeenCalled();
  });
});
