
import { tr } from '../../composables/useLocale';
/**
 * useTabManager — browser-style multi-tab orchestrator for evidence images.
 *
 * The composable owns the tabs[] list and the activeTabId, but holds no
 * file data of its own. When the user activates a different tab we call
 * back through service-locator hooks (registered once at App-level via
 * `initTabManagerHooks`) to drive the existing single-active-session
 * pipeline:
 *
 *     active_tab_A → snap activePanel → resetSession → openByPath(B) →
 *     restore activePanel from B's saved panel
 *
 * Why service-locator (rather than direct imports of the file-session
 * composables): `useFileSession` etc. are factory-called once at
 * AppShell.vue setup time, and the resulting refs are the canonical app
 * state. If TabManager imported `useFileSession` itself it would call
 * the factory a *second* time and end up with a parallel set of refs
 * disconnected from the rest of the app. Hooks dodge that without
 * forcing a global module-level singleton refactor of every file
 * session composable.
 *
 * The `mode` field is intentionally coarse: disk images and folder-backed
 * logical evidence use the image styling while the loaded session decides
 * the active panel.
 *
 * Persistence: none. Every app launch starts blank (matches the
 * decision in the implementation plan — the cost / benefit of
 * recovering "last session" tabs is lower priority than avoiding stale
 * file references after evidence moves on disk).
 */
import { computed, ref } from 'vue';
import { evidenceDisplayName } from '../../features/evidence/model/evidenceOverview';

export interface WorkspaceTab {
  id: string;            // crypto.randomUUID()
  filePath: string;      // absolute path; doubles as the backend cache key
  fileName: string;      // evidence display name for the chip label
  mode: 'image' | 'evidence';
  /** Panel the tab was on when the user last left it. Restored on
   *  re-activation so each tab "remembers where you were". */
  lastActivePanel: string;
  /** Monotonic activation sequence. Larger = more recently focused.
   *  Used to pick the next active tab when the current one is closed.
   *  We use a counter (not nextActivationSeq()) because rapid back-to-back
   *  activations can land in the same millisecond, which would make
   *  the "most recently used" tie-break non-deterministic. */
  lastActiveSeq: number;
  /** True if the tab was created while a previous analysis was still
   *  running. The chip appears in the bar but the backend load is
   *  deferred — we can't tear the in-flight session down without
   *  losing the user's progress. Cleared once the user clicks the tab
   *  (after the current analysis finishes) and the load actually runs. */
  pendingLoad?: boolean;
  /** Force the disk-image load path on (re)load regardless of extension.
   *  Set for folder-backed evidence (no disk-image extension) so a tab that
   *  gets reloaded after snapshot eviction still routes to `image_load`
   *  instead of extension-based rejection. */
  forceImage?: boolean;
}

let activationCounter = 0;
function nextActivationSeq(): number { return ++activationCounter; }

// ── Module-level singleton state ─────────────────────────────────────
//
// Multiple importers all see the same arrays/refs, no Pinia / Vuex needed.
// AppShell.vue is responsible for calling `initTabManagerHooks(...)` exactly once.
const tabs = ref<WorkspaceTab[]>([]);
const activeTabId = ref<string | null>(null);
const isSwitching = ref(false);

const activeTab = computed(
  () => tabs.value.find(t => t.id === activeTabId.value) ?? null,
);
/** Source tabs remain in the working set, but grouped paths share one visible tab. */
const visibleTabs = computed(() => tabs.value.filter(tab => (hooks?.resolveTabPath?.(tab.filePath) ?? tab.filePath) === tab.filePath));

// ── Service-locator hooks ───────────────────────────────────────────
/** `quiet` asks the load hook not to report a failure itself because the
 *  caller aggregates or annotates it (e.g. re-opening a whole case). */
