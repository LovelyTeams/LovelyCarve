import { onScopeDispose, ref, shallowRef, watch } from 'vue';
import { fsZipPreview, type ZipPreview } from '../../../lib/ipc/zip';
import type { ImageId, NodeId } from '../../../lib/ipc/ids';
export function useZipArchive(options: { imageId: () => ImageId; nodeId: () => NodeId }) {
  const loading = ref(false); const error = ref<string | null>(null); const report = shallowRef<ZipPreview | null>(null);
  let generation = 0; let disposed = false;
  async function load() {
    const captured = ++generation;
    loading.value = true; error.value = null; report.value = null;
    try {
      const loaded = await fsZipPreview({ imageId: options.imageId(), nodeId: options.nodeId() });
      if (!disposed && captured === generation) report.value = loaded;
    } catch (failure) { if (!disposed && captured === generation) error.value = String(failure); }
    finally { if (!disposed && captured === generation) loading.value = false; }
  }
  watch([options.imageId, options.nodeId], () => { void load(); }, { immediate: true, flush: 'sync' });
  onScopeDispose(() => { disposed = true; generation++; });
  return { loading, error, report };
}
