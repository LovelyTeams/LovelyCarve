
import { tr } from '../../../composables/useLocale';
import { computed, onScopeDispose, ref, shallowRef, watch } from 'vue';
import { timelineDatas, type TimelineQuery, type TimelineRow } from '../../../lib/ipc/timeline';
import { notifyError } from '../../../composables/useToast';
export function useTimelineRows(options: { imageId: () => string; buildQuery: () => TimelineQuery }) {
  const PAGE_SIZE = 200;
  const rows = shallowRef<TimelineRow[]>([]);
  const rowsTotal = ref(0);
  const loadingRows = ref(false);
  const order = ref<'asc' | 'desc'>('desc');
  const selectedRowId = ref<number | null>(null);
  const selectedRow = computed(() => rows.value.find(row => row.id === selectedRowId.value) ?? null);
  const rowsError = ref('');
  /** 请求代际：切换条件后旧响应丢弃。 */
  let rowsGeneration = 0;
  let fetchingGeneration: number | null = null;
  let disposed = false;
  const revision = ref(0);

  async function loadRowsFirstPage() {
    if (disposed) return;
    const generation = ++rowsGeneration;
    revision.value++;
    loadingRows.value = true;
    rowsError.value = '';
    rows.value = [];
    rowsTotal.value = 0;
    selectedRowId.value = null;
    try {
      const page = await timelineDatas({
        imageId: options.imageId(),
        query: options.buildQuery(),
        skip: 0,
        limit: PAGE_SIZE,
        order: order.value,
      });
      if (disposed || generation !== rowsGeneration) return;
      rows.value = page.items;
      rowsTotal.value = page.total;
      selectedRowId.value = null;
    } catch (error) {
      if (!disposed && generation === rowsGeneration) {
        rowsError.value = tr("记录加载失败，请重试");
        notifyError(tr("读取时间线明细失败"), String(error));
      }
    } finally {
      if (!disposed && generation === rowsGeneration) loadingRows.value = false;
    }
  }

  async function ensureRowsThrough(index: number) {
    if (disposed || loadingRows.value || fetchingGeneration === rowsGeneration || index < rows.value.length || rows.value.length >= rowsTotal.value) return;
    const generation = rowsGeneration;
    fetchingGeneration = generation;
    try {
      const page = await timelineDatas({
        imageId: options.imageId(),
        query: options.buildQuery(),
        skip: rows.value.length,
        limit: PAGE_SIZE,
        order: order.value,
      });
      if (disposed || generation !== rowsGeneration) return;
      rows.value = rows.value.concat(page.items);
      rowsTotal.value = page.total;
    } catch (error) {
      if (!disposed && generation === rowsGeneration) {
        rowsError.value = tr("记录加载失败，请重试");
        notifyError(tr("读取时间线明细失败"), String(error));
      }
    } finally {
      if (fetchingGeneration === generation) fetchingGeneration = null;
    }
  }

  function toggleOrder() {
    order.value = order.value === 'desc' ? 'asc' : 'desc';
  }


  function reset() {
    rowsGeneration++; rows.value = []; rowsTotal.value = 0; selectedRowId.value = null;
    loadingRows.value = false; rowsError.value = ''; fetchingGeneration = null; revision.value++;
  }
  watch(order, () => { void loadRowsFirstPage(); });
  onScopeDispose(() => { disposed = true; rowsGeneration++; fetchingGeneration = null; });
  return {
    rows, rowsTotal, loadingRows, order, selectedRowId, selectedRow, rowsError, revision,
    loadRowsFirstPage, ensureRowsThrough, toggleOrder, reset
  };
}
