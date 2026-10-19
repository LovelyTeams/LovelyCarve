// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { effectScope, nextTick, ref } from 'vue';
import { useBackgroundEvidenceLoads } from './useBackgroundEvidenceLoads';
import { __resetTabManagerForTesting, initTabManagerHooks, useTabManager } from './useTabManager';

const ipc = vi.hoisted(() => ({ load: vi.fn(), cancel: vi.fn(), error: vi.fn() }));
vi.mock('../../lib/ipc/image', () => ({ imageLoad: ipc.load }));
vi.mock('../../lib/ipc/taskRuntime', () => ({ taskCancel: ipc.cancel }));
vi.mock('../../composables/useToast', () => ({ notifyError: ipc.error }));
let scope: ReturnType<typeof effectScope>;
const directory = ref<string | null>('case-a');
let complete: (value: { id: string }) => void;
let sessionLoad: ReturnType<typeof vi.fn>;
function setup() {
  const tabs = useTabManager();
  const flow = scope.run(() => useBackgroundEvidenceLoads(tabs, () => directory.value))!;
  return { tabs, flow };
}
async function settle() { for (let index = 0; index < 10; index++) await nextTick(); }
beforeEach(() => {
  vi.clearAllMocks();
  __resetTabManagerForTesting();
  directory.value = 'case-a';
  scope = effectScope();
  ipc.load.mockImplementation(() => new Promise(resolve => { complete = resolve; }));
  ipc.cancel.mockResolvedValue(undefined);
  sessionLoad = vi.fn(async () => ({ opened: true }));
  initTabManagerHooks({ onLoadFile: sessionLoad, onTeardown: vi.fn(), getActivePanel: () => '',
    setActivePanel: vi.fn(), isBusy: () => false, captureSession: () => ({}), restoreSession: vi.fn() });
});
afterEach(() => { scope.stop(); __resetTabManagerForTesting(); });

describe('background evidence loading', () => {
  it('returns with an active tab before extraction completes, then loads its content', async () => {
    const { tabs, flow } = setup();
    expect(await flow.open('D:/phone.zip', { forceImage: true })).toEqual({ ok: true, queued: true });
    const tab = tabs.activeTab.value!;
    expect(tab.filePath).toBe('D:/phone.zip');
    expect(ipc.load).toHaveBeenCalledExactlyOnceWith({ path: 'D:/phone.zip', taskId: tab.loadTaskId, caseDir: 'case-a' });
    expect(sessionLoad).not.toHaveBeenCalled();
    await flow.open('D:/phone.zip');
    expect(ipc.load).toHaveBeenCalledTimes(1);
    complete({ id: 'image-a' });
    await settle();
    expect(sessionLoad).toHaveBeenCalledExactlyOnceWith('D:/phone.zip', { forceImage: true });
    expect(tabs.activeTab.value?.loadTaskId).toBeUndefined();
  });

  it('cancels a closed tab and prevents its late result from opening contents', async () => {
    const { tabs, flow } = setup();
    await flow.open('D:/phone.zip');
    const tab = tabs.activeTab.value!;
    await tabs.closeTab(tab.id);
    await settle();
    expect(ipc.cancel).toHaveBeenCalledExactlyOnceWith({ taskId: tab.loadTaskId });
    complete({ id: 'image-a' });
    await settle();
    expect(sessionLoad).not.toHaveBeenCalled();
    expect(tabs.tabs.value).toEqual([]);
  });

  it('lets result navigation await content without blocking ordinary tab opening', async () => {
    const { tabs, flow } = setup();
    await flow.open('D:/phone.zip');
    let navigated = false;
    const navigation = flow.open('D:/phone.zip', { waitUntilReady: true }).then(result => { navigated = true; return result; });
    await settle();
    expect(navigated).toBe(false);
    complete({ id: 'image-a' });
    expect(await navigation).toEqual({ ok: true });
    expect(tabs.activeTab.value?.pendingLoad).toBe(false);
    expect(sessionLoad).toHaveBeenCalledTimes(1);
  });

  it('rejects late contents after switching away and back to the same case directory', async () => {
    const { tabs, flow } = setup();
    await flow.open('D:/phone.zip');
    directory.value = 'case-b';
    tabs.closeAllTabs();
    directory.value = 'case-a';
    complete({ id: 'image-a' });
    await settle();
    expect(sessionLoad).not.toHaveBeenCalled();
    expect(ipc.error).not.toHaveBeenCalled();
  });
});
