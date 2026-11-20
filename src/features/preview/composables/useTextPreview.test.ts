import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { computed, effectScope, ref } from 'vue';
import { useTextPreview } from './useTextPreview';
import type { DetailTab, PreviewCategory } from '../model/preview';

const mocks = vi.hoisted(() => ({ detect: vi.fn(), text: vi.fn() }));
vi.mock('../../../lib/ipc/fs/read', () => ({ fsPreviewTextEncoding: mocks.detect, fsPreviewText: mocks.text }));
const scopes: ReturnType<typeof effectScope>[] = [];
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(yes => { resolve = yes; }); return { promise, resolve }; }
const guess = (encoding: string, from_bom = false, continues = false) => ({ encoding, from_bom, had_errors: false, continues });
// “你好世界” in GBK: it is not valid UTF-8, so the result shows which decoder ran.
const GBK = Uint8Array.from([0xC4, 0xE3, 0xBA, 0xC3, 0xCA, 0xC0, 0xBD, 0xE7]);
const request = { imageId: 'image-a', nodeId: 'file-a', maxBytes: 1024 * 1024 };
function setup(content: Uint8Array = GBK) {
  const scope = effectScope(); scopes.push(scope);
  const image = ref('image-a'), node = ref('file-a'), activeTab = ref<DetailTab>('text'), bytes = ref(content);
  const text = scope.run(() => useTextPreview({ imageId: () => image.value, nodeId: () => node.value, nodeKind: () => 'file', activeTab,
    category: computed<PreviewCategory>(() => 'code'), bytes, searchQuery: ref(''), previewCap: 1024 * 1024 }))!;
  return { text, image, node, activeTab, scope };
}
beforeEach(() => {
  vi.clearAllMocks();
  mocks.detect.mockResolvedValue(guess('GBK'));
  mocks.text.mockResolvedValue({ ...guess('x-unknown'), text: 'backend text' });
});
afterEach(() => { for (const scope of scopes.splice(0)) scope.stop(); });

describe('text decoding request owner', () => {
  it('decodes the preview bytes it already holds with the detected encoding', async () => {
    const state = setup(); await state.text.ensureDecodedText();
    expect(mocks.detect).toHaveBeenCalledExactlyOnceWith(request);
    expect(state.text.decodedText.value).toBe('你好世界'); expect(mocks.text).not.toHaveBeenCalled();
  });
  it('drops a declared byte-order mark like the backend decoder', async () => {
    mocks.detect.mockResolvedValue(guess('UTF-16LE', true));
    const state = setup(Uint8Array.from([0xFF, 0xFE, 0x41, 0x00, 0x42, 0x00])); await state.text.ensureDecodedText();
    expect(state.text.decodedText.value).toBe('AB'); expect(state.text.autoEncodingLabel.value).toBe('自动 · UTF-16LE');
  });
  it('leaves a character cut at the end out of the text only when more of the file follows', async () => {
    // “你好” plus the first byte of “世”.
    const cut = GBK.subarray(0, 5);
    mocks.detect.mockResolvedValue(guess('GBK', false, true));
    const head = setup(cut); await head.text.ensureDecodedText();
    expect(head.text.decodedText.value).toBe('你好');
    mocks.detect.mockResolvedValue(guess('GBK'));
    const whole = setup(cut); await whole.text.ensureDecodedText();
    expect(whole.text.decodedText.value).toBe('你好�');
  });
  it('asks the backend for the text only when this WebView has no decoder for the encoding', async () => {
    mocks.detect.mockResolvedValue(guess('x-unknown'));
    const state = setup(); await state.text.ensureDecodedText();
    expect(mocks.text).toHaveBeenCalledExactlyOnceWith(request); expect(state.text.decodedText.value).toBe('backend text');
  });
  it('deduplicates detection for one generation and preserves the UTF-8 fallback on failure', async () => {
    const state = setup(); await Promise.all([state.text.ensureDecodedText(), state.text.ensureDecodedText()]);
    expect(mocks.detect).toHaveBeenCalledOnce(); expect(state.text.decodedText.value).toBe('你好世界');
    state.text.resetTextPreview(); mocks.detect.mockRejectedValueOnce(new Error('decode failed'));
    await state.text.ensureDecodedText(); expect(state.text.decodedText.value).toBe(new TextDecoder().decode(GBK));
  });
  it('does not publish an old detection after A→B→A or clear the new decoding gate', async () => {
    const old = deferred<ReturnType<typeof guess>>(), recent = deferred<ReturnType<typeof guess>>();
    mocks.detect.mockReturnValueOnce(old.promise).mockReturnValueOnce(Promise.resolve(guess('Big5'))).mockReturnValueOnce(recent.promise);
    const state = setup(); const first = state.text.ensureDecodedText(); state.node.value = 'file-b'; await state.text.ensureDecodedText();
    state.node.value = 'file-a'; const third = state.text.ensureDecodedText(); old.resolve(guess('windows-1252')); await first;
    expect(state.text.decoded.value).toBeNull(); await state.text.ensureDecodedText(); expect(mocks.detect).toHaveBeenCalledTimes(3);
    recent.resolve(guess('GBK')); await third;
    expect(state.text.decoded.value?.encoding).toBe('GBK'); expect(state.text.decodedText.value).toBe('你好世界');
  });
  it('clears manual encoding and old detection when evidence changes with the same node id', async () => {
    const state = setup(); await state.text.ensureDecodedText(); state.text.encodingOverride.value = 'utf-8'; state.image.value = 'image-b';
    expect(state.text.encodingOverride.value).toBe(''); expect(state.text.decoded.value).toBeNull();
    await state.text.ensureDecodedText(); expect(mocks.detect).toHaveBeenLastCalledWith({ ...request, imageId: 'image-b' });
  });
  it('drops successful detection after final disposal', async () => {
    const old = deferred<ReturnType<typeof guess>>(); mocks.detect.mockReturnValueOnce(old.promise);
    const state = setup(); const operation = state.text.ensureDecodedText(); state.scope.stop(); old.resolve(guess('GBK')); await operation;
    expect(state.text.decoded.value).toBeNull();
  });
});
