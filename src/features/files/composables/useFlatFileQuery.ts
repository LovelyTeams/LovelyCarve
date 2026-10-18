
import { tr } from '../../../composables/useLocale';
import { onScopeDispose, ref, shallowRef, type Ref } from 'vue';
import { trackedFsFlatPage as fsFlatPage } from '../../tasks/api';
import { fsFlatExtCounts } from '../../../lib/ipc/fs/query';
import { notifyError } from '../../../composables/useToast';
import type { FlatFileFilter, FlatSortColumn, FsWalkEntry } from '../../../lib/ipc/fs/types';
import type { DiskFsNode as FsNode } from '../model/model';
import { FLAT_PAGE_SIZE, FLAT_CATCHUP_PAGE_SIZE, categoryCountsFromExtensions, flatRowKey } from '../model/flatQuery';

export interface FlatQueryRequest {
  keyword: string | null;
  filter: FlatFileFilter;
  sort: FlatSortColumn | null;
  sortdesc: boolean;
}

/** Recursive server-filtered pagination, including scroll catch-up and category counts. */
export function useFlatFileQuery(options: {
  imageId: () => string;
  parentId: () => string;
  pathPrefix: () => string;
  generation: () => number;
  enabled: () => boolean;
  request: () => FlatQueryRequest;
  signature: () => string;
  entries: Ref<FsNode[]>;
  loading: Ref<boolean>;
  hydrated: (nodes: FsNode[], generation: number) => void;
  beforeAppend: () => void;
}) {
  const { entries, loading } = options;
  const flatTotal = ref(0);
  const flatTruncated = ref(false);
  const flatCategoryCounts = shallowRef<{ counts: Record<string, number>; total: number }>({ counts: {}, total: 0 });
  let flatFetching = false;
  let flatWantedThrough = -1;
  let flatRefreshRequested = false;
  let lastLoadedFlatSignature: string | null = null;
  let flatCategoryCountsParent: string | null = null;
  let disposed = false;
  onScopeDispose(() => { disposed = true; });
  const current = (generation: number) => !disposed && generation === options.generation();

  function flatRowsFromPage(items: FsWalkEntry[], skip: number): FsNode[] {
    const prefix = options.pathPrefix();
    return items.map((entry, i) => ({
      ...entry.node,
      path: `${prefix}${entry.path}`,
      rowKey: flatRowKey(skip + i, entry.node.id),
    }));
  }

  async function loadFlatFirstPage(parentId: string, gen: number) {
    if (!current(gen) || !options.enabled()) return;
    const signature = options.signature();
    lastLoadedFlatSignature = signature;
    flatWantedThrough = -1;
    const refresh = flatRefreshRequested;
    flatRefreshRequested = false;
    const request = options.request();
    const page = await fsFlatPage({
      imageId: options.imageId(),
      parentId,
      deepsearch: true,
      keyword: request.keyword,
      sort: request.sort,
      sortdesc: request.sortdesc,
      skip: 0,
      limit: FLAT_PAGE_SIZE,
      filter: request.filter,
      refresh,
    });
    if (!current(gen) || !options.enabled() || signature !== options.signature()) return;
    flatTotal.value = page.total;
    flatTruncated.value = page.truncated;
    const rows = flatRowsFromPage(page.items, 0);
    entries.value = rows;
    options.hydrated(rows, gen);
    void loadFlatCategoryCounts(parentId, gen, refresh);
  }

  /** 按需补页到全局行号 `through`（含）：可视区间接近已加载末尾、拖滚动条或
   *  键盘跳转越过已加载行时调用。同一时刻只有一个补页请求在途，后到的目标
   *  只把 flatWantedThrough 推后；跨越大段留白时改用大页拉齐。 */
  async function ensureFlatLoadedThrough(through: number) {
    if (disposed || !options.enabled() || loading.value) return;
    flatWantedThrough = Math.max(flatWantedThrough, through);
    if (flatFetching) return;
    flatFetching = true;
    const gen = options.generation();
    const parentId = options.parentId();
    // 循环因装载代际更替而退出时，新一代首页装载期间滚动到的目标只记在
    // flatWantedThrough 里，没有人再发起补页——收尾时按当前状态再跑一轮。
    let supersededByReload = false;
    try {
      while (options.enabled() && entries.value.length <= flatWantedThrough) {
        if (!current(gen) || options.signature() !== lastLoadedFlatSignature) {
          // 首页重载在途 / 过滤排序已变：补页会拼进不同查询的行，让重载接手。
          supersededByReload = true;
          return;
        }
        if (entries.value.length >= flatTotal.value) return;
        const skip = entries.value.length;
        const gap = flatWantedThrough - skip + 1;
        const limit = gap > FLAT_PAGE_SIZE ? FLAT_CATCHUP_PAGE_SIZE : FLAT_PAGE_SIZE;
        const request = options.request();
        const page = await fsFlatPage({
          imageId: options.imageId(),
          parentId,
          deepsearch: true,
          keyword: request.keyword,
          sort: request.sort,
          sortdesc: request.sortdesc,
          skip,
          limit,
          filter: request.filter,
        });
        if (!current(gen) || !options.enabled() || options.signature() !== lastLoadedFlatSignature) {
          supersededByReload = true;
          return;
        }
        flatTotal.value = page.total;
        flatTruncated.value = page.truncated;
        // 总数在两页之间缩水（源卷变化）：没有更多行可拼，别空转。
        if (page.items.length === 0) return;
        const rows = flatRowsFromPage(page.items, skip);
        options.beforeAppend();
        entries.value = entries.value.concat(rows);
        options.hydrated(rows, gen);
      }
    } catch (err) {
      if (current(gen)) notifyError(tr("加载更多全显结果失败"), String(err));
    } finally {
      flatFetching = false;
      if (!disposed && supersededByReload && !loading.value && options.signature() === lastLoadedFlatSignature && flatWantedThrough >= entries.value.length) {
        void ensureFlatLoadedThrough(flatWantedThrough);
      }
    }
  }

  // 类别条计数：按起点缓存，同一起点上只换过滤/排序时不重取。

  async function loadFlatCategoryCounts(parentId: string, gen: number, refresh: boolean) {
    if (!refresh && flatCategoryCountsParent === parentId) return;
    try {
      const summary = await fsFlatExtCounts({ imageId: options.imageId(), parentId });
      if (!current(gen) || !options.enabled()) return;
      flatCategoryCountsParent = parentId;
      flatCategoryCounts.value = {
        counts: categoryCountsFromExtensions(summary.counts),
        total: summary.total,
      };
    } catch (err) {
      // 计数取不到不影响列表；类别条只是少了徽标数字。
      console.warn('flat category counts failed', err);
    }
  }

  return { flatTotal, flatTruncated, flatCategoryCounts, loadFlatFirstPage, ensureFlatLoadedThrough, loadedSignature: () => lastLoadedFlatSignature, requestRefresh: () => { flatRefreshRequested = true; }, resetCategoryCounts: () => { flatCategoryCountsParent = null; } };
}