export type LoadFileOptions = { forceImage?: boolean; quiet?: boolean };
type LoadFileFn = (filePath: string, opts?: LoadFileOptions) => Promise<{ opened: boolean; error?: string }>;
type Voider = () => void;
type GetActivePanelFn = () => string;
type SetActivePanelFn = (panelId: string) => void;
type IsBusyFn = () => boolean;

/** Opaque session snapshot — the tab manager doesn't read its
 *  contents, just stores and replays it. AppShell.vue bundles per-composable
 *  snapshots into this object. */
export type SessionSnapshot = unknown;

type CaptureSessionFn = () => SessionSnapshot;
type RestoreSessionFn = (snap: SessionSnapshot) => void;

interface TabManagerHooks {
  /** Redirect a source view to its parent evidence without unloading the source. */
  resolveTabPath?: (filePath: string) => string;
  /** Call the existing single-file open path. Caller is expected to
   *  have already torn down the prior session (we do that via
   *  `onTeardown` immediately before this). Returns whether the load
   *  actually populated state, plus a user-facing reason on failure. */
  onLoadFile: LoadFileFn;
  /** Reset every per-file ref + analyzer state + flag aggregator. */
  onTeardown: Voider;
  /** Read the current active panel id (for the "snapshot" step). */
  getActivePanel: GetActivePanelFn;
  /** Set the active panel id (for the "restore" step). */
  setActivePanel: SetActivePanelFn;
  /** True while a long-running analysis is in flight. We refuse to
   *  switch tabs in that state because the progress listener is a
   *  singleton — switching mid-analysis would route progress events
   *  for tab A into tab B's UI. */
  isBusy: IsBusyFn;
  /** Snapshot the current active tab's complete reactive state into
   *  an opaque object. Used to keep up to `SNAPSHOT_LRU_LIMIT` tabs
   *  hot in memory so users don't pay re-load cost on every switch. */
  captureSession: CaptureSessionFn;
  /** Restore a previously captured session; replaces `onLoadFile +
   *  onTeardown` for cache hits. */
  restoreSession: RestoreSessionFn;
}

let hooks: TabManagerHooks | null = null;

/** Per-tab session snapshots, keyed by `WorkspaceTab.id`. The map is
 *  bounded by `SNAPSHOT_LRU_LIMIT`: when adding the (limit+1)-th
 *  entry, the snapshot belonging to the least-recently-active tab
 *  is evicted. The evicted tab itself stays in `tabs[]` — only its
 *  cached snapshot is dropped, so the next click reloads from
 *  SQLite via `onLoadFile`. */
const snapshots = new Map<string, SessionSnapshot>();
const SNAPSHOT_LRU_LIMIT = 5;

function evictOldestSnapshot(): void {
  if (snapshots.size <= SNAPSHOT_LRU_LIMIT) return;
  // Pick the tab with the smallest lastActiveSeq among the tabs that
  // currently hold snapshots. Tabs missing from `tabs[]` shouldn't
  // happen (we delete on closeTab) but we treat them as eviction
  // candidates first to be safe.
  let oldestId: string | null = null;
  let oldestSeq = Infinity;
  for (const id of snapshots.keys()) {
    const tab = tabs.value.find(t => t.id === id);
    const seq = tab ? tab.lastActiveSeq : -1;
    if (seq < oldestSeq) {
      oldestSeq = seq;
      oldestId = id;
    }
  }
  if (oldestId) snapshots.delete(oldestId);
}

export function initTabManagerHooks(h: TabManagerHooks) {
  hooks = h;
  return () => { if (hooks === h) hooks = null; };
}

// ── Helpers ─────────────────────────────────────────────────────────
function deriveMode(_filePath: string): WorkspaceTab['mode'] {
  // Every loadable source in the current shell is routed through image_load:
  // disk images, folder evidence, physical disks, and synthetic RAID images.
  return 'image';
}

function snapshotActivePanel(): void {
  if (!hooks) return;
  const cur = tabs.value.find(t => t.id === activeTabId.value);
  if (!cur) return;
  cur.lastActivePanel = hooks.getActivePanel();
}

