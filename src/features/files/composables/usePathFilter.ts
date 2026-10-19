import { computed, nextTick, reactive, ref, type Ref } from 'vue';
import { containerFsChildren as fsChildren } from './containerReads';
import type { DiskFsNode } from '../model/model';

export interface PathFilterStackFrame {
  id: string;
  name: string;
}

export interface PathSuggestion {
  value: string;
  name: string;
  isDir: boolean;
  pre: string;
  hit: string;
  post: string;
}

interface PathLevelChild {
  name: string;
  isDir: boolean;
  id?: string;
}

interface FilterPopoverState {
  visible: boolean;
  key: string;
}

interface UsePathFilterOptions {
  imageId: () => string;
  stack: Ref<PathFilterStackFrame[]>;
  filters: Record<string, string>;
  entries: Ref<DiskFsNode[]>;
  flatMode: Ref<boolean>;
  filterPopover: FilterPopoverState;
  filterPopoverEl: Ref<HTMLElement | null>;
  fullPathOf: (node: DiskFsNode) => string;
  loadCurrent: () => void;
  hideFilterPopover: () => void;
  suggestionElement: Ref<HTMLElement | null>;
}

const PATH_SUGGEST_MAX = 50;
const PATH_LEVEL_COLLATOR = new Intl.Collator();

function comparePathChildren(a: PathLevelChild, b: PathLevelChild): number {
  return Number(b.isDir) - Number(a.isDir) || PATH_LEVEL_COLLATOR.compare(a.name, b.name);
}

function pathKeyOf(base: string): string {
  return base === '/' ? '/' : base.toLowerCase();
}

function pathChildKeyOf(baseKey: string, name: string): string {
  return baseKey === '/' ? `/${name.toLowerCase()}` : `${baseKey}/${name.toLowerCase()}`;
}

