
import { tr } from '../../../composables/useLocale';
// Active-case + volume-snapshot session state.
//
// A case is a global singleton (the backend holds exactly one active case),
// so this composable keeps module-level reactive state shared across every
// caller — the titlebar chip and the case-manager modal observe the same
// refs. All backend access goes through the `lib/ipc` service layer.
import { ref, shallowRef } from 'vue';

import { onEvent, type UnlistenFn } from '../../../lib/ipc/events';
import { taskCancel } from '../../../lib/ipc/taskRuntime';
import {
  autoSnapshotTaskId,
  caseActive,
  caseClose,
  caseCreate,
  caseEvidenceRemove,
  caseList,
  caseLock,
  caseOpen,
  caseRemove,
  caseRename,
  caseUnlock,
  snapshotBuild,
  snapshotDrop,
  snapshotList,
  SNAPSHOT_PROGRESS_EVENT,
  type CaseInfo,
  type CaseListItem,
  type SnapshotInfo,
  type SnapshotProgress,
} from '../../../lib/ipc/cases';
import { open as openDialog } from '@tauri-apps/plugin-dialog';

import { useRecentCases } from './useRecentCases';

const { recordCase, removeCase: forgetRecentCase } = useRecentCases();

const activeCase = shallowRef<CaseInfo | null>(null);
/** Locally registered cases (backend `case_list`), most recently opened first. */
const cases = ref<CaseListItem[]>([]);
const registryLoading = ref(false);
const registryError = ref('');
const snapshots = ref<SnapshotInfo[]>([]);
/** Live build progress keyed by `${image_id}:${volume_key}`. */
const buildProgress = ref<Map<string, SnapshotProgress>>(new Map());
const busy = ref(false);

// Workspace teardown injected by the shell (AppShell): closes every open
// image tab + wipes the live file session. A case's evidence is its working
// set, so switching to (or closing) a case must retire the previous case's
// tabs — otherwise a stale tab re-`image_load`s its image into the now-active
// case (cross-contamination). Decoupled via registration like
// `registerRaidImageOpener`, so the case composable stays UI-agnostic.
let workspaceTeardown: (() => void | Promise<void>) | null = null;
let caseOpened: ((info: CaseInfo) => void) | null = null;
let caseRevision = 0;
let activeRequest = 0;
let snapshotRequest = 0;
let registryRequest = 0;
let listenerGeneration = 0;
let listening: Promise<void> | null = null;
let unlisten: UnlistenFn | null = null;

/** Register the handler that closes all image tabs + clears the file session.
 *  Called once by AppShell. */
export function registerCaseWorkspaceTeardown(teardown: () => void | Promise<void>): () => void {
  workspaceTeardown = teardown;
  return () => { if (workspaceTeardown === teardown) workspaceTeardown = null; };
}

export function registerCaseOpened(handler: (info: CaseInfo) => void): () => void {
  caseOpened = handler;
  return () => { if (caseOpened === handler) caseOpened = null; };
}

/** The main workspace owns this singleton's event bridge; backend tasks keep running. */
export function disposeCaseSession(): void {
  listenerGeneration++;
  caseRevision++;
  activeRequest++;
  snapshotRequest++;
  registryRequest++;
  unlisten?.();
  unlisten = null;
  listening = null;
  workspaceTeardown = null;
  caseOpened = null;
  activeCase.value = null;
  cases.value = [];
  registryLoading.value = false;
  registryError.value = '';
  snapshots.value = [];
  buildProgress.value = new Map();
  busy.value = false;
}

function beginCaseMutation(): number {
  if (busy.value) throw new Error(tr("案件操作正在进行，请稍候"));
  busy.value = true;
  activeRequest++;
  snapshotRequest++;
  return ++caseRevision;
}

function progressKey(imageId: string, volumeKey: string): string {
  return `${imageId}:${volumeKey}`;
}

async function refreshActive(): Promise<void> {
  if (busy.value) return;
  const request = ++activeRequest;
  const revision = caseRevision;
  const info = await caseActive();
  if (busy.value || request !== activeRequest || revision !== caseRevision) return;
  activeCase.value = info;
  await refreshSnapshots();
}

/** Reload the backend case registry. Best-effort: a registry read failure
 *  must never break the case manager (the list just stays as it was). */
