
import { tr } from '../../composables/useLocale';
import { computed, nextTick, onMounted, onUnmounted, ref, watch, type Ref } from 'vue';
import { systemPathInfo } from '../../lib/ipc/system';
import { buildImageNavGroups } from './navigation';
import { type NavGroup } from './types';
import type { OpenFileOutcome } from '../../features/evidence/api';
import { useTabManager, type LoadFileOptions, type OpenTabResult } from './useTabManager';
import { useCaseSession } from '../../features/cases/api';
import { notifyError, notifyInfo } from '../../composables/useToast';
import type { RecentCase } from '../../features/cases/api';
import { useBackgroundEvidenceLoads } from './useBackgroundEvidenceLoads';

type OpenByPath = (filePath: string, options?: { onBeforeLoad?: () => void; forceImage?: boolean; caseDir?: string }) => Promise<OpenFileOutcome>;

type WorkspaceShellOptions = {
  /** True once an evidence image/folder is loaded; gates whether the sidebar
   *  renders for the active file session. */
  isEvidenceMode: Ref<boolean>;
  /** Folder evidence has no raw byte stream, so byte-level analysis entries
   *  such as signature carving must stay hidden for that session. */
  supportsRawAnalysis?: Ref<boolean>;
  openByPath: OpenByPath;
  /** Open the case-manager modal. Called when the user tries to open a file
   *  with no active case — they must create / open a case there first. */
  onNeedCase: () => void;
  /** Show the "添加证据" chooser (镜像文件 / 文件夹 / RAID 磁盘). The shell only
   *  gates on the active case and delegates the actual pick + load to AppShell.vue,
   *  which owns the dialog state and the RAID-reconstruction orchestration. */
  onRequestAddImage: () => void;
  /** Close the case gate before replaying a pending open action. */
  onCaseGateSatisfied?: () => void;
  /** Restore the selected case's evidence, including reconstructed RAID images. */
  onOpenCaseEvidence?: (paths: string[]) => Promise<void>;
  /** Remember which BCP file a drag-and-drop open should highlight. */
  onBcpFileOpen?: (folder: string, fileName: string) => void;
  /** Let the active workflow consume native drops before normal tab opening. */
  onDroppedPaths?: (paths: string[]) => Promise<boolean>;
};

type PendingOpenRequest =
  | { kind: 'choose-source' }
  | { kind: 'open-paths'; paths: string[] };

