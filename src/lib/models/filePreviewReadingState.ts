/** Reading context carried when a file preview moves between windows. */
import type { ImageId, NodeId } from '../ipc/ids';
export interface FilePreviewReadingState {
  imageId: ImageId;
  nodeId: NodeId;
  tab: 'detail' | 'raw' | 'text' | 'hex';
  searchQuery: string;
  encoding: string;
  page: number;
  pageSize: number;
  currentMatch: number;
  scrollTop: number;
  scrollLeft: number;
  codeScrollTop: number;
  codeScrollLeft: number;
}

/** Window URLs and event payloads must not supply invalid paging or scroll values. */
export function parseFilePreviewReadingState(value: unknown): FilePreviewReadingState | undefined {
  if (!value || typeof value !== 'object') return undefined;
  const state = value as Record<string, unknown>;
  if (typeof state.imageId !== 'string' || typeof state.nodeId !== 'string' ||
      typeof state.tab !== 'string' || !['detail', 'raw', 'text', 'hex'].includes(state.tab) ||
      typeof state.searchQuery !== 'string' || typeof state.encoding !== 'string' ||
      !Number.isSafeInteger(state.page) || Number(state.page) < 1 ||
      typeof state.pageSize !== 'number' || ![1024, 2048, 4096, 8192].includes(state.pageSize) ||
      !Number.isSafeInteger(state.currentMatch) || Number(state.currentMatch) < -1 ||
      ['scrollTop', 'scrollLeft', 'codeScrollTop', 'codeScrollLeft'].some(key =>
        typeof state[key] !== 'number' || !Number.isFinite(state[key]) || Number(state[key]) < 0)) return undefined;
  return {
    imageId: state.imageId, nodeId: state.nodeId, tab: state.tab as FilePreviewReadingState['tab'],
    searchQuery: state.searchQuery, encoding: state.encoding,
    page: Number(state.page), pageSize: Number(state.pageSize), currentMatch: Number(state.currentMatch),
    scrollTop: Number(state.scrollTop), scrollLeft: Number(state.scrollLeft),
    codeScrollTop: Number(state.codeScrollTop), codeScrollLeft: Number(state.codeScrollLeft),
  };
}