async function refreshCases(): Promise<void> {
  const request = ++registryRequest;
  registryLoading.value = true;
  registryError.value = '';
  try {
    const next = await caseList();
    if (request === registryRequest) cases.value = next;
  } catch (err) {
    if (request === registryRequest) registryError.value = tr("读取案件列表失败：{0}", { 0: String(err) });
  } finally {
    if (request === registryRequest) registryLoading.value = false;
  }
}

async function refreshSnapshots(): Promise<void> {
  // With a case open, the backend `snapshot_list` is authoritative — replace.
  // With NO case open, `snapshots` holds only the synthesized caseless session
  // entries (see upsertSessionSnapshot); leave them intact — resetting to the
  // case-only list ([]) here would wipe the "快照就绪" chip on any caseless
  // refreshActive() (e.g. opening the Case Manager modal). closeCase() clears
  // the ref explicitly when a case is closed, so no stale case entries linger.
  if (activeCase.value) {
    const request = ++snapshotRequest;
    const revision = caseRevision;
    const directory = activeCase.value.dir;
    const next = await snapshotList();
    if (request === snapshotRequest && revision === caseRevision && directory === activeCase.value?.dir) snapshots.value = next;
  }
}

// Caseless session snapshots: the backend `snapshot_list` is case-only, so when
// no case is open we synthesize a `SnapshotInfo` from the terminal build event
// (which carries entry_count + truncated) to drive the "快照就绪" chip. Dropped
// on cancel/error; the case path's authoritative list supersedes these the
// moment a case opens.
function upsertSessionSnapshot(payload: SnapshotProgress): void {
  const others = snapshots.value.filter(
    s => !(s.image_id === payload.image_id && s.volume_key === payload.volume_key),
  );
  if (payload.phase === 'done') {
    others.push({
      image_id: payload.image_id,
      volume_key: payload.volume_key,
      entry_count: payload.entries,
      truncated: payload.truncated,
      built_at: '',
      source_volume_size: 0,
      // A caseless build reports `done` only once its index is in place.
      indexed: true,
      stale: false,
    });
  }
  snapshots.value = others;
}

function ensureListening(): Promise<void> {
  if (listening) return listening;
  const generation = listenerGeneration;
  listening = onEvent<SnapshotProgress>(SNAPSHOT_PROGRESS_EVENT, ({ payload }) => {
    if (generation !== listenerGeneration) return;
    const key = progressKey(payload.image_id, payload.volume_key);
    const next = new Map(buildProgress.value);
    if (['done', 'reused', 'cancelled', 'error'].includes(payload.phase)) {
      next.delete(key);
      if (activeCase.value) {
        void refreshSnapshots().catch(error => console.warn('snapshot list refresh failed', error));
      } else {
        upsertSessionSnapshot(payload);
      }
    } else {
      next.set(key, payload);
    }
    buildProgress.value = next;
  }).then(stop => {
    if (generation !== listenerGeneration) stop();
    else unlisten = stop;
  }).catch(error => {
    if (generation !== listenerGeneration) return;
    listening = null;
    console.warn('subscribe snapshot progress failed', error);
  });
  return listening;
}

/** Pick a folder for a case directory. Returns null if the user cancels. */
async function pickDirectory(title: string): Promise<string | null> {
  const picked = await openDialog({ directory: true, multiple: false, title });
  if (typeof picked === 'string') return picked;
  return null;
}

