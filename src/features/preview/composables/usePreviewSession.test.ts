import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { effectScope, ref } from 'vue';
import { usePreviewSession } from './usePreviewSession';
import type { FsNode } from '../../../lib/ipc/fs/types';
import type { DetailTab } from '../model/preview';

const mocks = vi.hoisted(() => ({ preview: vi.fn(), identify: vi.fn(), error: vi.fn() }));
vi.mock('../../../lib/ipc/fs/read', () => ({ fsPreview: mocks.preview }));
vi.mock('../../../lib/ipc/fs/derived', () => ({ fsIdentifyType: mocks.identify }));
vi.mock('../../../composables/useToast', () => ({ notifyError: mocks.error }));
const scopes: ReturnType<typeof effectScope>[] = [];
const file = (id: string, size = 4, kind = 'file') => ({ id, size, kind } as FsNode);
const ident = { label: 'text', mime_type: 'text/plain' };
function deferred<T>() { let resolve!: (value: T) => void, reject!: (reason: unknown) => void; const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }
function setup(indexOnly = false) {
  const scope = effectScope(); scopes.push(scope);
  const image = ref('image-a'), node = ref(file('file-a')), activeTab = ref<DetailTab>('detail'), page = ref(3);
  const reset = vi.fn(), resetMedia = vi.fn(), media = vi.fn(), decode = vi.fn(async () => {}), identified = vi.fn();
  const session = scope.run(() => usePreviewSession({ indexOnly: () => indexOnly, imageId: () => image.value, node: () => node.value, activeTab, page,
    category: () => 'text', resetForNode: reset, resetMedia, loadNativeMedia: media, ensureDecodedText: decode, identified }))!;
  return { session, image, node, activeTab, page, reset, resetMedia, media, decode, identified, scope };
}
beforeEach(() => { vi.clearAllMocks(); mocks.preview.mockResolvedValue(new Uint8Array([1]).buffer); mocks.identify.mockResolvedValue(ident); });
afterEach(() => { for (const scope of scopes.splice(0)) scope.stop(); });