export function usePathFilter(options: UsePathFilterOptions) {
  const pathSuggestOpen = ref(false);
  const pathSuggestIndex = ref(-1);
  const pathSuggestEl = options.suggestionElement;
  const pathLevelCache = reactive(new Map<string, PathLevelChild[]>());
  const pathLevelIds = new Map<string, string>();
  const pathLevelFetches = new Map<string, Promise<void>>();
  const pathLevelFetching = ref(0);
  let pathLevelGeneration = 0;
  let pathFollowToken = 0;
  let lastPathSuggestClickAt = 0;

  const pathLevelIndex = computed<Map<string, PathLevelChild[]>>(() => {
    const raw = new Map<string, Map<string, PathLevelChild>>();
    for (const node of options.entries.value) {
      const path = options.fullPathOf(node);
      if (!path.startsWith('/')) continue;
      const segments = path.split('/').filter(Boolean);
      let parentKey = '/';
      for (let index = 0; index < segments.length; index += 1) {
        const segment = segments[index];
        const isLeaf = index === segments.length - 1;
        const isDir = !isLeaf || node.kind === 'dir';
        let bucket = raw.get(parentKey);
        if (!bucket) {
          bucket = new Map();
          raw.set(parentKey, bucket);
        }
        const existing = bucket.get(segment);
        if (existing) {
          if (isDir) existing.isDir = true;
          if (isLeaf && !existing.id && !node.deleted) existing.id = node.id;
        } else {
          bucket.set(segment, {
            name: segment,
            isDir,
            id: isLeaf && !node.deleted ? node.id : undefined,
          });
        }
        parentKey =
          parentKey === '/'
            ? `/${segment.toLowerCase()}`
            : `${parentKey}/${segment.toLowerCase()}`;
      }
    }
    const out = new Map<string, PathLevelChild[]>();
    for (const [key, bucket] of raw) {
      out.set(key, Array.from(bucket.values()).sort(comparePathChildren));
    }
    return out;
  });

  function invalidatePathLevels() {
    pathLevelGeneration += 1;
    pathLevelCache.clear();
    pathLevelIds.clear();
    pathLevelFetches.clear();
  }

  function invalidatePathFollow() {
    pathFollowToken += 1;
  }

  function pathQueryBase(): string | null {
    const query = (options.filters.path ?? '').trim();
    if (query !== '' && !query.startsWith('/')) return null;
    const cut = query.lastIndexOf('/');
    return cut <= 0 ? '/' : query.slice(0, cut);
  }

  function ensurePathLevel(base: string): Promise<void> {
    const key = pathKeyOf(base);
    if (pathLevelCache.has(key)) return Promise.resolve();
    const running = pathLevelFetches.get(key);
    if (running) return running;
    const imageId = options.imageId();
    const generation = pathLevelGeneration;
    const job = (async () => {
      let acc = '/';
      pathLevelIds.set('/', options.stack.value[0]?.id ?? 'root');
      for (const frame of options.stack.value.slice(1)) {
        acc = pathChildKeyOf(acc, frame.name);
        pathLevelIds.set(acc, frame.id);
      }
      let parentId = pathLevelIds.get(key);
      if (!parentId && key !== '/') {
        const cut = key.lastIndexOf('/');
        await ensurePathLevel(cut <= 0 ? '/' : key.slice(0, cut));
        if (imageId !== options.imageId() || generation !== pathLevelGeneration) return;
        parentId = pathLevelIds.get(key);
      }
      if (!parentId) return;
      pathLevelFetching.value += 1;
      try {
        const nodes = await fsChildren({ imageId, parentId });
        if (imageId !== options.imageId() || generation !== pathLevelGeneration) return;
        for (const node of nodes) {
          if (node.kind === 'dir' && !node.deleted) {
            pathLevelIds.set(pathChildKeyOf(key, node.name), node.id);
          }
        }
        for (const node of nodes) {
          if (node.kind !== 'dir' || !node.deleted) continue;
          const childKey = pathChildKeyOf(key, node.name);
          if (!pathLevelIds.has(childKey)) pathLevelIds.set(childKey, node.id);
        }
        pathLevelCache.set(
          key,
          nodes
            .map(node => ({
              name: node.name,
              isDir: node.kind === 'dir',
              id: node.kind === 'dir' && !node.deleted ? node.id : undefined,
            }))
            .sort(comparePathChildren),
        );
      } catch {
        if (imageId === options.imageId() && generation === pathLevelGeneration) {
          pathLevelCache.set(key, []);
        }
      } finally {
        pathLevelFetching.value -= 1;
      }
    })().finally(() => { if (pathLevelFetches.get(key) === job) pathLevelFetches.delete(key); });
    pathLevelFetches.set(key, job);
    return job;
  }

  function ensureCurrentPathLevel() {
    if (options.filterPopover.key !== 'path' || !options.imageId()) return;
    const base = pathQueryBase();
    if (base !== null) void ensurePathLevel(base);
  }

  function rebasePathFilterToStack() {
    const query = (options.filters.path ?? '').trim();
    if (!query.startsWith('/')) return;
    const base = options.stack.value.slice(1).map(frame => frame.name).join('/');
    options.filters.path = base ? `/${base}/` : '';
  }

  async function followPathFilterBase() {
    if (options.flatMode.value || !options.imageId()) return;
    const query = (options.filters.path ?? '').trim();
    if (query === '' || !query.startsWith('/')) return;
    const base = pathQueryBase();
    if (base === null) return;
    const token = ++pathFollowToken;
    const imageId = options.imageId();
    const segments = base.split('/').filter(Boolean);
    const frames: { id: string; name: string }[] = [];
    let key = '/';
    for (const segment of segments) {
      await ensurePathLevel(key);
      if (token !== pathFollowToken || imageId !== options.imageId() || options.flatMode.value) return;
      const level = pathLevelCache.get(key) ?? pathLevelIndex.value.get(key);
      const segmentLower = segment.toLowerCase();
      const child =
        level?.find(candidate => candidate.isDir && candidate.name === segment) ??
        level?.find(candidate => candidate.isDir && candidate.name.toLowerCase() === segmentLower);
      const id = child?.id ?? pathLevelIds.get(pathChildKeyOf(key, segment));
      if (!id) return;
      frames.push({ id, name: child?.name ?? segment });
      key = pathChildKeyOf(key, segment);
    }
    await ensurePathLevel(key);
    if (token !== pathFollowToken || imageId !== options.imageId() || options.flatMode.value) return;
    const fragment = query.slice(base === '/' ? 1 : base.length + 1);
    if (fragment) {
      const level = pathLevelCache.get(key) ?? pathLevelIndex.value.get(key);
      const fragmentLower = fragment.toLowerCase();
      const child =
        level?.find(candidate => candidate.isDir && candidate.name === fragment) ??
        level?.find(candidate => candidate.isDir && candidate.name.toLowerCase() === fragmentLower);
      const fragmentId = child?.id ?? pathLevelIds.get(pathChildKeyOf(key, fragment));
      if (child && fragmentId) {
        frames.push({ id: fragmentId, name: child.name });
        key = pathChildKeyOf(key, fragment);
      }
    }
    if ((options.filters.path ?? '').trim() !== query) return;
    const target = [options.stack.value[0], ...frames];
    if (
      options.stack.value.length === target.length &&
      options.stack.value.every((frame, index) => frame.id === target[index].id)
    ) {
      return;
    }
    options.stack.value = target;
    options.loadCurrent();
  }

  async function resolveAbsoluteDirStack(path: string): Promise<PathFilterStackFrame[] | null> {
    if (!options.imageId()) return null;
    const imageId = options.imageId();
    const generation = pathLevelGeneration;
    let normalized = path.trim();
    if (!normalized.startsWith('/')) return null;
    if (normalized.length > 1) normalized = normalized.replace(/\/+$/, '');
    const segments = normalized.split('/').filter(Boolean);
    const frames: PathFilterStackFrame[] = [];
    let key = '/';
    for (const segment of segments) {
      await ensurePathLevel(key);
      if (imageId !== options.imageId() || generation !== pathLevelGeneration) return null;
      const level = pathLevelCache.get(key) ?? pathLevelIndex.value.get(key);
      const segmentLower = segment.toLowerCase();
      const child =
        level?.find(candidate => candidate.isDir && candidate.name === segment) ??
        level?.find(candidate => candidate.isDir && candidate.name.toLowerCase() === segmentLower);
      const id = child?.id ?? pathLevelIds.get(pathChildKeyOf(key, segment));
      if (!id) return null;
      frames.push({ id, name: child?.name ?? segment });
      key = pathChildKeyOf(key, segment);
    }
    return [options.stack.value[0] ?? { id: 'root', name: '/' }, ...frames];
  }

  const pathSuggestMatches = computed<{ list: PathSuggestion[]; total: number }>(() => {
    const query = (options.filters.path ?? '').trim();
    if (query !== '' && !query.startsWith('/')) return { list: [], total: 0 };
    const cut = query.lastIndexOf('/');
    const base = cut <= 0 ? '/' : query.slice(0, cut);
    const fragment = cut < 0 ? '' : query.slice(cut + 1);
    const key = base === '/' ? '/' : base.toLowerCase();
    const children = pathLevelCache.get(key) ?? pathLevelIndex.value.get(key) ?? [];
    const fragmentLower = fragment.toLowerCase();
    const baseForValue = base === '/' ? '' : base;
    const prefix: PathSuggestion[] = [];
    const inner: PathSuggestion[] = [];
    let total = 0;
    for (const child of children) {
      const nameLower = child.name.toLowerCase();
      const at = fragmentLower === '' ? 0 : nameLower.indexOf(fragmentLower);
      if (at < 0) continue;
      total += 1;
      const bucket = at === 0 ? prefix : inner;
      if (bucket.length >= PATH_SUGGEST_MAX) continue;
      const aligned =
        fragment !== '' &&
        nameLower.length === child.name.length &&
        fragmentLower.length === fragment.length;
      bucket.push({
        value: `${baseForValue}/${child.name}${child.isDir ? '/' : ''}`,
        name: child.name,
        isDir: child.isDir,
        pre: aligned ? child.name.slice(0, at) : child.name,
        hit: aligned ? child.name.slice(at, at + fragment.length) : '',
        post: aligned ? child.name.slice(at + fragment.length) : '',
      });
    }
    return { list: [...prefix, ...inner].slice(0, PATH_SUGGEST_MAX), total };
  });

  const pathSuggestVisible = computed(
    () =>
      options.filterPopover.visible &&
      options.filterPopover.key === 'path' &&
      pathSuggestOpen.value &&
      pathSuggestMatches.value.list.length > 0,
  );

  const pathSuggestLoadingVisible = computed(
    () =>
      options.filterPopover.visible &&
      options.filterPopover.key === 'path' &&
      pathSuggestOpen.value &&
      pathSuggestMatches.value.list.length === 0 &&
      pathLevelFetching.value > 0,
  );

  function closePathSuggest() {
    pathSuggestOpen.value = false;
    pathSuggestIndex.value = -1;
  }

  function focusPathInputAtEnd() {
    nextTick(() => {
      const input = options.filterPopoverEl.value?.querySelector<HTMLInputElement>('input[type="text"]');
      if (!input) return;
      input.focus();
      input.setSelectionRange(input.value.length, input.value.length);
    });
  }

  function pickPathSuggestion(suggestion: PathSuggestion) {
    options.filters.path = suggestion.value;
    pathSuggestIndex.value = -1;
    void followPathFilterBase();
    if (suggestion.isDir) {
      pathSuggestOpen.value = true;
      void ensurePathLevel(suggestion.value.slice(0, -1) || '/');
      nextTick(() => {
        if (pathSuggestEl.value) pathSuggestEl.value.scrollTop = 0;
      });
    } else {
      closePathSuggest();
    }
    focusPathInputAtEnd();
  }

  function onPathSuggestClick(suggestion: PathSuggestion) {
    const now = Date.now();
    if (now - lastPathSuggestClickAt < 300) return;
    lastPathSuggestClickAt = now;
    pickPathSuggestion(suggestion);
  }

  function onTextFilterInput() {
    if (options.filterPopover.key !== 'path') return;
    pathSuggestOpen.value = true;
    pathSuggestIndex.value = -1;
    ensureCurrentPathLevel();
    void followPathFilterBase();
  }

  function onTextFilterFocus() {
    if (options.filterPopover.key === 'path' && !(options.filters.path ?? '').trim()) {
      pathSuggestOpen.value = true;
      pathSuggestIndex.value = -1;
      ensureCurrentPathLevel();
    }
  }

  function scrollPathSuggestIntoView() {
    nextTick(() => {
      const listEl = pathSuggestEl.value;
      const item = listEl?.children[pathSuggestIndex.value];
      if (!listEl || !(item instanceof HTMLElement)) return;
      if (item.offsetTop < listEl.scrollTop) {
        listEl.scrollTop = item.offsetTop;
      } else if (item.offsetTop + item.offsetHeight > listEl.scrollTop + listEl.clientHeight) {
        listEl.scrollTop = item.offsetTop + item.offsetHeight - listEl.clientHeight;
      }
    });
  }

  function onTextFilterKeydown(event: KeyboardEvent) {
    if (event.isComposing || event.keyCode === 229) return;
    const isPath = options.filterPopover.key === 'path';
    if (isPath && (event.key === 'ArrowDown' || event.key === 'ArrowUp')) {
      event.preventDefault();
      if (!pathSuggestOpen.value) {
        pathSuggestOpen.value = true;
        pathSuggestIndex.value = -1;
        ensureCurrentPathLevel();
      }
      const len = pathSuggestMatches.value.list.length;
      if (len === 0) return;
      const step = event.key === 'ArrowDown' ? 1 : -1;
      pathSuggestIndex.value =
        pathSuggestIndex.value < 0
          ? step === 1
            ? 0
            : len - 1
          : (pathSuggestIndex.value + step + len) % len;
      scrollPathSuggestIntoView();
      return;
    }
    if (isPath && event.key === 'Tab' && !event.shiftKey && pathSuggestVisible.value) {
      event.preventDefault();
      const list = pathSuggestMatches.value.list;
      const picked = list[pathSuggestIndex.value] ?? list[0];
      if (picked) pickPathSuggestion(picked);
      return;
    }
    if (event.key === 'Enter') {
      event.preventDefault();
      const picked =
        isPath && pathSuggestVisible.value && pathSuggestIndex.value >= 0
          ? pathSuggestMatches.value.list[pathSuggestIndex.value]
          : undefined;
      if (picked) {
        pickPathSuggestion(picked);
        return;
      }
      options.hideFilterPopover();
      return;
    }
    if (event.key === 'Escape') {
      event.preventDefault();
      if (isPath && pathSuggestVisible.value) {
        event.stopPropagation();
        closePathSuggest();
        return;
      }
      event.stopPropagation();
      options.hideFilterPopover();
    }
  }

  return {
    pathSuggestIndex,
    pathSuggestMatches,
    pathSuggestVisible,
    pathSuggestLoadingVisible,
    rebasePathFilterToStack,
    followPathFilterBase,
    resolveAbsoluteDirStack,
    invalidatePathLevels,
    invalidatePathFollow,
    ensureCurrentPathLevel,
    closePathSuggest,
    onPathSuggestClick,
    onTextFilterInput,
    onTextFilterFocus,
    onTextFilterKeydown,
  };
}