export function useCaseSession() {
  void ensureListening();

  async function createCase(
    dir: string,
    name: string,
    extra: { caseNumber?: string; description?: string; examiner?: string } = {},
  ): Promise<void> {
    const revision = beginCaseMutation();
    try {
      // Switching away from an open case: retire its tabs first (the backend
      // exits the previous case and unloads the matching runtime images), so
      // the new case starts empty instead of inheriting the previous case's
      // open images.
      await workspaceTeardown?.();
      if (revision !== caseRevision) return;
      const next = await caseCreate({ dir, name, ...extra });
      if (revision !== caseRevision) return;
      activeCase.value = next;
      recordCase(dir, activeCase.value.name);
      // Best-effort: a snapshot-list refresh failure must not abort a case
      // that already opened successfully (it would surface as a false "open
      // failed"). Snapshots can be refreshed again later.
      await refreshSnapshots().catch(() => { });
      await refreshCases();
    } finally {
      if (revision === caseRevision) busy.value = false;
    }
  }

  async function openCase(dir: string): Promise<void> {
    const revision = beginCaseMutation();
    try {
      // Switching cases: close the prior case's tabs before opening the new
      // one (see createCase). The modal re-opens the new case's evidence as
      // tabs after this resolves, so the teardown can't race them.
      await workspaceTeardown?.();
      if (revision !== caseRevision) return;
      const next = await caseOpen({ dir });
      if (revision !== caseRevision) return;
      activeCase.value = next;
      recordCase(dir, activeCase.value.name);
      await refreshSnapshots().catch(() => { });
      await refreshCases();
      // 即发即忘：开关开启 + 文件夹模式时，把案件内全部证据分区自动挂上。
      // 不 await——案件已打开成功，挂载在后台进行并自带提示，不拖慢打开。
      caseOpened?.(activeCase.value);
    } finally {
      if (revision === caseRevision) busy.value = false;
    }
  }

  async function closeCase(): Promise<void> {
    const revision = beginCaseMutation();
    try {
      await workspaceTeardown?.();
      if (revision !== caseRevision) return;
      await caseClose();
      if (revision !== caseRevision) return;
      activeCase.value = null;
      snapshots.value = [];
      buildProgress.value = new Map();
      await refreshCases();
    } finally {
      if (revision === caseRevision) busy.value = false;
    }
  }

  /** Toggle the active case's logical lock. */
  async function setLocked(locked: boolean): Promise<void> {
    const revision = beginCaseMutation();
    try {
      activeCase.value = locked ? await caseLock() : await caseUnlock();
      await refreshCases();
    } finally {
      if (revision === caseRevision) busy.value = false;
    }
  }

  async function renameCase(name: string): Promise<void> {
    const revision = beginCaseMutation();
    try {
      const next = await caseRename({ name });
      if (revision !== caseRevision) return;
      activeCase.value = next;
      if (activeCase.value) recordCase(activeCase.value.dir, activeCase.value.name);
      await refreshCases();
    } finally {
      if (revision === caseRevision) busy.value = false;
    }
  }

  /** Delete a case's own files (closes it first when it is active). */
  async function removeCase(dir: string): Promise<void> {
    const revision = beginCaseMutation();
    try {
      const wasActive = activeCase.value?.dir === dir;
      if (wasActive) {
        await workspaceTeardown?.();
      }
      if (revision !== caseRevision) return;
      await caseRemove({ dir });
      if (revision !== caseRevision) return;
      forgetRecentCase(dir);
      if (wasActive) {
        activeCase.value = null;
        snapshots.value = [];
        buildProgress.value = new Map();
      }
      await refreshCases();
    } finally {
      if (revision === caseRevision) busy.value = false;
    }
  }

  async function buildSnapshot(imageId: string, volumeKey: string): Promise<void> {
    const taskId = autoSnapshotTaskId(imageId, volumeKey);
    await snapshotBuild({ imageId, volumeKey, taskId });
  }

  async function cancelBuild(imageId: string, volumeKey: string): Promise<void> {
    await taskCancel({ taskId: autoSnapshotTaskId(imageId, volumeKey) });
  }

  async function dropSnapshot(imageId: string, volumeKey: string): Promise<void> {
    await snapshotDrop({ imageId, volumeKey });
    await refreshSnapshots();
  }

  /** Remove an evidence image from the active case (cascades to its snapshots
   *  on the backend). Updates the active-case view from the returned info. */
  async function removeEvidence(imageId: string): Promise<void> {
    const revision = beginCaseMutation();
    try {
      const next = await caseEvidenceRemove({ imageId });
      if (revision !== caseRevision) return;
      activeCase.value = next;
      await refreshSnapshots();
    } finally {
      if (revision === caseRevision) busy.value = false;
    }
  }

  return {
    activeCase,
    cases,
    registryLoading,
    registryError,
    snapshots,
    buildProgress,
    busy,
    refreshActive,
    refreshCases,
    refreshSnapshots,
    pickDirectory,
    createCase,
    openCase,
    closeCase,
    setLocked,
    renameCase,
    removeCase,
    buildSnapshot,
    cancelBuild,
    dropSnapshot,
    removeEvidence,
  };
}
