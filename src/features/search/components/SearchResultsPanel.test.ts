// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, nextTick, type App } from 'vue';
import SearchResultsPanel from './SearchResultsPanel.vue';
import type { GlobalSearchHistoryEntry } from '../../../lib/ipc/search';
import type { UnlistenFn } from '../../../lib/ipc/events';

const ipc = vi.hoisted(() => ({
  history: vi.fn(), start: vi.fn(), hits: vi.fn(), progress: vi.fn(), unlisten: vi.fn(), notify: vi.fn(),
}));
vi.mock('../../../lib/ipc/search', () => ({
  globalSearchHistoryList: ipc.history, globalSearchHitsPage: ipc.hits, globalSearchStart: ipc.start,
  onGlobalSearchProgress: ipc.progress, globalSearchExport: vi.fn(), globalSearchHistoryDelete: vi.fn(),
}));
vi.mock('../../../lib/ipc/fs/query', () => ({ fsChildren: vi.fn(async () => []) }));
vi.mock('../../../lib/ipc/taskRuntime', () => ({ taskCancel: vi.fn() }));
vi.mock('../../../composables/useToast', () => ({ notifyError: ipc.notify, notifyInfo: vi.fn(), notifyExportSuccess: vi.fn() }));

const runningTask: GlobalSearchHistoryEntry = {
  search_id: 'search-a', image_id: 'disk-a', keywords: ['password'], regexes: [], codepages: ['UTF-8'],
  match_case: false, search_deleted: false, search_empty_space: false, calc_hash: false, scope_json: '{}',
  status: 'running', error: null, started_ms: 1, finished_ms: null, files_total: 10, files_scanned: 0,
  matched_file_count: 0, total_matches: 0, record_count: 0, read_errors: 0, truncated: false,
};
let app: App | null;
let warnings: ReturnType<typeof vi.spyOn>;
async function settle() { for (let index = 0; index < 12; index++) await nextTick(); }
async function mount() {
  const host = document.createElement('div');
  document.body.append(host);
  app = createApp(SearchResultsPanel, { imageId: 'disk-a' });
  app.mount(host);
  await settle();
}
function unmount() { app?.unmount(); app = null; }
function progress() {
  ipc.progress.mock.calls[0][0]({ payload: {
    image_id: 'disk-a', search_id: 'search-a', files_done: 2, files_total: 10, hits: 0, matched_files: 0,
  } });
}
beforeEach(() => {
  vi.resetAllMocks();
  vi.useFakeTimers();
  warnings = vi.spyOn(console, 'warn').mockImplementation(() => {});
  ipc.history.mockResolvedValue([]);
  ipc.hits.mockResolvedValue({ search_id: 'search-a', items: [], total: 0 });
  ipc.progress.mockResolvedValue(ipc.unlisten);
});
afterEach(() => {
  unmount();
  document.body.replaceChildren();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('global search subscription lifecycle', () => {
  it('registers cleanup in setup without Vue lifecycle warnings', async () => {
    await mount();
    expect(warnings.mock.calls.flat().join(' ')).not.toContain('no active component instance');
    expect(ipc.unlisten).not.toHaveBeenCalled();
    unmount();
    expect(ipc.unlisten).toHaveBeenCalledOnce();
  });

  it('releases a subscription that resolves after the component is unmounted', async () => {
    let resolve!: (cleanup: UnlistenFn) => void;
    ipc.progress.mockReturnValueOnce(new Promise<UnlistenFn>(finish => { resolve = finish; }));
    await mount();
    unmount();
    resolve(ipc.unlisten);
    await settle();
    expect(ipc.unlisten).toHaveBeenCalledOnce();
    expect(warnings.mock.calls.flat().join(' ')).not.toContain('no active component instance');
  });

  it('reports subscription failure without an unhandled rejection', async () => {
    ipc.progress.mockRejectedValueOnce(new Error('event service unavailable'));
    await mount();
    expect(ipc.notify).toHaveBeenCalledExactlyOnceWith('订阅搜索进度失败', 'Error: event service unavailable');
  });

  it('cancels queued polling and ignores late events after unmount', async () => {
    ipc.history.mockResolvedValue([runningTask]);
    await mount();
    expect(ipc.hits).toHaveBeenCalledOnce();
    ipc.hits.mockClear();
    progress();
    unmount();
    progress();
    await vi.advanceTimersByTimeAsync(500);
    expect(ipc.unlisten).toHaveBeenCalledOnce();
    expect(ipc.hits).not.toHaveBeenCalled();
  });

  it('keeps progress polling active while mounted and reports read errors', async () => {
    ipc.history.mockResolvedValue([runningTask]);
    await mount();
    ipc.hits.mockClear();
    ipc.hits.mockRejectedValueOnce(new Error('read failed'));
    progress();
    await vi.advanceTimersByTimeAsync(200);
    expect(ipc.hits).toHaveBeenCalledOnce();
    expect(document.querySelector('.result-read-error')?.textContent).toContain('read failed');
    expect(document.body.textContent).not.toContain('本次搜索没有命中');
    progress();
    await vi.advanceTimersByTimeAsync(200);
    expect(ipc.hits).toHaveBeenCalledTimes(2);
    expect(document.querySelector('.result-read-error')).toBeNull();
  });

  it('does not report a delayed history error after unmount', async () => {
    let reject!: (error: Error) => void;
    ipc.history.mockReturnValueOnce(new Promise((_resolve, fail) => { reject = fail; }));
    await mount();
    unmount();
    reject(new Error('late history error'));
    await settle();
    expect(ipc.notify).not.toHaveBeenCalled();
  });
});


describe('search history visibility', () => {
  it('starts collapsed when there are no search records', async () => {
    await mount();
    expect((document.querySelector('.task-pane') as HTMLElement).style.display).toBe('none');
  });

  it('automatically opens existing history and keeps a manual collapse during progress refresh', async () => {
    ipc.history.mockResolvedValue([runningTask]);
    await mount();
    const pane = document.querySelector('.task-pane') as HTMLElement;
    expect(pane.style.display).not.toBe('none');
    (document.querySelector('[aria-label="收起搜索记录"]') as HTMLButtonElement).click();
    await settle();
    await vi.advanceTimersByTimeAsync(500);
    expect(pane.style.display).toBe('none');
    progress();
    await vi.advanceTimersByTimeAsync(500);
    expect(pane.style.display).toBe('none');
  });
});


it('opens the sidebar when the first search record is created', async () => {
  await mount();
  (document.querySelector('.workspace-start .primary') as HTMLButtonElement).click(); await settle();
  const keywords = document.querySelector('textarea') as HTMLTextAreaElement;
  keywords.value = 'password'; keywords.dispatchEvent(new Event('input')); await settle();
  ipc.start.mockImplementation(async ({ searchId }: { searchId: string }) => {
    ipc.history.mockResolvedValue([{ ...runningTask, search_id: searchId, status: 'success' }]);
    return { status: 'success', record_count: 0 };
  });
  (document.querySelector('.gs-modal-footer .primary') as HTMLButtonElement).click(); await settle();
  expect(ipc.start).toHaveBeenCalledOnce();
  expect((document.querySelector('.task-pane') as HTMLElement).style.display).not.toBe('none');
  expect(document.querySelector('.task-title')?.textContent).toContain('password');
});

it('offers retry for a completed search read without claiming it had no hits', async () => {
  ipc.history.mockResolvedValue([{ ...runningTask, status: 'success' }]);
  ipc.hits.mockRejectedValueOnce(new Error('结果不可读')); await mount();
  expect(document.querySelector('.result-read-error')?.textContent).toContain('结果不可读');
  expect(document.body.textContent).not.toContain('本次搜索没有命中');
  (document.querySelector('.result-read-error button') as HTMLButtonElement).click(); await settle();
  expect(document.querySelector('.result-read-error')).toBeNull();
  expect(document.body.textContent).toContain('本次搜索没有命中');
});

it('renders an indeterminate fill when a running search has no progress total', async () => {
  ipc.history.mockResolvedValue([runningTask]); await mount();
  expect(document.querySelector('.banner-progress.indeterminate > span')).not.toBeNull();
  expect(document.querySelector('.task-progress.indeterminate > span')).not.toBeNull();
  expect(document.querySelector('[role="progressbar"]')?.hasAttribute('aria-valuenow')).toBe(false);
});

it('retries an unreadable history from the landing page', async () => {
  ipc.history.mockRejectedValueOnce(new Error('记录不可读')); await mount();
  expect(document.querySelector('.workspace-start')?.textContent).toContain('记录不可读');
  expect(document.body.textContent).not.toContain('还没有搜索记录');
  (document.querySelector('.workspace-start .primary') as HTMLButtonElement).click(); await settle();
  expect(document.querySelector('.workspace-start')?.textContent).toContain('还没有搜索记录');
});
