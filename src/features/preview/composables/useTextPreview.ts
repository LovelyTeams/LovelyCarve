
import { tr } from '../../../composables/useLocale';
import { computed, onScopeDispose, ref, watch, type ComputedRef, type Ref } from 'vue';
import { fsPreviewText, type DecodedText } from '../../../lib/ipc/fs/read';
import { highlightHtml } from '../../../utils/highlight';
import type { PreviewCategory } from '../model/preview';

type DetailTab = 'detail' | 'raw' | 'text' | 'hex';

interface UseTextPreviewOptions {
  imageId: () => string;
  nodeId: () => string;
  nodeKind: () => string;
  activeTab: Ref<DetailTab>;
  category: ComputedRef<PreviewCategory>;
  bytes: Ref<Uint8Array>;
  searchQuery: Ref<string>;
  previewCap: number;
}

const TEXT_CHUNK_CHARS = 8000;

const decoderCache = new Map<string, TextDecoder>();

function getDecoder(label: string): TextDecoder | null {
  let decoder = decoderCache.get(label);
  if (!decoder) {
    try {
      decoder = new TextDecoder(label, { fatal: false, ignoreBOM: true });
    } catch {
      return null;
    }
    decoderCache.set(label, decoder);
  }
  return decoder;
}

function chunkText(text: string, target: number): string[] {
  if (!text) return [];
  const chunks: string[] = [];
  const length = text.length;
  let index = 0;
  while (index < length) {
    let end = Math.min(index + target, length);
    let next = end;
    if (end < length) {
      const newline = text.lastIndexOf('\n', end);
      if (newline > index) {
        end = newline;
        next = newline + 1;
      }
    }
    chunks.push(text.slice(index, end));
    index = next;
  }
  return chunks;
}

export function useTextPreview(options: UseTextPreviewOptions) {
  const decoded = ref<(DecodedText & { nodeId: string }) | null>(null);
  const decodingSettled = ref(false);
  const encodingOverride = ref('');
  let decodingFor: number | null = null;
  let generation = 0;
  let disposed = false;

  function resetTextPreview() {
    generation++;
    decodingFor = null;
    decoded.value = null;
    decodingSettled.value = false;
    encodingOverride.value = '';
  }
  watch(() => [options.imageId(), options.nodeId()], resetTextPreview, { flush: 'sync' });
  onScopeDispose(() => { disposed = true; generation++; });

  const decodedText = computed(() => {
    const bytes = options.bytes.value;
    if (encodingOverride.value) {
      if (bytes.length === 0) return '';
      const decoder = getDecoder(encodingOverride.value);
      if (decoder) {
        try {
          return decoder.decode(bytes);
        } catch {
          // Fall through to the UTF-8 fallback below.
        }
      }
    }

    const decodedValue = decoded.value;
    if (decodedValue && decodedValue.nodeId === options.nodeId()) return decodedValue.text;
    if (bytes.length === 0) return '';
    try {
      return new TextDecoder('utf-8', { fatal: false }).decode(bytes);
    } catch {
      return '[binary]';
    }
  });

  const needsDecodedText = computed(() =>
    options.nodeKind() === 'file' &&
    options.bytes.value.length > 0 &&
    (
      options.activeTab.value === 'text' ||
      (
        options.activeTab.value === 'raw' &&
        (
          options.category.value === 'markdown' ||
          options.category.value === 'code' ||
          options.category.value === 'structured'
        )
      )
    ),
  );

  const autoEncodingLabel = computed(() => {
    const decodedValue = decoded.value;
    return decodedValue && decodedValue.nodeId === options.nodeId()
      ? tr("自动 · {0}", { 0: decodedValue.encoding })
      : tr("自动");
  });

  const encodingWarn = computed(() =>
    !encodingOverride.value &&
    decoded.value?.nodeId === options.nodeId() &&
    decoded.value.had_errors,
  );

  const encodingTitle = computed(() => {
    if (encodingOverride.value) return tr("已手动指定文本编码（点选「自动」恢复检测）");
    const decodedValue = decoded.value;
    if (!decodedValue || decodedValue.nodeId !== options.nodeId()) return tr("文本编码（自动检测）");
    const how = decodedValue.from_bom ? tr("BOM 声明") : tr("自动检测");
    return decodedValue.had_errors
      ? tr("编码 {0}（{1}）· 含无法解码字节，若仍乱码可手动切换编码", { 0: decodedValue.encoding, 1: how })
      : tr("编码 {0}（{1}）", { 0: decodedValue.encoding, 1: how });
  });

  async function ensureDecodedText() {
    if (disposed || !needsDecodedText.value) return;
    const nodeId = options.nodeId();
    const imageId = options.imageId();
    const requestGeneration = generation;
    if (decoded.value?.nodeId === nodeId) return;
    if (decodingFor === requestGeneration) return;
    decodingFor = requestGeneration;
    try {
      const result = await fsPreviewText({
        imageId,
        nodeId,
        maxBytes: options.previewCap,
      });
      if (disposed || requestGeneration !== generation || imageId !== options.imageId() || nodeId !== options.nodeId()) return;
      decoded.value = { ...result, nodeId };
    } catch {
      // Keep the UTF-8 fallback in place; preview should not fail because
      // charset detection did.
    } finally {
      if (decodingFor === requestGeneration) {
        decodingFor = null;
        decodingSettled.value = true;
      }
    }
  }

  const highlightedTextHtml = computed(() =>
    highlightHtml(decodedText.value, options.searchQuery.value).html,
  );

  const textChunks = computed(() =>
    options.searchQuery.value ? [] : chunkText(decodedText.value, TEXT_CHUNK_CHARS),
  );

  return {
    decoded,
    decodingSettled,
    decodedText,
    encodingOverride,
    needsDecodedText,
    autoEncodingLabel,
    encodingWarn,
    encodingTitle,
    ensureDecodedText,
    highlightedTextHtml,
    textChunks,
    resetTextPreview,
  };
}
