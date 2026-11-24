import { computed, onScopeDispose, ref, watch } from 'vue';
import { fsZipMemberRead, type ZipPreviewEntry } from '../../../lib/ipc/zip';
import type { ImageId, NodeId } from '../../../lib/ipc/ids';
import { HEX_VIEW_CAP, hexRows as toHexRows, looksTextual, textChunks as toTextChunks } from '../model/bytePreview';
export function useZipMember(options: { imageId: () => ImageId; nodeId: () => NodeId }) {
  const MEMBER_FETCH_CAP = 1024 * 1024;
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

  const textChunks = computed<string[]>(() => toTextChunks(memberBytes.value));
  const hexCapped = computed(() => !!memberBytes.value && memberBytes.value.length > HEX_VIEW_CAP);
  const hexRows = computed(() => toHexRows(memberBytes.value));


  watch([options.imageId, options.nodeId], () => {
    generation++; selected.value = null; memberBytes.value = null; memberLoading.value = false; memberError.value = null;
  }, { flush: 'sync' });
  onScopeDispose(() => { disposed = true; generation++; });
  return { selected, memberBytes, memberLoading, memberError, memberTab, canPreview, memberTruncated, textChunks, hexCapped, hexRows, HEX_VIEW_CAP, selectMember };
}
