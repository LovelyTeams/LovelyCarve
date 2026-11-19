import { computed, onScopeDispose, ref, watch } from 'vue';
import { fsZipMemberRead, type ZipPreviewEntry } from '../../../lib/ipc/zip';
import type { ImageId, NodeId } from '../../../lib/ipc/ids';
export function useZipMember(options: { imageId: () => ImageId; nodeId: () => NodeId }) {
  const MEMBER_FETCH_CAP = 1024 * 1024;
  const HEX_VIEW_CAP = 32 * 1024;
  const TEXT_CHUNK_CHARS = 8000;
  let generation = 0; let disposed = false;
  const selected = ref<ZipPreviewEntry | null>(null);
  const memberBytes = ref<Uint8Array | null>(null);
  const memberLoading = ref(false);
  const memberError = ref<string | null>(null);
  const memberTab = ref<'text' | 'hex'>('text');

  const canPreview = computed(() => selected.value?.kind === 'file');
  const memberTruncated = computed(
    () => !!selected.value && !!memberBytes.value && selected.value.size > memberBytes.value.length,
  );

  async function selectMember(e: ZipPreviewEntry) {
    if (disposed) return;
    const captured = ++generation;
    selected.value = e;
    memberLoading.value = false;
    memberBytes.value = null;
    memberError.value = null;
    if (e.kind !== 'file') return; // 符号链接 / 加密 / 不支持：模板直接给出说明
    if (e.size === 0) {
      memberBytes.value = new Uint8Array();
      return;
    }
    const path = e.path;
    memberLoading.value = true;
    try {
      const buf = await fsZipMemberRead({
        imageId: options.imageId(),
        nodeId: options.nodeId(),
        memberPath: path,
        maxBytes: MEMBER_FETCH_CAP,
      });
      if (disposed || captured !== generation) return; // 已切换到别的成员，丢弃过期结果
      const bytes = new Uint8Array(buf);
      memberBytes.value = bytes;
      memberTab.value = looksTextual(bytes) ? 'text' : 'hex';
    } catch (err) {
      if (!disposed && captured === generation) memberError.value = String(err);
    } finally {
      if (!disposed && captured === generation) memberLoading.value = false;
    }
  }

  // 头部嗅探：含 NUL → 二进制（默认十六进制）；否则可打印比例够高 → 文本。
  function looksTextual(b: Uint8Array): boolean {
    const n = Math.min(b.length, 4096);
    if (n === 0) return true;
    let printable = 0;
    for (let i = 0; i < n; i++) {
      const c = b[i];
      if (c === 0) return false;
      if (c === 9 || c === 10 || c === 13 || (c >= 0x20 && c <= 0x7e) || c >= 0x80) printable += 1;
    }
    return printable / n > 0.85;
  }

  const memberText = computed(() => {
    const b = memberBytes.value;
    if (!b || b.length === 0) return '';
    try {
      return new TextDecoder('utf-8', { fatal: false }).decode(b);
    } catch {
      return '';
    }
  });

  const textChunks = computed<string[]>(() => {
    const text = memberText.value;
    if (!text) return [];
    const out: string[] = [];
    for (let i = 0; i < text.length; i += TEXT_CHUNK_CHARS) {
      out.push(text.slice(i, i + TEXT_CHUNK_CHARS));
    }
    return out;
  });

  const hexCapped = computed(() => !!memberBytes.value && memberBytes.value.length > HEX_VIEW_CAP);

  const hexRows = computed(() => {
    const b = memberBytes.value;
    if (!b) return [] as { offset: string; bytes: string; ascii: string }[];
    const n = Math.min(b.length, HEX_VIEW_CAP);
    const rows: { offset: string; bytes: string; ascii: string }[] = [];
    for (let off = 0; off < n; off += 16) {
      const end = Math.min(off + 16, n);
      let bytes = '';
      let ascii = '';
      for (let j = off; j < end; j++) {
        bytes += b[j].toString(16).padStart(2, '0').toUpperCase() + ' ';
        ascii += b[j] >= 0x20 && b[j] <= 0x7e ? String.fromCharCode(b[j]) : '.';
      }
      // 末行补齐字节列宽，ASCII 列对齐。
      bytes = bytes.padEnd(16 * 3, ' ');
      rows.push({ offset: off.toString(16).padStart(8, '0').toUpperCase(), bytes, ascii });
    }
    return rows;
  });


  watch([options.imageId, options.nodeId], () => {
    generation++; selected.value = null; memberBytes.value = null; memberLoading.value = false; memberError.value = null;
  }, { flush: 'sync' });
  onScopeDispose(() => { disposed = true; generation++; });
  return { selected, memberBytes, memberLoading, memberError, memberTab, canPreview, memberTruncated, textChunks, hexCapped, hexRows, HEX_VIEW_CAP, selectMember };
}
