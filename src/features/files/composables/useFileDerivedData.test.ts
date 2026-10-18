import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { effectScope } from 'vue';
import type { DiskFsNode } from '../model/model';
import type { FsTypeIdent } from '../../../lib/ipc/fs/derived';
import { useFileDerivedData } from './useFileDerivedData';

const mocks = vi.hoisted(() => ({ digests: vi.fn(), types: vi.fn(), persistDigest: vi.fn(), persistType: vi.fn(), error: vi.fn() }));
vi.mock('../../../lib/ipc/fs/derived', () => ({ fsDigestLoadNodes: mocks.digests, fsFtypeLoadNodes: mocks.types,
  fsDigestPersist: mocks.persistDigest, fsFtypePersist: mocks.persistType }));
vi.mock('../../../composables/useToast', () => ({ notifyError: mocks.error }));

const scopes: ReturnType<typeof effectScope>[] = [];
const file = (id: string) => ({ id, kind: 'file' } as DiskFsNode);
const ident = (label: string): FsTypeIdent => ({ label, mime_type: 'text/plain', description: label, extensions: ['txt'], inferred: false, score: 1 });
function setup() {
  const scope = effectScope(); scopes.push(scope);
  const reveal = vi.fn();
  const data = scope.run(() => useFileDerivedData({ imageId: () => 'disk-a', queryGeneration: () => 1, revealTypeColumn: reveal }))!;
  return { data, reveal, scope };
}
beforeEach(() => {
  vi.clearAllMocks();
  mocks.digests.mockResolvedValue({}); mocks.types.mockResolvedValue({});
  mocks.persistDigest.mockResolvedValue(false); mocks.persistType.mockResolvedValue(false);
});
afterEach(() => { for (const scope of scopes.splice(0)) scope.stop(); });

describe('derived column hydration', () => {
  it('chunks unique file identities and skips already hydrated nodes and directories', async () => {
    const { data } = setup();
    const nodes = Array.from({ length: 5001 }, (_, index) => file(String(index)));
    nodes.push(file('0'), { id: 'directory', kind: 'dir' } as DiskFsNode);
    await data.hydrateDigests(nodes, 1);
    expect(mocks.digests.mock.calls.map(([args]) => [args.imageId, args.nodeIds.length])).toEqual([['disk-a', 5000], ['disk-a', 1]]);
    await data.hydrateDigests(nodes, 1);
    expect(mocks.digests).toHaveBeenCalledTimes(2);
  });

  it('fills missing hash algorithms while keeping newer session results', async () => {
    const { data } = setup();
    data.digestCache.set('file', { md5: 'fresh' });
    mocks.digests.mockResolvedValueOnce({ file: { md5: 'stored', sha256: 'stored-sha' } });
    await data.hydrateDigests([file('file')], 1);
    expect(data.digestCache.get('file')).toEqual({ md5: 'fresh', sha256: 'stored-sha' });
  });

  it('keeps a fresh type and reveals the column for additional archived types', async () => {
    const { data, reveal } = setup();
    data.ftypeCache.set('file', ident('fresh'));
    mocks.types.mockResolvedValueOnce({ file: ident('stored'), other: ident('other') });
    await data.hydrateFtypes([file('file'), file('other')], 1);
    expect([...data.ftypeCache]).toEqual([['file', ident('fresh')], ['other', ident('other')]]);
    expect(reveal).toHaveBeenCalledOnce();
  });

  it.each(['image reset', 'case hydration invalidation', 'dispose'] as const)('ignores archived data after %s', async reason => {
    const { data, scope, reveal } = setup();
    let finish!: (value: unknown) => void;
    mocks.types.mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
    const operation = data.hydrateFtypes([file('file')], 1);
    if (reason === 'image reset') data.reset();
    else if (reason === 'case hydration invalidation') data.clearHydration();
    else scope.stop();
    finish({ file: ident('old') }); await operation;
    expect([...data.ftypeCache]).toEqual([]);
    expect(reveal).not.toHaveBeenCalled();
  });

  it('reports real write failures and treats the no-case false result as a no-op', async () => {
    const { data } = setup();
    data.persistDigests('original-disk', { file: { md5: 'value' } });
    await Promise.resolve();
    expect(mocks.error).not.toHaveBeenCalled();
    mocks.persistType.mockRejectedValueOnce(new Error('disk full'));
    data.persistFtypes('original-disk', { file: ident('text') });
    await Promise.resolve();
    expect(mocks.persistType).toHaveBeenCalledWith({ imageId: 'original-disk', entries: { file: ident('text') } });
    expect(mocks.error).toHaveBeenCalledWith('保存类型识别结果失败', 'Error: disk full');
  });
});
