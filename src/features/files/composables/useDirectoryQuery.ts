
import { tr } from '../../../composables/useLocale';
import { onScopeDispose, ref, type Ref } from 'vue';
import { trackedFsChildrenPage as fsChildrenPage } from '../../tasks/api';
import { notifyError } from '../../../composables/useToast';
import type { FlatFileFilter, FlatSortColumn } from '../../../lib/ipc/fs/types';
import type { DiskFsNode as FsNode } from '../model/model';

export interface DirectoryQuery {
  filter?: FlatFileFilter;
  sort?: FlatSortColumn | null;
  descending?: boolean;
}

/** Cursor pagination owns totals/cursors, while the panel coordinates navigation. */
export function useDirectoryQuery(options: {
  imageId: () => string;
  parentId: () => string;
  generation: () => number;
  enabled: () => boolean;
  query: () => DirectoryQuery;
  signature: () => string;
  clientFilter: () => boolean;
  entries: Ref<FsNode[]>;
  loading: Ref<boolean>;
  hydrated: (nodes: FsNode[], generation: number) => void;
  firstPageLoaded: (nodes: FsNode[], generation: number) => Promise<void>;
  beforeAppend: () => void;
  wantedThrough: () => number;
}) {
  const { entries, loading } = options;
  const directoryTotal = ref(0);
  const directoryCursor = ref<string | null>(null);
  let directoryFetching = false;
  let directorySignature = '';
  let disposed = false;
  onScopeDispose(() => { disposed = true; });
  const current = (generation: number) => !disposed && generation === options.generation();

  async function loadDirectoryFirstPage(parentId: string, gen: number, restore: { targetId?: string | null; scrollTop?: number } = {}) {
    if (!current(gen) || !options.enabled()) return;
    const imageId = options.imageId();
    const query = options.query();
    const signature = options.signature();
    directorySignature = signature;
    const target = restore.targetId;
    const restoreCount = Math.ceil((restore.scrollTop ?? 0) / 36) + 100;
    let cursor: string | null = null;
    const nodes: FsNode[] = [];
    do {
      const page = await fsChildrenPage({ imageId, parentId, query, cursor, pageSize: 500 });
      if (!current(gen) || signature !== options.signature()) return;
      if (page.nextCursor && page.nextCursor === cursor) throw new Error(tr("目录分页未前进，请刷新后重试"));
      nodes.push(...page.items);
      cursor = page.nextCursor;
      directoryTotal.value = page.total ?? nodes.length;
    } while (cursor && (options.clientFilter() || (target && !nodes.some(node => node.id === target)) || nodes.length < restoreCount));
    directoryCursor.value = cursor;
    entries.value = nodes;
    options.hydrated(nodes, gen);
    await options.firstPageLoaded(nodes, gen);
  }

  async function loadDirectoryNextPage() {
    if (disposed || directoryFetching || loading.value || !options.enabled() || !directoryCursor.value) return;
    directoryFetching = true;
    const gen = options.generation();
    const signature = directorySignature;
    const cursor = directoryCursor.value;
    try {
      const page = await fsChildrenPage({ imageId: options.imageId(), parentId: options.parentId(),
        cursor, pageSize: 500, query: options.query() });
      if (!current(gen) || signature !== options.signature() || !options.enabled()) return;
      if (page.nextCursor && page.nextCursor === cursor) throw new Error(tr("目录分页未前进，请刷新后重试"));
      directoryCursor.value = page.nextCursor;
      directoryTotal.value = page.total ?? entries.value.length + page.items.length;
      options.beforeAppend();
      entries.value = entries.value.concat(page.items);
      options.hydrated(page.items, gen);
    } catch (error) {
      if (current(gen)) notifyError(tr("目录补页失败"), String(error));
    } finally {
      directoryFetching = false;
      if (!disposed && !current(gen) && !loading.value && options.wantedThrough() >= entries.value.length) {
        void loadDirectoryNextPage();
      }
    }
  }

  return { directoryTotal, directoryCursor, loadDirectoryFirstPage, loadDirectoryNextPage, loadedSignature: () => directorySignature };
}