/** Capture the current active tab's full session into the LRU cache.
 *  Skipped if there's no active tab or the active tab has been
 *  removed from `tabs[]`. Called before every switch-away. */
function stashCurrentSnapshot(): void {
  if (!hooks) return;
  const cur = activeTabId.value;
  if (!cur) return;
  if (!tabs.value.some(t => t.id === cur)) return;
  snapshots.set(cur, hooks.captureSession());
  evictOldestSnapshot();
}

/** Outcome of `openNewTab`. A tab queued behind a running analysis
 *  (`pendingLoad`) or an already-open tab counts as success. */
export type OpenTabResult = { ok: true } | { ok: false; error: string };

// ── Public API ──────────────────────────────────────────────────────
async function openNewTab(filePath: string, opts?: LoadFileOptions): Promise<OpenTabResult> {
  if (!hooks) {
    console.warn('[useTabManager] hooks not initialised; openNewTab is a no-op');
    return { ok: false, error: tr("工作区尚未就绪") };
  }
  if (!filePath) return { ok: false, error: tr("路径为空") };
  filePath = hooks.resolveTabPath?.(filePath) ?? filePath;

  // De-dup: same path → activate the existing tab instead of stacking
  // a duplicate. This is what the user expects when they pick a file
  // they already have open from the file dialog or via Ctrl+O.
  const existing = tabs.value.find(t => t.filePath === filePath);
  if (existing) {
    await activateTab(existing.id);
    return tabs.value.some(t => t.id === existing.id)
      ? { ok: true }
      : { ok: false, error: tr("已打开的标签页重新载入失败") };
  }

  const mode = deriveMode(filePath);
  const newTab: WorkspaceTab = {
    id: typeof crypto !== 'undefined' && 'randomUUID' in crypto
      ? crypto.randomUUID()
      : `tab-${nextActivationSeq()}-${Math.random().toString(36).slice(2, 8)}`,
    filePath,
    fileName: evidenceDisplayName({ path: filePath, source_format: filePath.startsWith('assembled://') ? 'assembled' : '' }, tr),
    mode,
    lastActivePanel: '',
    lastActiveSeq: nextActivationSeq(),
    forceImage: opts?.forceImage,
  };

  // Analysis-in-flight path: we can't tear down the active session
  // without killing the user's running analysis, and the progress
  // listeners are app-singletons so we can't fan them out to a second
  // concurrent load. Instead of refusing the new tab (which leaves the
  // user with no way to queue a second image), insert it as a deferred
  // entry — the chip appears in the bar with a "未分析" hint, and the
  // load fires the moment the user clicks it after the current run
  // finishes. The active tab stays put.
  if (hooks.isBusy()) {
    newTab.pendingLoad = true;
    tabs.value = [...tabs.value, newTab];
    return { ok: true };
  }

  // Snapshot the panel of the tab we're leaving so we can restore it
  // when the user clicks back into it later, and stash its complete
  // session so the user can switch back without paying reload cost.
  snapshotActivePanel();
  stashCurrentSnapshot();

  // Insert the tab optimistically AND make it active before the load
  // begins, so the loading spinner (which renders on the active tab while
  // `isSwitching`) shows on the NEW tab — not the one we're leaving. We
  // remember the previous active id to restore it if the load fails.
  const previousActiveId = activeTabId.value;
  tabs.value = [...tabs.value, newTab];
  activeTabId.value = newTab.id;
  isSwitching.value = true;
  // Roll back a failed open: drop the new tab and re-point the active tab.
  // Prefer the tab that was active before, but it may have been closed during
  // the load (closeTab has no in-flight guard), so fall back to the
  // most-recently-used remaining tab — never leave activeTabId dangling on a
  // removed id (which would strand the UI on the welcome screen).
  // `onTeardown` already wiped the live session by the time a failure
  // surfaces, so also replay the re-activated tab's stashed snapshot —
  // without it the active chip sits over a blank workspace (clicking it is
  // a no-op: activateTab early-returns on the already-active id).
  const rollback = () => {
    tabs.value = tabs.value.filter(t => t.id !== newTab.id);
    let fallbackId: string | null;
    if (visibleTabs.value.some(t => t.id === previousActiveId)) {
      fallbackId = previousActiveId;
    } else {
      const mru = [...visibleTabs.value].sort((a, b) => b.lastActiveSeq - a.lastActiveSeq)[0];
      fallbackId = mru?.id ?? null;
    }
    activeTabId.value = fallbackId;
    if (!fallbackId || !hooks) return;
    const cached = snapshots.get(fallbackId);
    if (cached !== undefined) {
      hooks.restoreSession(cached);
      snapshots.delete(fallbackId);
      const tab = tabs.value.find(t => t.id === fallbackId);
      if (tab?.lastActivePanel) hooks.setActivePanel(tab.lastActivePanel);
    }
    // Snapshot LRU-evicted: the existing switch-away-and-back reload path
    // still recovers the tab; nothing to restore here.
  };
  try {
    hooks.onTeardown();
    const outcome = await hooks.onLoadFile(filePath, { forceImage: newTab.forceImage, quiet: opts?.quiet });
    if (outcome.opened) {
      newTab.lastActiveSeq = nextActivationSeq();
      // First-open: leave the panel decision to the workspace shell's
      // `finalizeOpenOutcome` (which already set a sensible default
      // for this mode). We don't touch lastActivePanel here.
      return { ok: true };
    }
    // Backend refused — yank the tab back out and restore the active tab.
    // The load hook has already reported the reason unless `quiet`.
    rollback();
    return { ok: false, error: outcome.error ?? tr("载入未完成") };
  } catch (e) {
    // The workspace shell's load hook catches load errors itself, so this
    // only fires on an unexpected teardown/hook failure. Roll back the
    // optimistic insert and the active pointer.
    rollback();
    console.error('[useTabManager] openNewTab failed:', e);
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  } finally {
    isSwitching.value = false;
  }
}

