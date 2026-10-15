
import { tr } from '../../../composables/useLocale';
import { computed, onScopeDispose, ref, watch } from 'vue';
import { carveResultsPage, type CarveArtifact, type CarveCategory, type CarveScanSnapshot } from '../../../lib/ipc/carve';
import { mergeCarveArtifacts } from '../model/carveResults';
import { formatOffset } from '../model/carveView';
export function useCarveResults(options: { imageId: () => string; scan: () => CarveScanSnapshot | null }) {
  let disposed = false; let pageGeneration = 0; let pendingResultSync = false;
  const artifacts = ref<CarveArtifact[]>([]);
  const nextCursor = ref<string | null>(null);
  const resumeCursor = ref<string | null>(null);
  const resultTotal = ref(0);
  const loadingMore = ref(false);
  const syncingResults = ref(false);
  const resultsError = ref('');
  let failedRead: 'more' | 'sync' | null = null;
  const query = ref('');
  const queryDraft = ref('');
  const categoryFilterDraft = ref<'all' | CarveCategory>('all');
  const categoryFilter = ref<'all' | CarveCategory>('all');
  const hasFilters = computed(() => query.value.trim().length > 0 || categoryFilter.value !== 'all');
  const selectedId = ref<string | null>(null);
  const checkedIds = ref<Set<string>>(new Set());
  function applyFilters() {
    query.value = queryDraft.value.trim();
    categoryFilter.value = categoryFilterDraft.value;
    if (hasFilters.value && nextCursor.value) void loadMore();
  }
  function clearFilters() {
    queryDraft.value = query.value = '';
    categoryFilterDraft.value = categoryFilter.value = 'all';
  }
  function resetResults() {
    pageGeneration++;
    pendingResultSync = false;
    selectedId.value = null;
    artifacts.value = [];
    checkedIds.value = new Set();
    nextCursor.value = resumeCursor.value = null;
    resultTotal.value = 0;
    syncingResults.value = loadingMore.value = false;
    resultsError.value = ''; failedRead = null;
    clearFilters();
  }
  const visibleArtifacts = computed(() => {
    const normalized = query.value.trim().toLocaleLowerCase();
    return artifacts.value.filter(artifact => {
      if (categoryFilter.value !== 'all' && artifact.category !== categoryFilter.value) return false;
      if (!normalized) return true;
      return artifact.name.toLocaleLowerCase().includes(normalized)
        || artifact.extension.toLocaleLowerCase().includes(normalized)
        || artifact.format_id.toLocaleLowerCase().includes(normalized)
        || artifact.region.toLocaleLowerCase().includes(normalized)
        || formatOffset(artifact.offset).toLocaleLowerCase().includes(normalized);
    });
  });
  const selectedArtifact = computed(() =>
    artifacts.value.find(artifact => artifact.id === selectedId.value) ?? null,
  );
  const allVisibleSelected = computed(() =>
    visibleArtifacts.value.length > 0
    && visibleArtifacts.value.every(artifact => checkedIds.value.has(artifact.id)),
  );
  function toggleChecked(id: string) {
    const next = new Set(checkedIds.value);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    checkedIds.value = next;
  }

  function toggleAllVisible() {
    const next = new Set(checkedIds.value);
    if (allVisibleSelected.value) {
      for (const artifact of visibleArtifacts.value) next.delete(artifact.id);
    } else {
      for (const artifact of visibleArtifacts.value) next.add(artifact.id);
    }
    checkedIds.value = next;
  }

  async function fetchPage(cursor: string | null, reset: boolean) {
    const current = options.scan();
    if (!current) return;
    const generation = pageGeneration;
    let page = await carveResultsPage({
      scanId: current.scan_id,
      cursor,
      limit: 200,
    });
    if (disposed || generation !== pageGeneration || options.scan()?.scan_id !== current.scan_id) return;
    artifacts.value = reset ? page.items : mergeCarveArtifacts(artifacts.value, page.items);
    nextCursor.value = page.next_cursor;
    resumeCursor.value = page.resume_cursor;
    resultTotal.value = page.total;
    // A filter describes the whole retained scan. Read remaining metadata pages
    // before declaring no match; clearing the filter stops this drain after the
    // current page, and context generations reject late responses.
    while (hasFilters.value && page.next_cursor) {
      const cursor = page.next_cursor;
      page = await carveResultsPage({ scanId: current.scan_id, cursor, limit: 200 });
      if (disposed || generation !== pageGeneration || options.scan()?.scan_id !== current.scan_id) return;
      if (page.next_cursor === cursor) throw new Error(tr("结果游标未前进，请重试读取"));
      artifacts.value = mergeCarveArtifacts(artifacts.value, page.items);
      nextCursor.value = page.next_cursor;
      resumeCursor.value = page.resume_cursor;
      resultTotal.value = page.total;
    }
  }

  async function syncNewResults() {
    if (!options.scan() || disposed) return;
    if (syncingResults.value || loadingMore.value) {
      pendingResultSync = true;
      return;
    }
    syncingResults.value = true;
    resultsError.value = ''; failedRead = null;
    const generation = pageGeneration;
    try {
      await fetchPage(resumeCursor.value, artifacts.value.length === 0);
    } catch (error) {
      if (!disposed && generation === pageGeneration) { resultsError.value = String(error); failedRead = 'sync'; }
    } finally {
      if (generation === pageGeneration) {
        syncingResults.value = false;
        if (pendingResultSync) {
          pendingResultSync = false;
          void syncNewResults();
        }
      }
    }
  }

  async function loadMore() {
    if (!nextCursor.value || loadingMore.value || syncingResults.value) return;
    const generation = pageGeneration;
    loadingMore.value = true;
    resultsError.value = ''; failedRead = null;
    try {
      await fetchPage(nextCursor.value, false);
    } catch (error) {
      if (!disposed && generation === pageGeneration) { resultsError.value = String(error); failedRead = 'more'; }
    } finally {
      if (generation === pageGeneration) {
        loadingMore.value = false;
        if (pendingResultSync) {
          pendingResultSync = false;
          void syncNewResults();
        }
      }
    }
  }

  async function loadFirstPage() {
    if (disposed) return;
    resetResults();
    const generation = pageGeneration;
    syncingResults.value = true;
    try {
      await fetchPage(null, true);
    } catch (error) {
      if (!disposed && generation === pageGeneration) { resultsError.value = String(error); failedRead = 'sync'; }
    } finally {
      if (generation === pageGeneration) {
        syncingResults.value = false;
        if (pendingResultSync) {
          pendingResultSync = false;
          void syncNewResults();
        }
      }
    }
  }

  function retryResults() {
    if (failedRead === 'more') void loadMore();
    else void syncNewResults();
  }

  watch(options.imageId, resetResults, { flush: 'sync' });
  onScopeDispose(() => { disposed = true; pageGeneration++; });
  return {
    artifacts, nextCursor, resultTotal, loadingMore, syncingResults, resultsError, retryResults, query, queryDraft,
    categoryFilterDraft, categoryFilter, hasFilters, selectedId, checkedIds, visibleArtifacts, selectedArtifact, allVisibleSelected,
    applyFilters, clearFilters, toggleChecked, toggleAllVisible, loadMore, syncNewResults, loadFirstPage, resetResults
  };
}
