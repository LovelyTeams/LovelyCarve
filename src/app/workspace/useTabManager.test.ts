// @vitest-environment happy-dom
import { describe, it, expect, beforeEach } from 'vitest';
import { ref } from 'vue';
import {
  useTabManager,
  initTabManagerHooks,
  __resetTabManagerForTesting,
  type LoadFileOptions,
} from './useTabManager';

/**
 * Builds a fake hook bundle that records calls and lets each test
 * decide what `onLoadFile` should resolve to. The shape mirrors the
 * real workspace shell's contracts.
 */
function makeHookHarness(opts: {
  loadResult?: { opened: boolean; error?: string };
  panel?: string;
  busy?: boolean;
  resolveTabPath?: (path: string) => string;
} = {}) {
  const calls = {
    teardown: 0,
    load: [] as string[],
    loadOptions: [] as (LoadFileOptions | undefined)[],
    setPanel: [] as string[],
    capture: 0,
    restore: 0,
  };
  let panel = opts.panel ?? 'overview';
  let busy = opts.busy ?? false;
  // The "live state" we capture / restore — small but distinct per
  // load() call so tests can assert which snapshot was put back.
  let liveState: { path: string; counter: number } = { path: '', counter: 0 };

  initTabManagerHooks({
    resolveTabPath: opts.resolveTabPath,
    onTeardown: () => { calls.teardown += 1; liveState = { path: '', counter: 0 }; },
    onLoadFile: async (filePath, loadOptions) => {
      calls.load.push(filePath);
      calls.loadOptions.push(loadOptions);
      liveState = { path: filePath, counter: calls.load.length };
      return opts.loadResult ?? { opened: true };
    },
    getActivePanel: () => panel,
    setActivePanel: (next) => { panel = next; calls.setPanel.push(next); },
    isBusy: () => busy,
    captureSession: () => {
      calls.capture += 1;
      // Return a snapshot of the current liveState so the test can
      // verify that switching back restores exactly this object.
      return { ...liveState };
    },
    restoreSession: (snap) => {
      calls.restore += 1;
      liveState = { ...(snap as typeof liveState) };
    },
  });

  return {
    calls,
    setPanel: (p: string) => { panel = p; },
    setBusy: (b: boolean) => { busy = b; },
    panel: () => panel,
    liveState: () => liveState,
  };
}