async function activateTab(tabId: string): Promise<void> {
  if (!hooks) return;
  if (tabId === activeTabId.value) return;
  if (hooks.isBusy()) return;

  const next = tabs.value.find(t => t.id === tabId);
  if (!next) return;
  const targetPath = hooks.resolveTabPath?.(next.filePath) ?? next.filePath;
  if (targetPath !== next.filePath) {
    await openNewTab(targetPath);
    return;
  }

  snapshotActivePanel();
  // Stash the leaving tab's session BEFORE we touch live state.
  // The LRU may immediately drop this entry if the cap is full and
  // `next` happens to be the freshest by activation order — that's
  // fine, the lastActivePanel field on `next` is independent.
  stashCurrentSnapshot();

  isSwitching.value = true;
  try {
    // Cache hit — restore directly from snapshot, no backend
    // roundtrip. This is the whole point of the LRU(5): up to 5 tabs
    // stay "hot" and switching between them is just a ref swap.
    const cached = snapshots.get(next.id);
    if (cached !== undefined) {
      hooks.restoreSession(cached);
      // The snapshot is now LIVE in the refs. Drop the cache entry
      // so we don't double-account memory; we'll re-stash it on the
      // next switch-away if the user navigates elsewhere.
      snapshots.delete(next.id);
      activeTabId.value = next.id;
      next.lastActiveSeq = nextActivationSeq();
      if (next.lastActivePanel) hooks.setActivePanel(next.lastActivePanel);
      return;
    }

    // Cache miss — fall through to the original reload path. Either
    // the tab is being activated for the first time, the tab was
    // created in `pendingLoad` mode during a previous analysis run,
    // or its snapshot was evicted by the LRU cap. Backend `DbCache`
    // + `StreamCache` make this fast for repeats.
    hooks.onTeardown();
    const outcome = await hooks.onLoadFile(next.filePath, { forceImage: next.forceImage });
    if (outcome.opened) {
      activeTabId.value = next.id;
      next.lastActiveSeq = nextActivationSeq();
      // First successful load clears the deferred flag so the chip
      // stops rendering its "未分析" hint.
      if (next.pendingLoad) next.pendingLoad = false;
      if (next.lastActivePanel) {
        hooks.setActivePanel(next.lastActivePanel);
      }
    } else {
      // Load failed (file deleted, etc.) — the load hook has reported the
      // reason; close the tab so we don't present a "broken" entry the
      // user can keep clicking on.
      await closeTab(tabId);
    }
  } catch (e) {
    console.error('[useTabManager] activateTab failed:', e);
    await closeTab(tabId);
  } finally {
    isSwitching.value = false;
  }
}

