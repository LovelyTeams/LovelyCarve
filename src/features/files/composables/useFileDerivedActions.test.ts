import { beforeEach, expect, it, vi } from 'vitest';
import type { useFileDerivedData } from './useFileDerivedData';
import type { DiskFsNode } from '../model/model';
import { useFileDerivedActions } from './useFileDerivedActions';

const mocks = vi.hoisted(() => ({ digest: vi.fn(), update: vi.fn(), dismiss: vi.fn(), broadcast: vi.fn() }));
vi.mock('../../../lib/ipc/fs/derived', () => ({ fsDigest: mocks.digest }));
vi.mock('../../../composables/useToast', () => ({ pushPersistentToast: () => ({ update: mocks.update, dismiss: mocks.dismiss }) }));
vi.mock('../../preview/api', () => ({ broadcastPreview: mocks.broadcast }));

beforeEach(() => vi.clearAllMocks());

it('reports synthetic-byte hashes without publishing bare digests to caches, previews or persistence', async () => {
  mocks.digest.mockResolvedValue({ md5: 'synthetic', read_faults_detected: true, synthetic_bytes: 116,
    bad_regions: [{ offset: 100, len: 116 }] });
  const digestCache = new Map();
  const persistDigests = vi.fn();
  const revealColumns = vi.fn();
  const data = { digestCache, ftypeCache: new Map(), persistDigests, persistFtypes: vi.fn(), generation: () => 1 } as unknown as ReturnType<typeof useFileDerivedData>;
  const actions = useFileDerivedActions({ imageId: () => 'disk', data, revealColumns });
  await actions.digestCheckedFiles([{ id: 'file', kind: 'file' } as DiskFsNode], ['md5']);
  expect(digestCache.size).toBe(0);
  expect(persistDigests).toHaveBeenCalledWith('disk', {});
  expect(mocks.broadcast).not.toHaveBeenCalled();
  expect(revealColumns).not.toHaveBeenCalled();
  expect(mocks.update).toHaveBeenLastCalledWith(expect.objectContaining({ kind: 'error', detail: expect.stringContaining('116') }));
});
