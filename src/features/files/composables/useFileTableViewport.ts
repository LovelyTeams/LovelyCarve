import { nextTick, onDeactivated, onScopeDispose, ref, watch, type Ref } from 'vue';
import { useVirtualList } from '../../../composables/useVirtualList';
import { rowKeyOf, type DiskFsNode as FsNode } from '../model/model';
import type { FileScrollRestore } from '../model/navigation';
import { FLAT_PREFETCH_ROWS, flatSortColumn } from '../model/flatQuery';
import { LARGE_FILE_LIST as LARGE_LIST } from '../model/filters';
import type { useDirectoryQuery } from './useDirectoryQuery';
import type { useFlatFileQuery } from './useFlatFileQuery';

/** Virtual viewport, scroll restoration and query-reload scheduling are panel-local. */
export function useFileTableViewport(options: {
  entries: Ref<FsNode[]>; rows: Ref<FsNode[]>; loading: Ref<boolean>; flatMode: Ref<boolean>;
  container: Ref<HTMLElement | null>; rowHeight: number; virtualCount: Ref<number>;
  selectedKey: Ref<string | null>; detailNode: Ref<FsNode | null>; pendingRestore: Ref<FileScrollRestore | null>;
  sortKey: Ref<string | null>; sortDir: Ref<'asc' | 'desc'>;
  flatSignature: Ref<string>; directorySignature: Ref<string>;
  flatQuery: Pick<ReturnType<typeof useFlatFileQuery>, 'flatTotal' | 'ensureFlatLoadedThrough' | 'loadedSignature'>;
  directoryQuery: Pick<ReturnType<typeof useDirectoryQuery>, 'loadDirectoryNextPage' | 'loadedSignature'>;
  generation: () => number;
  loadCurrent: () => Promise<void>;
  hydrateVisible: (nodes: FsNode[]) => void;
  scrollToRow: (index: number) => void;
}) {
  const { entries, rows: filteredEntries, loading, flatMode: deepsearch, container: tableWrapRef,
    rowHeight: ROW_HEIGHT, virtualCount: flatVirtualCount, selectedKey, detailNode, pendingRestore,
    sortKey, sortDir, flatSignature: flatRequestSignature, directorySignature: directoryRequestSignature,
    flatQuery, directoryQuery, loadCurrent } = options;
  const { flatTotal, ensureFlatLoadedThrough } = flatQuery;
  const { loadDirectoryNextPage } = directoryQuery;
  let disposed = false;
  let sortFrame: number | null = null;
  const { visibleItems, paddingTop, paddingBottom, scrollToTop, range } = useVirtualList<FsNode>({
    items: filteredEntries,
    itemHeight: ROW_HEIGHT,
    containerRef: tableWrapRef,
    overscan: 6,
    virtualCount: flatVirtualCount,
  });
  // 可视区间接近或越过已加载末尾 → 预取下一页（或跨越留白一次拉齐）。
  watch([range, () => entries.value.length, loading], ([{ end }]) => {
    if (!deepsearch.value) {
      if (end + FLAT_PREFETCH_ROWS >= entries.value.length) void loadDirectoryNextPage();
      return;
    }
    if (loading.value) return;
    if (entries.value.length >= flatTotal.value) return;
    const wanted = Math.min(flatTotal.value - 1, end + FLAT_PREFETCH_ROWS);
    if (wanted >= entries.value.length) void ensureFlatLoadedThrough(wanted);
  });
  // 关键词 / 列漏斗 / 排序变化 → 从第一页重载。去抖吸收连续击键；签名相同
  // （开关切换时 showAllFiles 已经装载过）则不重复装载。
  let flatReloadTimer: number | null = null;
  watch(flatRequestSignature, () => {
    if (!deepsearch.value) return;
    if (flatReloadTimer !== null) window.clearTimeout(flatReloadTimer);
    flatReloadTimer = window.setTimeout(() => {
      flatReloadTimer = null;
      if (!deepsearch.value || flatRequestSignature.value === flatQuery.loadedSignature()) return;
      loadCurrent();
    }, 200);
  });
  onScopeDispose(() => {
    if (flatReloadTimer !== null) window.clearTimeout(flatReloadTimer);
  });
  let directoryReloadTimer: ReturnType<typeof setTimeout> | null = null;
  watch(directoryRequestSignature, () => {
    if (deepsearch.value) return;
    if (directoryReloadTimer) clearTimeout(directoryReloadTimer);
    directoryReloadTimer = setTimeout(() => {
      if (!deepsearch.value && directoryQuery.loadedSignature() !== directoryRequestSignature.value) void loadCurrent();
    }, 200);
  });
  onScopeDispose(() => { if (directoryReloadTimer) clearTimeout(directoryReloadTimer); });
  watch(visibleItems, rows => {
    if (!deepsearch.value) options.hydrateVisible(rows.map(row => row.item));
  });
  // Reset the virtual scroll to the top when navigating into a new directory —
  // unless a goUp/goTo armed a position restore (put the user back where they
  // left, highlight the directory just exited), or the change is a flat-mode
  // page append (loading the next page must not yank the scroll).
  let suppressScrollReset = false;
  watch(entries, () => {
    if (suppressScrollReset) {
      suppressScrollReset = false;
      return;
    }
    const restore = pendingRestore.value;
    pendingRestore.value = null;
    const generation = options.generation();
    const loadedEntries = entries.value;
    nextTick(() => {
      if (disposed || generation !== options.generation() || loadedEntries !== entries.value) return;
      if (!restore) {
        scrollToTop();
        return;
      }
      if (tableWrapRef.value) tableWrapRef.value.scrollTop = restore.scrollTop;
      const childNode = restore.childId
        ? entries.value.find(n => n.id === restore.childId) ?? null
        : null;
      if (childNode) {
        selectedKey.value = rowKeyOf(childNode);
        if (restore.openDetail) {
          // 树里点选文件：选中后打开其详情面板，并把该行滚动进视口。
          detailNode.value = childNode;
          const idx = filteredEntries.value.findIndex(n => rowKeyOf(n) === rowKeyOf(childNode));
          if (idx >= 0) options.scrollToRow(idx);
        }
      }
    });
  });

  // 排序忙碌指示：大列表上点列头先渲染一帧 spinner / progress 光标，下一帧
  // 才真正改排序键触发全量重算——否则窗口直接冻结数秒且无任何反馈。
  const sortBusyKey = ref<string | null>(null);
  function cancelPendingSort() {
    if (sortFrame !== null) cancelAnimationFrame(sortFrame);
    sortFrame = null;
    sortBusyKey.value = null;
  }
  onDeactivated(cancelPendingSort);
  onScopeDispose(() => { disposed = true; cancelPendingSort(); });

  /** 列头点击依次切换：升序 → 降序 → 恢复原始顺序。 */
  function cycleSort(key: string) {
    if (sortKey.value !== key) setSort(key, 'asc');
    else setSort(key, sortDir.value === 'asc' ? 'desc' : null);
  }

  /** 按 `key` 列以 `direction` 排序；`direction` 为 null 时撤销该列的排序。 */
  function setSort(key: string, direction: 'asc' | 'desc' | null) {
    // 上一次大列表排序还在两帧窗口里：忽略追加点击，免得两次全量排序背靠背
    // 跑、第二次还没有任何忙碌指示。
    if (disposed || sortBusyKey.value !== null) return;
    // 不改变排序的请求不触发重载，全显下的列头 spinner 也就无人收尾。
    if (direction === null ? sortKey.value !== key : sortKey.value === key && sortDir.value === direction) return;
    const apply = () => {
      if (direction === null) {
        sortKey.value = null;
        return;
      }
      sortKey.value = key;
      sortDir.value = direction;
    };
    if (deepsearch.value) {
      // 全显的排序在服务端：派生列（哈希/标签等）排不了，点了不动；其余列
      // 改键即触发请求签名重载，列头 spinner 由重载收尾时清掉。
      if (!flatSortColumn(key)) return;
      apply();
      sortBusyKey.value = key;
      return;
    }
    if (entries.value.length <= LARGE_LIST) {
      apply();
      return;
    }
    sortBusyKey.value = key;
    const generation = options.generation();
    // 两层 rAF：第一帧把 busy 指示画出来，第二帧再触发重算。
    sortFrame = requestAnimationFrame(() => {
      sortFrame = requestAnimationFrame(() => {
        sortFrame = null;
        if (disposed || generation !== options.generation()) {
          sortBusyKey.value = null;
          return;
        }
        apply();
        nextTick(() => {
          if (disposed || generation !== options.generation()) return;
          sortBusyKey.value = null;
        });
      });
    });
  }

  return { visibleItems, paddingTop, paddingBottom, range, sortBusyKey, cycleSort, setSort,
    beforeAppend: () => { suppressScrollReset = true; } };
}