describe('useTabManager', () => {
  beforeEach(() => {
    __resetTabManagerForTesting();
  });

  it('starts empty', () => {
    const tm = useTabManager();
    expect(tm.tabs.value).toEqual([]);
    expect(tm.activeTabId.value).toBeNull();
    expect(tm.activeTab.value).toBeNull();
  });

  it('opens a background tab immediately and loads contents only when ready', async () => {
    const harness = makeHookHarness();
    const tabs = useTabManager();
    const id = await tabs.beginBackgroundLoad('phone.zip', 'unpack-1', { forceImage: true });
    expect(tabs.activeTab.value).toMatchObject({ id, loadTaskId: 'unpack-1' });
    expect(tabs.isSwitching.value).toBe(false);
    expect(harness.calls.load).toEqual([]);
    await tabs.finishBackgroundLoad(id);
    expect(harness.calls.load).toEqual(['phone.zip']);
    expect(tabs.activeTab.value).toMatchObject({ id, pendingLoad: false, loadTaskId: undefined });
  });

  it('keeps the previous session intact when switching around an extracting tab', async () => {
    const harness = makeHookHarness();
    const tabs = useTabManager();
    await tabs.openNewTab('disk.E01');
    const disk = tabs.activeTabId.value!;
    const zip = await tabs.beginBackgroundLoad('phone.zip', 'unpack-1');
    await tabs.activateTab(disk);
    expect(harness.liveState().path).toBe('disk.E01');
    await tabs.activateTab(zip);
    await tabs.activateTab(disk);
    expect(harness.liveState().path).toBe('disk.E01');
    await tabs.finishBackgroundLoad(zip);
    expect(tabs.activeTabId.value).toBe(disk);
    expect(harness.calls.load).toEqual(['disk.E01']);
    await tabs.activateTab(zip);
    expect(harness.calls.load).toEqual(['disk.E01', 'phone.zip']);
  });

  it('ignores completion of a closed background tab', async () => {
    const harness = makeHookHarness();
    const tabs = useTabManager();
    const id = await tabs.beginBackgroundLoad('phone.zip', 'unpack-1');
    tabs.closeAllTabs();
    await tabs.finishBackgroundLoad(id);
    expect(tabs.tabs.value).toEqual([]);
    expect(harness.calls.load).toEqual([]);
  });

  describe('grouped evidence tabs', () => {
    it('hides members without dropping their records and numbers only visible tabs', async () => {
      const aliases = ref(new Map<string, string>());
      const harness = makeHookHarness({ resolveTabPath: path => aliases.value.get(path) ?? path });
      const manager = useTabManager();
      const members = ['a.dd', 'b.dd', 'c.dd', 'd.dd'];
      for (const path of [...members, 'assembled://set', 'ordinary.E01']) await manager.openNewTab(path);
      aliases.value = new Map(members.map(path => [path, 'assembled://set']));
      expect(manager.tabs.value).toHaveLength(6);
      expect(manager.visibleTabs.value.map(tab => tab.filePath)).toEqual(['assembled://set', 'ordinary.E01']);
      await manager.activateTabByIndex(1);
      expect(manager.activeTab.value?.filePath).toBe('assembled://set');
      await manager.cycleTab(1);
      expect(manager.activeTab.value?.filePath).toBe('ordinary.E01');
      await manager.cycleTab(-1);
      expect(manager.activeTab.value?.filePath).toBe('assembled://set');
      const loaded = harness.calls.load.length;
      await manager.openNewTab('a.dd');
      expect(harness.calls.load).toHaveLength(loaded);
      expect(manager.activeTab.value?.filePath).toBe('assembled://set');
    });

    it('returns to the welcome state after closing the sole visible array and reopens members through it', async () => {
      const aliases = ref(new Map<string, string>());
      const harness = makeHookHarness({ resolveTabPath: path => aliases.value.get(path) ?? path });
      const manager = useTabManager();
      await manager.openNewTab('member.dd');
      const memberId = manager.activeTabId.value!;
      await manager.openNewTab('assembled://set');
      aliases.value = new Map([['member.dd', 'assembled://set']]);
      await manager.closeTab(manager.activeTabId.value!);
      expect(manager.visibleTabs.value).toEqual([]);
      expect(manager.activeTabId.value).toBeNull();
      expect(manager.tabs.value.map(tab => tab.id)).toEqual([memberId]);
      await manager.openNewTab('member.dd');
      expect(harness.calls.load).toEqual(['member.dd', 'assembled://set', 'assembled://set']);
      expect(manager.visibleTabs.value.map(tab => tab.filePath)).toEqual(['assembled://set']);
      await manager.activateTab(memberId);
      expect(manager.activeTab.value?.filePath).toBe('assembled://set');
    });
  });

  describe('openNewTab', () => {
    it('uses evidence display names for queued RAID tabs while retaining their source paths', async () => {
      makeHookHarness({ busy: true });
      const manager = useTabManager();
      const path = 'assembled://md:auto-raid5-59494dc69af24768b6a690819d35420e';
      await manager.openNewTab(path);
      await manager.openNewTab('assembled://md:custom-array');
      expect(manager.tabs.value.map(tab => [tab.fileName, tab.filePath, tab.pendingLoad])).toEqual([
        ['RAID 5 重组镜像', path, true],
        ['RAID 重组镜像', 'assembled://md:custom-array', true],
      ]);
    });

    it('inserts a tab and activates it on successful load', async () => {
      const h = makeHookHarness();
      const tm = useTabManager();

      await tm.openNewTab('/exp/A.pcap');

      expect(tm.tabs.value).toHaveLength(1);
      expect(tm.tabs.value[0].fileName).toBe('A.pcap');
      expect(tm.tabs.value[0].mode).toBe('image');
      expect(tm.activeTabId.value).toBe(tm.tabs.value[0].id);
      expect(h.calls.load).toEqual(['/exp/A.pcap']);
      // Single teardown — first open has nothing to tear down before
      // the load, but the manager still calls it for symmetry.
      expect(h.calls.teardown).toBe(1);
    });

    it('rolls the tab back out when load fails', async () => {
      const h = makeHookHarness({ loadResult: { opened: false } });
      const tm = useTabManager();

      await tm.openNewTab('/exp/missing.pcap');

      expect(tm.tabs.value).toEqual([]);
      expect(tm.activeTabId.value).toBeNull();
      expect(h.calls.load).toEqual(['/exp/missing.pcap']);
    });

    it('returns the load failure reason and forwards the quiet flag', async () => {
      const h = makeHookHarness({ loadResult: { opened: false, error: '镜像格式损坏' } });
      const tm = useTabManager();

      const result = await tm.openNewTab('/exp/broken.E01', { forceImage: true, quiet: true });

      expect(result).toEqual({ ok: false, error: '镜像格式损坏' });
      expect(h.calls.loadOptions).toEqual([{ forceImage: true, quiet: true }]);
      expect(tm.tabs.value).toEqual([]);
    });

    it('reports success for opened, already-open and deferred tabs', async () => {
      const h = makeHookHarness();
      const tm = useTabManager();

      expect(await tm.openNewTab('/exp/A.E01')).toEqual({ ok: true });
      expect(await tm.openNewTab('/exp/A.E01')).toEqual({ ok: true });
      h.setBusy(true);
      expect(await tm.openNewTab('/exp/B.E01')).toEqual({ ok: true });
      expect(tm.tabs.value.map(tab => [tab.fileName, tab.pendingLoad ?? false])).toEqual([
        ['A.E01', false],
        ['B.E01', true],
      ]);
    });

    it('refuses duplicates and re-activates the existing tab', async () => {
      const h = makeHookHarness();
      const tm = useTabManager();

      await tm.openNewTab('/exp/A.pcap');
      await tm.openNewTab('/exp/B.pcap');
      expect(tm.tabs.value).toHaveLength(2);
      expect(tm.activeTabId.value).toBe(tm.tabs.value[1].id);

      // Open A again — should activate the original tab, not stack.
      await tm.openNewTab('/exp/A.pcap');
      expect(tm.tabs.value).toHaveLength(2);
      expect(tm.tabs.value[0].fileName).toBe('A.pcap');
      expect(tm.activeTabId.value).toBe(tm.tabs.value[0].id);
      // With the LRU snapshot cache, switching back to A is served
      // from memory rather than re-loading. Only the two original
      // first-opens should appear in the load log.
      expect(h.calls.load).toEqual(['/exp/A.pcap', '/exp/B.pcap']);
      expect(h.calls.restore).toBeGreaterThan(0);
    });

    it('inserts a deferred tab while a long-running analysis is in flight', async () => {
      // Old behaviour was to refuse the open outright, leaving the user
      // with no way to queue a second pcap. The new contract: the chip
      // appears in the bar marked `pendingLoad: true`, but no teardown
      // or onLoadFile fires until the user actually activates it.
      const h = makeHookHarness({ busy: true });
      const tm = useTabManager();

      await tm.openNewTab('/exp/A.pcap');
      expect(tm.tabs.value).toHaveLength(1);
      expect(tm.tabs.value[0].pendingLoad).toBe(true);
      expect(tm.activeTabId.value).toBeNull();
      expect(h.calls.load).toEqual([]);
      expect(h.calls.teardown).toBe(0);
    });

    it('clears the pendingLoad flag when the user activates the deferred tab after busy clears', async () => {
      const h = makeHookHarness({ busy: true });
      const tm = useTabManager();

      await tm.openNewTab('/exp/A.pcap');
      expect(tm.tabs.value[0].pendingLoad).toBe(true);

      // Analysis finishes, busy clears. User clicks the deferred tab —
      // now the load actually fires and the flag drops.
      h.setBusy(false);
      const idA = tm.tabs.value[0].id;
      await tm.activateTab(idA);

      expect(tm.activeTabId.value).toBe(idA);
      expect(tm.tabs.value[0].pendingLoad).toBe(false);
      expect(h.calls.load).toEqual(['/exp/A.pcap']);
    });
  });

  describe('activateTab', () => {
    it('snapshots the previous tab\'s active panel and restores the next', async () => {
      const h = makeHookHarness();
      const tm = useTabManager();

      // Open A, simulate user navigating to "http" panel
      await tm.openNewTab('/exp/A.pcap');
      const idA = tm.tabs.value[0].id;
      h.setPanel('http');

      // Open B, navigate to "dns"
      await tm.openNewTab('/exp/B.pcap');
      const idB = tm.tabs.value[1].id;
      h.setPanel('dns');

      // Switch back to A — should restore "http"
      await tm.activateTab(idA);
      expect(tm.activeTabId.value).toBe(idA);
      expect(h.panel()).toBe('http');

      // The B tab should still remember "dns"
      expect(tm.tabs.value.find(t => t.id === idB)?.lastActivePanel).toBe('dns');
    });

    it('is a no-op when activating the already-active tab', async () => {
      const h = makeHookHarness();
      const tm = useTabManager();

      await tm.openNewTab('/exp/A.pcap');
      const id = tm.tabs.value[0].id;
      const loadCountBefore = h.calls.load.length;

      await tm.activateTab(id);
      expect(h.calls.load.length).toBe(loadCountBefore);
    });
  });

  describe('closeTab', () => {
    it('removes the tab and switches to the most-recently-active remaining one', async () => {
      makeHookHarness();
      const tm = useTabManager();

      await tm.openNewTab('/exp/A.pcap');
      await tm.openNewTab('/exp/B.pcap');
      await tm.openNewTab('/exp/C.pcap');
      const idA = tm.tabs.value[0].id;
      const idC = tm.tabs.value[2].id;

      // Activate A so its lastActiveAt becomes the most recent, then
      // switch back to C.
      await tm.activateTab(idA);
      await tm.activateTab(idC);

      // Closing C → A should win over B (A was activated more recently).
      await tm.closeTab(idC);
      expect(tm.tabs.value).toHaveLength(2);
      expect(tm.activeTabId.value).toBe(idA);
    });

    it('clears the active tab and tears down state when closing the last tab', async () => {
      const h = makeHookHarness();
      const tm = useTabManager();

      await tm.openNewTab('/exp/A.pcap');
      const id = tm.tabs.value[0].id;
      const teardownsBefore = h.calls.teardown;

      await tm.closeTab(id);
      expect(tm.tabs.value).toEqual([]);
      expect(tm.activeTabId.value).toBeNull();
      // One extra teardown when the last tab goes away — the welcome
      // screen should render against a freshly-cleared state.
      expect(h.calls.teardown).toBe(teardownsBefore + 1);
    });

    it('keeps the active tab unchanged when closing a non-active tab', async () => {
      makeHookHarness();
      const tm = useTabManager();

      await tm.openNewTab('/exp/A.pcap');
      await tm.openNewTab('/exp/B.pcap');
      const idA = tm.tabs.value[0].id;
      const idB = tm.tabs.value[1].id;

      // B is currently active; closing A shouldn't disturb that.
      await tm.closeTab(idA);
      expect(tm.tabs.value).toHaveLength(1);
      expect(tm.activeTabId.value).toBe(idB);
    });
  });

  describe('cycleTab / activateTabByIndex', () => {
    it('cycleTab(+1) wraps from the last tab back to the first', async () => {
      makeHookHarness();
      const tm = useTabManager();

      await tm.openNewTab('/exp/A.pcap');
      await tm.openNewTab('/exp/B.pcap');
      const idA = tm.tabs.value[0].id;
      // Active is B; +1 should wrap to A
      await tm.cycleTab(1);
      expect(tm.activeTabId.value).toBe(idA);
    });

    it('activateTabByIndex uses 1-based indexing and ignores out-of-range', async () => {
      makeHookHarness();
      const tm = useTabManager();

      await tm.openNewTab('/exp/A.pcap');
      await tm.openNewTab('/exp/B.pcap');
      await tm.openNewTab('/exp/C.pcap');

      await tm.activateTabByIndex(2);
      expect(tm.activeTabId.value).toBe(tm.tabs.value[1].id);

      // Out of range: state unchanged
      const before = tm.activeTabId.value;
      await tm.activateTabByIndex(99);
      expect(tm.activeTabId.value).toBe(before);
    });
  });

  describe('snapshot LRU', () => {
    it('switching between 2 tabs restores from snapshot — no second load', async () => {
      const h = makeHookHarness();
      const tm = useTabManager();

      await tm.openNewTab('/exp/A.pcap');
      await tm.openNewTab('/exp/B.pcap');
      const idA = tm.tabs.value[0].id;

      // Now switch back to A — should HIT the snapshot cache, not call onLoadFile again.
      const loadsBefore = h.calls.load.length;
      await tm.activateTab(idA);
      expect(h.calls.load.length).toBe(loadsBefore); // no new load
      expect(h.calls.restore).toBeGreaterThan(0);
      expect(h.liveState().path).toBe('/exp/A.pcap');
    });

    it('beyond LRU(5) evicts the oldest tab\'s snapshot — switching to it reloads', async () => {
      const h = makeHookHarness();
      const tm = useTabManager();

      // Open 6 tabs in sequence. Each open stashes the previous tab's
      // snapshot. After tab 6, we expect tab 1's snapshot to have been
      // evicted (it has the smallest lastActiveSeq among snapshotted).
      for (let i = 1; i <= 6; i++) {
        await tm.openNewTab(`/exp/tab${i}.pcap`);
      }

      const idTab1 = tm.tabs.value[0].id;
      const loadsBefore = h.calls.load.length;

      // Switch to tab1 — its snapshot should have been evicted, so the
      // tab manager falls through to onLoadFile.
      await tm.activateTab(idTab1);
      expect(h.calls.load.length).toBeGreaterThan(loadsBefore);
      expect(h.liveState().path).toBe('/exp/tab1.pcap');
    });

    it('tabs within LRU(5) all stay snapshot-resident', async () => {
      const h = makeHookHarness();
      const tm = useTabManager();

      // Open exactly 5 tabs.
      for (let i = 1; i <= 5; i++) {
        await tm.openNewTab(`/exp/tab${i}.pcap`);
      }

      // Walk back through tabs 4, 3, 2, 1 — each should restore from
      // snapshot without re-loading.
      const loadsBefore = h.calls.load.length;
      for (let i = 4; i >= 1; i--) {
        const id = tm.tabs.value[i - 1].id;
        await tm.activateTab(id);
      }
      expect(h.calls.load.length).toBe(loadsBefore);
      expect(h.liveState().path).toBe('/exp/tab1.pcap');
    });

    it('closing a tab drops its snapshot from the cache', async () => {
      const h = makeHookHarness();
      const tm = useTabManager();

      await tm.openNewTab('/exp/A.pcap');
      await tm.openNewTab('/exp/B.pcap');
      const idA = tm.tabs.value[0].id;

      await tm.closeTab(idA);
      expect(tm.tabs.value).toHaveLength(1);

      // Re-opening A should be a fresh load, not a snapshot restore.
      const restoresBefore = h.calls.restore;
      await tm.openNewTab('/exp/A.pcap');
      expect(h.calls.restore).toBe(restoresBefore);
      expect(h.calls.load).toContain('/exp/A.pcap');
    });
  });

  describe('mode detection', () => {
    it.each([
      ['/x/foo.pcap', 'image'],
      ['/x/foo.pcapng', 'image'],
      ['/x/foo.cap', 'image'],
      ['/x/foo.log', 'image'],
      ['/x/foo.txt', 'image'],
    ])('%s -> %s', async (path, expected) => {
      makeHookHarness();
      const tm = useTabManager();
      await tm.openNewTab(path);
      expect(tm.tabs.value[0].mode).toBe(expected);
    });
  });
});
