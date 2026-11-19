import { computed, onScopeDispose, ref, shallowRef, watch } from 'vue';
import { globalSearchHitsPage, type GlobalSearchDistinct, type GlobalSearchHit } from '../../../lib/ipc/search';
import type { ImageId } from '../../../lib/ipc/ids';
interface SearchHitsOptions { imageId: () => ImageId; searchId: () => string | null; running: () => boolean; }
/** One owner for the selected search's completed pages and running cursor. */
export function useSearchHits(options: SearchHitsOptions) {
  const hits = shallowRef<GlobalSearchHit[]>([]);
  const hitsTotal = ref(0);
  const loadingHits = ref(false);
  const hitsError = ref('');
  let failedRead: 'first' | 'more' | 'incremental' | null = null;
  const selectedHitSeq = ref<number | null>(null);
  const selectedHit = computed(() => hits.value.find(hit => hit.hit_seq === selectedHitSeq.value) ?? null);
  const filterKeywordDraft = ref('');
  const filterKeyword = ref('');
  const distinctMode = ref<GlobalSearchDistinct>('none');
  let generation = 0;
  let lastLoadedSeq = -1;
  let fetchingGeneration: number | null = null;
  let disposed = false;
  const current = (captured: number) => !disposed && captured === generation;

  async function pollIncremental() {
    const searchId = options.searchId();
    if (!searchId || !options.running() || disposed || fetchingGeneration === generation) return;
    const captured = generation;
    fetchingGeneration = captured;
    hitsError.value = ''; failedRead = null;
    try {
      const page = await globalSearchHitsPage({
        imageId: options.imageId(), searchId, limit: 500,
        afterSeq: lastLoadedSeq >= 0 ? lastLoadedSeq : undefined
      });
      if (!current(captured)) return;
      if (page.items.length) {
        hits.value = hits.value.concat(page.items);
        lastLoadedSeq = page.items[page.items.length - 1].hit_seq;
      }
      // afterSeq total counts only the remaining cursor range.
      hitsTotal.value = hits.value.length;
    } catch (error) { if (current(captured)) { hitsError.value = String(error); failedRead = 'incremental'; } }
    finally { if (fetchingGeneration === captured) fetchingGeneration = null; }
  }
  async function loadHitsFirstPage() {
    if (disposed) return;
    const captured = ++generation;
    lastLoadedSeq = -1;
    hits.value = []; hitsTotal.value = 0; selectedHitSeq.value = null;
    hitsError.value = ''; failedRead = null;
    const searchId = options.searchId();
    loadingHits.value = Boolean(searchId);
    if (!searchId) return;
    try {
      if (options.running()) await pollIncremental();
      else {
        const page = await globalSearchHitsPage({
          imageId: options.imageId(), searchId, skip: 0, limit: 200,
          keyword: filterKeyword.value || undefined, distinct: distinctMode.value
        });
        if (!current(captured)) return;
        hits.value = page.items; hitsTotal.value = page.total;
        lastLoadedSeq = page.items.length ? page.items[page.items.length - 1].hit_seq : -1;
      }
    } catch (error) { if (current(captured)) { hitsError.value = String(error); failedRead = 'first'; } }
    finally { if (current(captured)) loadingHits.value = false; }
  }
  async function ensureHitsThrough(index: number) {
    const searchId = options.searchId();
    if (!searchId || disposed || options.running() || loadingHits.value || fetchingGeneration === generation
      || hits.value.length >= hitsTotal.value || index + 60 < hits.value.length) return;
    const captured = generation;
    fetchingGeneration = captured;
    hitsError.value = ''; failedRead = null;
    try {
      const page = await globalSearchHitsPage({
        imageId: options.imageId(), searchId, skip: hits.value.length,
        limit: 200, keyword: filterKeyword.value || undefined, distinct: distinctMode.value
      });
      if (!current(captured)) return;
      hits.value = hits.value.concat(page.items); hitsTotal.value = page.total;
    } catch (error) { if (current(captured)) { hitsError.value = String(error); failedRead = 'more'; } }
    finally { if (fetchingGeneration === captured) fetchingGeneration = null; }
  }
  function retryHits() {
    switch (failedRead) {
      case 'incremental': void pollIncremental(); break;
      case 'more': void ensureHitsThrough(hits.value.length); break;
      case 'first': void loadHitsFirstPage(); break;
      case null: break;
    }
  }
  function applyFilters() { filterKeyword.value = filterKeywordDraft.value.trim(); void loadHitsFirstPage(); }
  function clearResultFilter() { filterKeywordDraft.value = ''; applyFilters(); }
  watch([options.imageId, options.searchId], () => {
    filterKeywordDraft.value = ''; filterKeyword.value = ''; distinctMode.value = 'none';
    void loadHitsFirstPage();
  }, { immediate: true, flush: 'sync' });
  watch(options.running, (running, previous) => { if (previous && !running) void loadHitsFirstPage(); });
  onScopeDispose(() => { disposed = true; generation++; fetchingGeneration = null; });
  return {
    hits, hitsTotal, loadingHits, hitsError, retryHits, selectedHitSeq, selectedHit, filterKeywordDraft, filterKeyword,
    distinctMode, loadHitsFirstPage, pollIncremental, ensureHitsThrough, applyFilters, clearResultFilter
  };
}