export function useWorkspaceShell(options: WorkspaceShellOptions) {
  const activePanel = ref('');
  const isDragOver = ref(false);

  // Multi-tab orchestrator. We import the singleton here so the rest
  // of the workspace can route file-open requests through it without
  // each call site needing to know about the tab abstraction.
  const tabManager = useTabManager();
  const { activeCase, openCase, refreshActive, busy: caseBusy } = useCaseSession();
  const backgroundLoads = useBackgroundEvidenceLoads(tabManager, () => activeCase.value?.dir ?? null);
  const isOpeningRecentCase = ref(false);

  let dragLeaveTimer: ReturnType<typeof setTimeout> | null = null;
  let unlistenDragDrop: (() => void) | null = null;
  let pendingOpenRequest: PendingOpenRequest | null = null;

  const sidebarGroups = computed<NavGroup[]>(() => {
    // Only a loaded evidence session gets a sidebar.
    if (!options.isEvidenceMode.value) return [];
    return buildImageNavGroups({
      supportsRawAnalysis: options.supportsRawAnalysis?.value ?? true,
    });
  });

  const finalizeOpenOutcome = async (outcome: OpenFileOutcome) => {
    if (!outcome.opened) return;
    activePanel.value = outcome.nextPanel;
    // image_load registers a newly opened image as evidence of the active case
    // on the backend, but `activeCase` is only replaced on case create / open.
    // Re-read it when the image is missing, so views scoped to the current
    // evidence (task hub, quick analysis, result navigation) can find it.
    // Best-effort: a failed refresh must not fail an image that did open.
    if (activeCase.value && !activeCase.value.evidence.some(evidence => evidence.image_id === outcome.imageId)) {
      await refreshActive().catch(error => console.warn('case refresh after image load failed', error));
    }
  };

  /** Run the underlying single-file load + finalize. This is the
   *  function `useTabManager`'s `onLoadFile` hook calls — it does
   *  NOT touch `tabs[]`, NOT call any reset (the tab manager already
   *  invoked `onTeardown` before we get here). Returning the outcome
   *  lets the tab manager decide whether to keep the new tab.
   *
   *  Every tab load (picker, drag & drop, re-activating an evicted tab)
   *  funnels through here, so failures are reported once, with the
   *  backend's reason, unless the caller passes `quiet` to aggregate or
   *  annotate them itself. */
  const loadFileForTab = async (filePath: string, opts?: LoadFileOptions): Promise<OpenFileOutcome> => {
    let outcome: OpenFileOutcome;
    try {
      outcome = await options.openByPath(filePath, { forceImage: opts?.forceImage, ...(activeCase.value ? { caseDir: activeCase.value.dir } : {}) });
      await finalizeOpenOutcome(outcome);
    } catch (error) {
      console.error('Failed to open file:', error);
      outcome = { opened: false, error: error instanceof Error ? error.message : String(error) };
    }
    if (!outcome.opened && !opts?.quiet) {
      const name = filePath.split(/[\\/]/).pop() || filePath;
      notifyError(tr("打开证据失败"), `${name}：${outcome.error ?? tr("载入未完成")}`);
    }
    return outcome;
  };

  /** Public-facing "add image" — what the welcome screen, sidebar open
   *  button, "+" button on Tabbar, and Ctrl+O all call. Gates on an active
   *  case, then opens the chooser dialog (镜像文件 / 文件夹 / RAID 磁盘); AppShell.vue
   *  picks the source and runs the matching load flow. */
  const handleOpenFile = async () => {
    if (!ensureCaseBeforeOpen({ kind: 'choose-source' })) return;
    options.onRequestAddImage();
  };

  /** Programmatically open an image/file by absolute path (no picker), as a
   *  tab. Used when re-opening a case to restore its evidence images.
   *  `openNewTab` dedups by path and rolls back a failed load; a failure is
   *  already reported unless `quiet`, in which case the caller surfaces
   *  the returned `error`. */
  const openImagePath = (path: string, opts?: LoadFileOptions): Promise<OpenTabResult> =>
    backgroundLoads.open(path, opts);

  const handleSidebarSelect = (id: string) => {
    activePanel.value = id;
  };

  async function handleOpenRecentCase(entry: RecentCase): Promise<void> {
    if (isOpeningRecentCase.value || caseBusy.value) return;
    isOpeningRecentCase.value = true;
    clearPendingOpenRequest();
    try {
      await openCase(entry.dir);
      const paths = [...new Set((activeCase.value?.evidence ?? []).map(evidence => evidence.path))];
      if (paths.length > 0) {
        await options.onOpenCaseEvidence?.(paths);
      } else {
        options.onNeedCase();
      }
    } catch (error) {
      notifyError(tr("打开案件失败"), `${entry.name}：${error}`);
    } finally {
      isOpeningRecentCase.value = false;
    }
  }

  async function openDroppedPaths(paths: string[]): Promise<void> {
    for (const path of paths) {
      if (/\.bcp$/i.test(path)) {
        const folder = path.replace(/[\\/][^\\/]+$/, '');
        options.onBcpFileOpen?.(folder, path.split(/[\\/]/).at(-1) ?? '');
        const result = await tabManager.openNewTab(folder, { forceImage: true });
        if (result.ok) activePanel.value = 'bcp';
        continue;
      }
      const pathInfo = await systemPathInfo(path).catch(() => null);
      await openImagePath(path, { forceImage: pathInfo?.is_directory === true });
    }
  }

  async function resumePendingOpenRequest(): Promise<void> {
    const request = pendingOpenRequest;
    if (!request || !activeCase.value) return;
    pendingOpenRequest = null;
    options.onCaseGateSatisfied?.();
    await nextTick();
    if (request.kind === 'choose-source') {
      options.onRequestAddImage();
      return;
    }
    await openDroppedPaths(request.paths);
  }

  function clearPendingOpenRequest(): void {
    pendingOpenRequest = null;
  }

  /** An opened file must live inside a case. If none is active, open the
   *  case-manager modal (new / open existing / recent) and defer this open.
   *  Once the case becomes active, the saved action resumes automatically.
   *  Returns false when there's no case (caller must not open the file). */
  const ensureCaseBeforeOpen = (request: PendingOpenRequest): boolean => {
    if (activeCase.value) return true;
    pendingOpenRequest = request;
    notifyInfo(tr("请先创建或打开案件"), tr("证据镜像会登记到当前案件，并用于卷快照、标签和哈希结果持久化。"));
    options.onNeedCase();
    return false;
  };

  watch(
    () => activeCase.value?.id ?? null,
    caseId => {
      if (!caseId) return;
      void resumePendingOpenRequest();
    },
  );

  const handleGlobalKeydown = (event: KeyboardEvent) => {
    // Don't hijack shortcuts when the user is typing in a form field.
    const target = event.target as HTMLElement | null;
    const inEditable = !!target && (
      target.tagName === 'INPUT' ||
      target.tagName === 'TEXTAREA' ||
      target.isContentEditable
    );

    const ctrl = event.ctrlKey || event.metaKey;

    if (ctrl && event.key === 'o') {
      event.preventDefault();
      void handleOpenFile();
      return;
    }

    if (ctrl && (event.key === 't' || event.key === 'T')) {
      if (inEditable) return;
      event.preventDefault();
      void handleOpenFile();
      return;
    }

    if (ctrl && (event.key === 'w' || event.key === 'W')) {
      if (inEditable) return;
      const activeId = tabManager.activeTabId.value;
      if (activeId) {
        event.preventDefault();
        tabManager.closeTab(activeId);
      }
      return;
    }

    if (ctrl && event.key === 'Tab') {
      if (inEditable) return;
      if (tabManager.visibleTabs.value.length < 2) return;
      event.preventDefault();
      void tabManager.cycleTab(event.shiftKey ? -1 : 1);
      return;
    }

    // Ctrl+1..9 → jump straight to tab N (1-based; matches Chrome).
    if (ctrl && /^[1-9]$/.test(event.key)) {
      if (inEditable) return;
      const index = parseInt(event.key, 10);
      if (index <= tabManager.visibleTabs.value.length) {
        event.preventDefault();
        void tabManager.activateTabByIndex(index);
      }
    }
  };

  const setupDragDrop = async () => {
    try {
      const { getCurrentWebviewWindow } = await import('@tauri-apps/api/webviewWindow');
      const appWindow = getCurrentWebviewWindow();
      unlistenDragDrop = await appWindow.onDragDropEvent(async event => {
        if (event.payload.type === 'enter' || event.payload.type === 'over') {
          isDragOver.value = true;
          if (dragLeaveTimer) {
            clearTimeout(dragLeaveTimer);
            dragLeaveTimer = null;
          }
          return;
        }

        if (event.payload.type === 'drop') {
          if (dragLeaveTimer) {
            clearTimeout(dragLeaveTimer);
            dragLeaveTimer = null;
          }
          isDragOver.value = false;
          const paths = event.payload.paths ?? [];
          if (paths.length > 0) {
            if (await options.onDroppedPaths?.(paths)) return;
            // A dropped image must belong to a case. With none active this
            // opens the case modal and replays the same drop after the case
            // becomes active, so the user does not have to drag it again.
            if (!ensureCaseBeforeOpen({ kind: 'open-paths', paths })) return;
            await openDroppedPaths(paths);
          }
          return;
        }

        if (event.payload.type === 'leave') {
          dragLeaveTimer = setTimeout(() => {
            isDragOver.value = false;
          }, 100);
        }
      });
    } catch (error) {
      console.warn('Drag-drop setup failed:', error);
    }
  };

  onMounted(() => {
    document.addEventListener('keydown', handleGlobalKeydown);
    void setupDragDrop();
  });

  onUnmounted(() => {
    document.removeEventListener('keydown', handleGlobalKeydown);
    if (dragLeaveTimer) {
      clearTimeout(dragLeaveTimer);
    }
    if (unlistenDragDrop) {
      unlistenDragDrop();
      unlistenDragDrop = null;
    }
  });

  return {
    activePanel,
    isDragOver,
    sidebarGroups,
    handleOpenFile,
    handleOpenRecentCase,
    isOpeningRecentCase,
    openImagePath,
    handleSidebarSelect,
    clearPendingOpenRequest,
    /** TabManager hook implementations (AppShell.vue wires these via
     *  `initTabManagerHooks`). Exposed instead of called inline so
     *  the workspace shell stays decoupled from tab-manager imports
     *  on its own setup path. */
    loadFileForTab,
  };
}