describe('content preview session', () => {
  it('opens offline metadata and does not request bytes, media, decoding or identification', async () => {
    const state = setup(true); state.activeTab.value = 'hex';
    await state.session.loadPreview(); await state.session.identifyType();
    expect(state.activeTab.value).toBe('detail'); expect(state.session.loading.value).toBe(false);
    expect(state.session.error.value).toContain('源镜像不可用');
    expect(mocks.preview).not.toHaveBeenCalled(); expect(mocks.identify).not.toHaveBeenCalled();
    expect(state.media).not.toHaveBeenCalled(); expect(state.decode).not.toHaveBeenCalled();
    state.node.value = file('file-b'); await state.session.loadPreview();
    expect(state.activeTab.value).toBe('detail');
  });
  it('keeps the file view preference when switching files and visiting a directory', async () => {
    const state = setup(); await state.session.loadPreview();
    state.activeTab.value = 'hex';
    state.node.value = file('file-b'); await state.session.loadPreview();
    expect(state.activeTab.value).toBe('hex');
    state.node.value = file('dir', 0, 'dir'); await state.session.loadPreview();
    expect(state.activeTab.value).toBe('detail');
    state.node.value = file('file-c'); await state.session.loadPreview();
    expect(state.activeTab.value).toBe('hex');
  });
  it('resets local state once per file and keeps the existing capped prefix command', async () => {
    const state = setup(); await state.session.loadPreview();
    expect(mocks.preview).toHaveBeenCalledWith({ imageId: 'image-a', nodeId: 'file-a', maxBytes: 1024 * 1024 });
    expect(state.activeTab.value).toBe('raw'); expect(state.page.value).toBe(1);
    expect(state.reset).toHaveBeenCalledOnce(); expect(state.media).toHaveBeenCalledOnce(); expect(state.decode).toHaveBeenCalledOnce();
    expect(state.session.bytes.value).toEqual(new Uint8Array([1])); expect(state.session.loading.value).toBe(false);
  });
  it('drops old prefix bytes after A→B→A even when the node id matches again', async () => {
    const old = deferred<ArrayBuffer>(), recent = deferred<ArrayBuffer>();
    mocks.preview.mockReturnValueOnce(old.promise).mockReturnValueOnce(Promise.resolve(new Uint8Array([2]).buffer)).mockReturnValueOnce(recent.promise);
    const state = setup(); const first = state.session.loadPreview();
    state.node.value = file('file-b'); await state.session.loadPreview();
    state.node.value = file('file-a'); const third = state.session.loadPreview();
    old.resolve(new Uint8Array([9]).buffer); await first;
    expect(state.session.bytes.value.length).toBe(0); expect(state.session.loading.value).toBe(true);
    recent.resolve(new Uint8Array([3]).buffer); await third;
    expect(state.session.bytes.value).toEqual(new Uint8Array([3]));
  });
  it('drops old responses when changing evidence with the same node id', async () => {
    const old = deferred<ArrayBuffer>(); mocks.preview.mockReturnValueOnce(old.promise);
    const state = setup(); const first = state.session.loadPreview();
    state.image.value = 'image-b'; await state.session.loadPreview();
    old.reject(new Error('old image failed')); await first;
    expect(state.session.error.value).toBeNull(); expect(state.session.bytes.value).toEqual(new Uint8Array([1]));
  });
  it('settles loading/error on a directory or empty file without starting a read', async () => {
    const old = deferred<ArrayBuffer>(); mocks.preview.mockReturnValueOnce(old.promise);
    const state = setup(); const first = state.session.loadPreview();
    state.node.value = file('directory', 0, 'dir'); await state.session.loadPreview();
    expect(state.activeTab.value).toBe('detail'); expect(state.session.loading.value).toBe(false);
    state.node.value = file('empty', 0); await state.session.loadPreview();
    expect(state.activeTab.value).toBe('raw'); expect(mocks.preview).toHaveBeenCalledOnce();
    old.reject(new Error('old failed')); await first;
    expect(state.session.error.value).toBeNull();
  });
  it('allows new-file identification while the old operation is still pending', async () => {
    const old = deferred<typeof ident>(); mocks.identify.mockReturnValueOnce(old.promise);
    const state = setup(); await state.session.loadPreview(); const first = state.session.identifyType();
    state.node.value = file('file-b'); await state.session.loadPreview(); await state.session.identifyType();
    old.resolve({ label: 'old', mime_type: 'image/jpeg' }); await first;
    expect(state.identified.mock.calls).toEqual([[{ nodeId: 'file-b', ident }]]);
    expect(state.session.localIdent.value).toEqual(ident); expect(state.session.identifying.value).toBe(false);
    expect(state.resetMedia).toHaveBeenCalledOnce();
  });
  it('reports an active identification error and ignores late failures after disposal', async () => {
    const state = setup(); mocks.identify.mockRejectedValueOnce(new Error('active failure'));
    await state.session.identifyType(); expect(mocks.error).toHaveBeenCalledWith('类型识别失败', 'Error: active failure');
    const old = deferred<typeof ident>(); mocks.identify.mockReturnValueOnce(old.promise);
    const operation = state.session.identifyType(); state.scope.stop(); old.reject(new Error('disposed')); await operation;
    expect(mocks.error).toHaveBeenCalledOnce(); expect(state.identified).not.toHaveBeenCalled();
  });
  it('ignores a successful prefix read after its owner is disposed', async () => {
    const old = deferred<ArrayBuffer>(); mocks.preview.mockReturnValueOnce(old.promise);
    const state = setup(); const operation = state.session.loadPreview(); state.scope.stop();
    old.resolve(new Uint8Array([9]).buffer); await operation;
    expect(state.session.bytes.value.length).toBe(0); expect(state.decode).not.toHaveBeenCalled();
  });
});
