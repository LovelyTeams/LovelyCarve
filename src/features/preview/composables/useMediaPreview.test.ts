// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { effectScope, ref } from 'vue';
import { useMediaPreview } from './useMediaPreview';

const mocks = vi.hoisted(() => ({ read: vi.fn(), materialize: vi.fn(), release: vi.fn(), cancel: vi.fn(), createUrl: vi.fn(), revokeUrl: vi.fn(), task: 0 }));
vi.mock('../../../lib/ipc/fs/read', () => ({ fsRead: mocks.read }));
vi.mock('../../../lib/ipc/taskRuntime', () => ({ taskCancel: mocks.cancel }));
vi.mock('../../../lib/ipc/media', () => ({ fsMediaTemp: mocks.materialize, fsMediaRelease: mocks.release,
  mediaStreamUrl: (path: string) => `asset:${path}`, newPreviewTaskId: () => `preview-${++mocks.task}` }));
const scopes: ReturnType<typeof effectScope>[] = [];
function deferred<T>() { let resolve!: (value: T) => void, reject!: (reason: unknown) => void; const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }
function setup() {
  const scope = effectScope(); scopes.push(scope);
  const image = ref('image-a'), node = ref('file-a'), size = ref(4), category = ref('image'), mime = ref('image/png');
  const media = scope.run(() => useMediaPreview({ imageId: () => image.value, nodeId: () => node.value, nodeSize: () => size.value,
    category: () => category.value, mime: () => mime.value, extension: () => 'disguised',
    isBlobMedia: () => category.value === 'image', isStreamMedia: () => category.value === 'video' || category.value === 'audio' }))!;
  return { media, image, node, size, category, mime, scope };
}
beforeEach(() => {
  vi.clearAllMocks(); mocks.task = 0;
  mocks.read.mockResolvedValue(new Uint8Array([1]).buffer); mocks.materialize.mockResolvedValue('cached.mp4');
  mocks.release.mockResolvedValue(undefined); mocks.cancel.mockResolvedValue(undefined); mocks.createUrl.mockReturnValue('blob:preview');
  vi.stubGlobal('URL', { createObjectURL: mocks.createUrl, revokeObjectURL: mocks.revokeUrl });
});
afterEach(() => { for (const scope of scopes.splice(0)) scope.stop(); vi.unstubAllGlobals(); });

describe('media preview lease owner', () => {
  it('deduplicates image reads and revokes its Blob URL on reset/disposal', async () => {
    const state = setup(); await Promise.all([state.media.loadMedia(), state.media.loadMedia()]);
    expect(mocks.read).toHaveBeenCalledOnce(); expect(mocks.createUrl).toHaveBeenCalledOnce();
    expect(mocks.createUrl.mock.calls[0][0].type).toBe('image/png');
    state.media.resetMedia(); expect(mocks.revokeUrl).toHaveBeenCalledWith('blob:preview');
    await state.media.loadMedia(); state.scope.stop(); expect(mocks.revokeUrl).toHaveBeenCalledTimes(2);
  });
  it('does not fetch empty/oversized images or create a URL after disposal', async () => {
    const state = setup(); state.size.value = 0; await state.media.loadMedia();
    state.size.value = 16 * 1024 * 1024 + 1; await state.media.loadMedia(); expect(mocks.read).not.toHaveBeenCalled();
    state.size.value = 4; const old = deferred<ArrayBuffer>(); mocks.read.mockReturnValueOnce(old.promise);
    const operation = state.media.loadMedia(); state.scope.stop(); old.resolve(new Uint8Array([1]).buffer); await operation;
    expect(mocks.createUrl).not.toHaveBeenCalled();
  });
  it('drops old image bytes after A→B→A and keeps the new request loading', async () => {
    const old = deferred<ArrayBuffer>(), recent = deferred<ArrayBuffer>();
    mocks.read.mockReturnValueOnce(old.promise).mockReturnValueOnce(Promise.resolve(new Uint8Array([2]).buffer)).mockReturnValueOnce(recent.promise);
    const state = setup(); const first = state.media.loadMedia();
    state.node.value = 'file-b'; state.media.resetMedia(); await state.media.loadMedia();
    state.node.value = 'file-a'; state.media.resetMedia(); const third = state.media.loadMedia();
    old.resolve(new Uint8Array([9]).buffer); await first;
    expect(state.media.mediaUrl.value).toBeNull(); expect(state.media.mediaLoading.value).toBe(true);
    recent.resolve(new Uint8Array([3]).buffer); await third;
    expect(mocks.createUrl).toHaveBeenCalledTimes(2);
  });
  it('does not publish an old evidence read that reuses the same node id', async () => {
    const old = deferred<ArrayBuffer>(); mocks.read.mockReturnValueOnce(old.promise);
    const state = setup(); const first = state.media.loadMedia(); state.image.value = 'image-b'; state.media.resetMedia();
    await state.media.loadMedia(); old.reject(new Error('old failure')); await first;
    expect(state.media.mediaError.value).toBeNull(); expect(mocks.createUrl).toHaveBeenCalledOnce();
  });
  it('uses detected MIME for materialization, deduplicates it and releases its lease', async () => {
    const state = setup(); state.category.value = 'video'; state.mime.value = 'video/mp4';
    await Promise.all([state.media.loadStreamMedia(), state.media.loadStreamMedia()]);
    expect(mocks.materialize.mock.calls).toEqual([[{ imageId: 'image-a', nodeId: 'file-a', ext: 'mp4', taskId: 'preview-1' }]]);
    expect(state.media.streamUrl.value).toBe('asset:cached.mp4');
    state.media.onMediaError(); expect(state.media.streamError.value).toBe('内置播放器无法解码此视频编码');
    state.scope.stop();
    expect(mocks.cancel).toHaveBeenCalledWith({ taskId: 'preview-1' }); expect(mocks.release).toHaveBeenCalledWith('preview-1');
  });
  it('releases materialization that completes after cancellation without publishing a URL', async () => {
    const old = deferred<string>(); mocks.materialize.mockReturnValueOnce(old.promise);
    const state = setup(); state.category.value = 'video'; const operation = state.media.loadStreamMedia();
    state.media.resetMedia(); expect(mocks.cancel).toHaveBeenCalledOnce();
    old.resolve('late.mp4'); await operation;
    expect(mocks.release.mock.calls).toEqual([['preview-1']]); expect(state.media.streamUrl.value).toBeNull();
  });
  it('does not let an old stream failure overwrite a new stream', async () => {
    const old = deferred<string>(); mocks.materialize.mockReturnValueOnce(old.promise);
    const state = setup(); state.category.value = 'video'; const first = state.media.loadStreamMedia();
    state.media.resetMedia(); await state.media.loadStreamMedia(); old.reject(new Error('late failure')); await first;
    expect(state.media.streamError.value).toBeNull(); expect(state.media.streamUrl.value).toBe('asset:cached.mp4');
  });
});
