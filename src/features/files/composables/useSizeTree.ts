import { computed, onScopeDispose, ref, shallowRef, watch } from 'vue';
import { fsSizeTree, type FsSizeTreeView } from '../../../lib/ipc/fs/sizeTree';

/** 树图的度量：大小、文件数，或按大小排布、按最新修改时间着色。 */
export type TreemapMode = 'bytes' | 'files' | 'age';

export type SizeTreeStatus = 'idle' | 'loading' | 'ready' | 'unindexed' | 'unsupported' | 'error';

/** 小于目录合计这一比例的条目画不出来，后端直接并入「其他」。 */
const MIN_SHARE = 1 / 50_000;
/** 进出目录时复用的视图数。 */
const CACHE_LIMIT = 16;

/** 当前目录的树图数据。按请求缓存已就绪的视图，返回上级、再次进入都不必等后端；
 *  卷索引未就绪时随快照状态变化自动重试。 */
export function useSizeTree(options: {
  imageId: () => string;
  /** 当前目录；为 null 时不请求。 */
  nodeId: () => string | null;
  enabled: () => boolean;
  mode: () => TreemapMode;
  depth: () => number;
  includeDeleted: () => boolean;
  /** 卷快照状态的签名：变化时重试未覆盖的目录。 */
  snapshotSignature: () => string;
}) {
  const status = ref<SizeTreeStatus>('idle');
  const view = shallowRef<FsSizeTreeView | null>(null);
  /** `view` 所属的目录。 */
  const viewNodeId = ref<string | null>(null);
  const error = ref<string | null>(null);
  const cache = new Map<string, FsSizeTreeView>();
  let generation = 0;
  onScopeDispose(() => { generation++; });

  const request = computed(() => {
    const nodeId = options.nodeId();
    if (!options.enabled() || !nodeId) return null;
    return {
      imageId: options.imageId(),
      nodeId,
      depth: options.depth(),
      metric: options.mode() === 'files' ? 'files' as const : 'bytes' as const,
      includeDeleted: options.includeDeleted(),
      minShare: MIN_SHARE,
    };
  });
  const requestKey = computed(() => (request.value ? JSON.stringify(request.value) : null));

  function remember(key: string, value: FsSizeTreeView) {
    cache.delete(key);
    cache.set(key, value);
    while (cache.size > CACHE_LIMIT) {
      const oldest = cache.keys().next().value;
      if (oldest === undefined) break;
      cache.delete(oldest);
    }
  }

  async function load() {
    const current = request.value;
    const key = requestKey.value;
    const ticket = ++generation;
    if (!current || !key) {
      status.value = 'idle';
      return;
    }
    const cached = cache.get(key);
    if (cached) {
      remember(key, cached);
      view.value = cached;
      viewNodeId.value = current.nodeId;
      status.value = 'ready';
      error.value = null;
      return;
    }
    // 上一个视图留在屏幕上，直到新的到达；状态只标出正在加载。
    status.value = 'loading';
    error.value = null;
    try {
      const result = await fsSizeTree(current);
      if (ticket !== generation) return;
      if (result.status === 'ready') {
        remember(key, result.view);
        view.value = result.view;
        viewNodeId.value = current.nodeId;
      } else {
        view.value = null;
        viewNodeId.value = null;
      }
      status.value = result.status;
    } catch (cause) {
      if (ticket !== generation) return;
      view.value = null;
      viewNodeId.value = null;
      error.value = String(cause);
      status.value = 'error';
    }
  }

  watch(requestKey, () => { void load(); }, { immediate: true });
  // 快照构建完成（或状态变化）时，只重试还没有数据的目录。
  watch(options.snapshotSignature, () => {
    if (status.value === 'unindexed' || status.value === 'error') void load();
  });

  /** 刷新：丢弃缓存的视图重新请求（后端的聚合树随索引文件更新）。 */
  function refresh() {
    cache.clear();
    void load();
  }

  /** 换检材时丢弃全部视图：节点 id 只在一个检材内有效。 */
  function reset() {
    generation++;
    cache.clear();
    view.value = null;
    viewNodeId.value = null;
    error.value = null;
    status.value = 'idle';
  }

  return { status, view, viewNodeId, error, refresh, reset, retry: load };
}
