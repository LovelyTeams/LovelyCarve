
import { tr } from '../../../composables/useLocale';
import { onScopeDispose, ref, shallowRef, watch } from 'vue';
import { carveResultsPage, type CarveArtifact, type CarveScanSnapshot } from '../../../lib/ipc/carve';
import { mergeCarveArtifacts } from '../model/carveResults';

const PAGE_SIZE = 500;

/** Loads every signature result of the selected scan; the result tree needs all of them. */
export function useCarveResults(options: { imageId: () => string; scan: () => CarveScanSnapshot | null }) {
  let disposed = false; let generation = 0; let pending = false;
  // Pages arrive in result order and never mutate; a deep proxy over large scans is pure overhead.
  const artifacts = shallowRef<CarveArtifact[]>([]);
  const resultTotal = ref(0);
  const syncingResults = ref(false);
  const resultsError = ref('');
  let cursor: string | null = null;

  function resetResults() {
    generation++;
    pending = false;
    artifacts.value = [];
    cursor = null;
    resultTotal.value = 0;
    syncingResults.value = false;
    resultsError.value = '';
  }

  /** Reads from the last stable cursor until the current end of the scan. */
  async function drain() {
    const current = options.scan();
    if (!current || disposed) return;
    if (syncingResults.value) { pending = true; return; }
    const captured = generation;
    syncingResults.value = true;
    resultsError.value = '';
    try {
      for (;;) {
        const page = await carveResultsPage({ scanId: current.scan_id, cursor, limit: PAGE_SIZE });
        if (disposed || captured !== generation || options.scan()?.scan_id !== current.scan_id) return;
        if (page.next_cursor !== null && page.next_cursor === cursor) throw new Error(tr("结果游标未前进，请重试读取"));
        if (page.items.length) artifacts.value = mergeCarveArtifacts(artifacts.value, page.items);
        resultTotal.value = page.total;
        cursor = page.resume_cursor ?? cursor;
        if (!page.next_cursor) break;
      }
    } catch (error) {
      if (!disposed && captured === generation) resultsError.value = String(error);
    } finally {
      if (captured === generation) {
        syncingResults.value = false;
        if (pending) {
          pending = false;
          if (!resultsError.value) void drain();
        }
      }
    }
  }

  function loadFirstPage() {
    if (disposed) return Promise.resolve();
    resetResults();
    return drain();
  }

  function syncNewResults() { void drain(); }
  function retryResults() { void drain(); }

  watch(options.imageId, resetResults, { flush: 'sync' });
  onScopeDispose(() => { disposed = true; generation++; });
  return { artifacts, resultTotal, syncingResults, resultsError, retryResults, syncNewResults, loadFirstPage, resetResults };
}
