import { onScopeDispose, ref, shallowRef, watch } from 'vue';
import { carveFilesystemResults, type CarveScanSnapshot, type RecoveryScan } from '../../../lib/ipc/carve';

/** Loads the recovered volumes, directories and files once a session's filesystem phase finishes. */
export function useCarveFilesystem(options: { scan: () => CarveScanSnapshot | null }) {
  // Results are replaced as a whole and can hold tens of thousands of records.
  const result = shallowRef<RecoveryScan | null>(null);
  const loading = ref(false);
  const error = ref('');
  let loadedFor: string | null = null;
  let pendingFor: string | null = null;
  let request = 0;
  let disposed = false;

  async function load(scanId: string) {
    const current = ++request;
    pendingFor = scanId;
    loading.value = true;
    error.value = '';
    try {
      const loaded = await carveFilesystemResults({ scanId });
      if (disposed || current !== request) return;
      result.value = loaded;
      loadedFor = scanId;
    } catch (cause) {
      if (!disposed && current === request) error.value = String(cause);
    } finally {
      if (!disposed && current === request) { loading.value = false; pendingFor = null; }
    }
  }

  function retry() {
    const scanId = options.scan()?.scan_id;
    if (scanId) void load(scanId);
  }

  // Synchronous so A→B→A switches never keep B's tree or skip A's reload.
  watch([
    () => options.scan()?.scan_id ?? null,
    () => { const scan = options.scan(); return Boolean(scan?.filesystem?.finished && !scan.filesystem.error); },
  ], ([scanId, ready], previous) => {
    if (scanId !== previous?.[0]) {
      request++; loadedFor = null; pendingFor = null; result.value = null; loading.value = false; error.value = '';
    }
    if (scanId && ready && scanId !== loadedFor && scanId !== pendingFor) void load(scanId);
  }, { immediate: true, flush: 'sync' });
  onScopeDispose(() => { disposed = true; request++; });
  return { result, loading, error, retry };
}
