import { nextTick, onScopeDispose, watch, type Ref } from 'vue';
import { parseFilePreviewReadingState, type FilePreviewReadingState } from '../../../lib/models/filePreviewReadingState';
import type { ImageId, NodeId } from '../../../lib/ipc/ids';

/** Restores reading context after asynchronous bytes, text and native viewers are ready. */
export function usePreviewReadingState(options: {
  imageId: () => ImageId;
  nodeId: () => NodeId;
  initial: () => FilePreviewReadingState | undefined;
  body: Ref<HTMLElement | null>;
  tab: Ref<FilePreviewReadingState['tab']>;
  searchQuery: Ref<string>;
  encoding: Ref<string>;
  page: Ref<number>;
  pageSize: Ref<number>;
  currentMatch: Ref<number>;
  matchCount: Ref<number>;
  totalPages: Ref<number>;
  ready: () => boolean;
  codeView: () => unknown;
  restoreMatch: (index: number) => void;
}) {
  let generation = 0;
  let pending: FilePreviewReadingState | undefined;
  let disposed = false;
  watch(() => [options.imageId(), options.nodeId(), options.initial()], () => {
    generation++;
    const state = parseFilePreviewReadingState(options.initial());
    pending = state?.imageId === options.imageId() && state.nodeId === options.nodeId() ? state : undefined;
    if (!pending) return;
    options.tab.value = pending.tab;
    options.searchQuery.value = pending.searchQuery;
    options.encoding.value = pending.encoding;
    options.pageSize.value = pending.pageSize;
    options.page.value = pending.page;
  }, { immediate: true, flush: 'post' });

  watch(() => [options.ready(), options.body.value, options.codeView(), options.tab.value, options.searchQuery.value, options.totalPages.value, options.matchCount.value], async () => {
    const state = pending;
    const request = generation;
    if (!state) return;
    if (options.tab.value !== state.tab || options.searchQuery.value !== state.searchQuery) { pending = undefined; return; }
    if (!options.ready()) return;
    await nextTick();
    if (disposed || request !== generation || pending !== state) return;
    const body = options.body.value;
    if (!body) return;
    const code = body.querySelector<HTMLElement>('.cm-scroller');
    if (body.querySelector('.code-view') && !code) return;
    // CSV and Markdown publish searchable matches after their own async rendering.
    if (state.currentMatch >= options.matchCount.value && state.currentMatch >= 0) return;
    options.restoreMatch(state.currentMatch);
    options.page.value = Math.min(state.page, options.totalPages.value);
    await nextTick();
    if (disposed || request !== generation || pending !== state) return;
    body.scrollTop = state.scrollTop;
    body.scrollLeft = state.scrollLeft;
    if (code) { code.scrollTop = state.codeScrollTop; code.scrollLeft = state.codeScrollLeft; }
    pending = undefined;
  }, { immediate: true, flush: 'post' });

  onScopeDispose(() => { disposed = true; generation++; pending = undefined; });
  function getReadingState(): FilePreviewReadingState {
    if (pending) return { ...pending };
    const body = options.body.value;
    const code = body?.querySelector<HTMLElement>('.cm-scroller');
    return {
      imageId: options.imageId(), nodeId: options.nodeId(), tab: options.tab.value,
      searchQuery: options.searchQuery.value, encoding: options.encoding.value,
      page: options.page.value, pageSize: options.pageSize.value, currentMatch: options.currentMatch.value,
      scrollTop: body?.scrollTop ?? 0, scrollLeft: body?.scrollLeft ?? 0,
      codeScrollTop: code?.scrollTop ?? 0, codeScrollLeft: code?.scrollLeft ?? 0,
    };
  }
  return { getReadingState };
}
