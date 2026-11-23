// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { effectScope, nextTick, ref, type EffectScope } from 'vue';
import { useCarvePreview } from './useCarvePreview';
import type { CarveArtifact, CarveScanSnapshot } from '../../../lib/ipc/carve';
const read = vi.hoisted(() => vi.fn());
vi.mock('../../../lib/ipc/carve', () => ({ carvePreview: read }));
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; }
let scope: EffectScope;
const image = ref('a'); const scan = ref({ scan_id: 'scan' } as CarveScanSnapshot);
const artifact = ref<CarveArtifact | null>(null);
const picture = { id: 'file', category: 'image', extension: 'jpg', length: 100 } as CarveArtifact;
const create = vi.fn(() => 'blob:preview'); const revoke = vi.fn();
async function settle() { for (let index = 0; index < 8; index++) await nextTick(); }
function setup() { scope = effectScope(); return scope.run(() => useCarvePreview({ imageId: () => image.value, scan: () => scan.value, artifact: () => artifact.value }))!; }
beforeEach(() => { vi.resetAllMocks(); image.value = 'a'; artifact.value = null; scan.value = { scan_id: 'scan' } as CarveScanSnapshot; vi.stubGlobal('URL', { createObjectURL: create, revokeObjectURL: revoke }); create.mockReturnValue('blob:preview'); });
afterEach(() => { scope?.stop(); vi.unstubAllGlobals(); });
it('rejects a same-artifact A→B→A response before creating an object URL', async () => {
  const old = deferred<Uint8Array>(); const fresh = deferred<Uint8Array>();
  read.mockReturnValueOnce(old.promise).mockResolvedValueOnce(new Uint8Array([2])).mockReturnValueOnce(fresh.promise);
  const preview = setup(); artifact.value = picture; image.value = 'b'; image.value = 'a';
  old.resolve(new Uint8Array([1])); await settle(); expect(create).not.toHaveBeenCalled(); expect(preview.previewLoading.value).toBe(true);
  fresh.resolve(new Uint8Array([3])); await settle(); expect(create).toHaveBeenCalledOnce(); expect(preview.previewUrl.value).toBe('blob:preview');
});
it('releases the current URL and ignores a response after disposal', async () => {
  read.mockResolvedValueOnce(new Uint8Array([1])); const preview = setup(); artifact.value = picture; await settle();
  const late = deferred<Uint8Array>(); read.mockReturnValueOnce(late.promise); artifact.value = { ...picture, id: 'next' }; scope.stop();
  late.resolve(new Uint8Array([2])); await settle(); expect(create).toHaveBeenCalledOnce(); expect(revoke).toHaveBeenCalledExactlyOnceWith('blob:preview'); expect(preview.previewUrl.value).toBeNull();
});
it('keeps non-images and oversized images out of the member read path', async () => {
  const preview = setup(); artifact.value = { ...picture, length: 8 * 1024 ** 2 + 1 };
  expect(preview.previewMessage.value).toContain('8 MB'); artifact.value = { ...picture, category: 'document' };
  expect(preview.previewMessage.value).toBe('此格式暂不提供内嵌渲染'); expect(read).not.toHaveBeenCalled();
});