async function closeTab(tabId: string): Promise<void> {
  const idx = tabs.value.findIndex(t => t.id === tabId);
  if (idx < 0) return;

  const wasActive = activeTabId.value === tabId;
  tabs.value = tabs.value.filter(t => t.id !== tabId);
  // Drop the cached snapshot — the tab no longer exists, no need to
  // keep its session in memory.
  snapshots.delete(tabId);

  if (!wasActive) return;

  if (visibleTabs.value.length === 0) {
    activeTabId.value = null;
    // Tear the file session down so the welcome screen has a clean
    // slate to render.
    hooks?.onTeardown();
    return;
  }

  // Activate the most-recently-used remaining tab. We need the
  // active-tab pointer to land on the picked tab's id by the time
  // this Promise resolves, so callers (including tests) can observe
  // the post-close state without manually flushing microtasks.
  const next = [...visibleTabs.value].sort((a, b) => b.lastActiveSeq - a.lastActiveSeq)[0];
  // Clear the dangling pointer so activateTab's "already active"
  // short-circuit doesn't fire on the closed tab's id.
  activeTabId.value = null;
  await activateTab(next.id);
}

/** Close every tab and tear the live session down — used when switching or
 *  closing a case, whose tabs belong to that case's working set. Clears the
 *  snapshot LRU and the active pointer so the next case (or the welcome
 *  screen) starts from a clean slate; no per-tab activation churn. */
function closeAllTabs(): void {
  tabs.value = [];
  activeTabId.value = null;
  snapshots.clear();
  hooks?.onTeardown();
}

/** Cycle through visible tabs only (Ctrl+Tab). */
async function cycleTab(direction: 1 | -1): Promise<void> {
  const candidates = visibleTabs.value;
  if (candidates.length < 2) return;
  const idx = candidates.findIndex(t => t.id === activeTabId.value);
  if (idx < 0) {
    await activateTab(candidates[0].id);
    return;
  }
  const nextIdx = (idx + direction + candidates.length) % candidates.length;
  await activateTab(candidates[nextIdx].id);
}

/** Activate by 1-based index (Ctrl+1..9). Out-of-range = no-op. */
async function activateTabByIndex(index1Based: number): Promise<void> {
  const idx = index1Based - 1;
  if (idx < 0 || idx >= visibleTabs.value.length) return;
  await activateTab(visibleTabs.value[idx].id);
}

// ── Test-only reset (no-export shim) ────────────────────────────────
// Used by the unit tests to clear singleton state between runs;
// production code should never call this.
export function __resetTabManagerForTesting(): void {
  tabs.value = [];
  activeTabId.value = null;
  isSwitching.value = false;
  hooks = null;
  snapshots.clear();
  activationCounter = 0;
}

// ── Hook-style accessor ─────────────────────────────────────────────
export function useTabManager() {
  return {
    tabs,
    visibleTabs,
    activeTabId,
    activeTab,
    isSwitching,
    openNewTab,
    activateTab,
    closeTab,
    closeAllTabs,
    cycleTab,
    activateTabByIndex,
  };
}
