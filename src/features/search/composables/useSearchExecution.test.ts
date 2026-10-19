// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { effectScope, nextTick, ref, type EffectScope } from 'vue';
import { useSearchExecution } from './useSearchExecution';
import { buildSearchOptions, createSearchDraft } from '../model/searchConfiguration';
import type { GlobalSearchSummary } from '../../../lib/ipc/search';
const calls = vi.hoisted(() => ({ start: vi.fn(), subscribe: vi.fn(), cleanup: vi.fn(), error: vi.fn(), info: vi.fn() }));
vi.mock('../../../lib/ipc/search', () => ({ globalSearchStart: calls.start, onGlobalSearchProgress: calls.subscribe }));
vi.mock('../../../composables/useToast', () => ({ notifyError: calls.error, notifyInfo: calls.info }));
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; }
let scope: EffectScope; const image = ref('a'); const selected = ref<string | null>('search');
const refresh = vi.fn(async () => {}); const accept = vi.fn(() => true); const poll = vi.fn(async () => {}); const load = vi.fn(async () => {});
function setup() { scope = effectScope(); return scope.run(() => useSearchExecution({ imageId: () => image.value, selectedSearchId: () => selected.value, refreshTasks: refresh, acceptProgress: accept, pollIncremental: poll, loadFirstPage: load }))!; }
async function settle() { for (let index = 0; index < 8; index++) await nextTick(); }
beforeEach(() => { vi.resetAllMocks(); vi.useFakeTimers(); image.value = 'a'; selected.value = 'search'; accept.mockReturnValue(true); calls.subscribe.mockResolvedValue(calls.cleanup); });
afterEach(() => { scope?.stop(); vi.useRealTimers(); });
it('does not refresh a new A→B→A view when an old search completes', async () => {
  const completion = deferred<GlobalSearchSummary>(); calls.start.mockReturnValueOnce(completion.promise);
  const execution = setup(); execution.startSearch(buildSearchOptions(createSearchDraft()));
  expect(calls.start.mock.calls[0][0].imageId).toBe('a');
  image.value = 'b'; image.value = 'a'; completion.resolve({ status: 'cancelled', record_count: 1 } as GlobalSearchSummary);
  await settle(); await vi.advanceTimersByTimeAsync(500);
  expect(refresh).not.toHaveBeenCalled(); expect(load).not.toHaveBeenCalled(); expect(calls.info).not.toHaveBeenCalled();
});
it('drops a queued poll when the selection changes away and back', async () => {
  setup(); await settle(); calls.subscribe.mock.calls[0][0]({ payload: { image_id: 'a', search_id: 'search' } });
  selected.value = 'other'; selected.value = 'search'; await vi.advanceTimersByTimeAsync(200);
  expect(poll).not.toHaveBeenCalled();
  calls.subscribe.mock.calls[0][0]({ payload: { image_id: 'a', search_id: 'search' } });
  await vi.advanceTimersByTimeAsync(200); expect(poll).toHaveBeenCalledOnce();
});
it('releases a late subscription once and cannot schedule polling after disposal', async () => {
  const subscription = deferred<() => void>(); calls.subscribe.mockReturnValueOnce(subscription.promise);
  setup(); scope.stop(); subscription.resolve(calls.cleanup); await settle();
  calls.subscribe.mock.calls[0][0]({ payload: { image_id: 'a', search_id: 'search' } }); await vi.advanceTimersByTimeAsync(500);
  expect(calls.cleanup).toHaveBeenCalledOnce(); expect(accept).not.toHaveBeenCalled(); expect(poll).not.toHaveBeenCalled();
});
