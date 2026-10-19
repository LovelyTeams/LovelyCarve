import { computed, onScopeDispose, watch, type Ref } from 'vue';
import { initTabManagerHooks, useTabManager } from '../workspace/useTabManager';
import { raidMemberTabAliases, raidTabPathKey } from '../workspace/raidTabAliases';
import type { LoadFileOptions } from '../workspace/useTabManager';
import { registerCaseWorkspaceTeardown, registerCaseOpened, disposeCaseSession, useCaseSession } from '../../features/cases/api';
import { registerBitlockerUnlockNavigator, type OpenFileOutcome } from '../../features/evidence/api';
import type { useFileSession } from '../../features/evidence/api';
import { registerRaidImageOpener, useRaidOverview, disposeRaidStack } from '../../features/raid/api';

import { flushFileTagWrites } from '../../features/files/api';
import { flushFileDerivedWrites } from '../../features/files/api';
import { useAutoMount } from '../../features/evidence/api';
import { registerAutoDetectReportPublished, disposeAutoDetect, ensureAutoDetectListener } from '../../features/evidence/api';
import { disposeTaskCenter, useTaskCenter } from '../../features/tasks/api';
import { disposeAnalysisResults } from '../../features/analysis/api';

import { registerTaskContext } from '../../features/tasks/api';
import { registerAnalysisContext } from '../../features/analysis/api';

import { registerBitlockerSnapshotWorkflow, disposeBitlockerJobs, disposeImageHashJobs, registerEvidenceContext, disposeImageExportJobs } from '../../features/evidence/api';
import { registerEncryptedVolumeImageUpdater, registerEncryptedVolumeImageOpener, disposeEncryptedVolumeUnlockJobs } from '../../features/evidence/api';

type FileSession = ReturnType<typeof useFileSession>;

/** Bind the one main-window file session to existing singleton workflow hooks. */
export function bindWorkspaceServices(options: {
  resetFileSession: FileSession['resetSession'];
  captureFileSession: FileSession['captureSnapshot'];
  restoreFileSession: FileSession['restoreSnapshot'];
  updateImageInfo: FileSession['updateImageInfo'];
  loadFileForTab: (path: string, options?: LoadFileOptions) => Promise<OpenFileOutcome>;
  loading: Ref<boolean>;
  activePanel: Ref<string>;
  showToolSettings: Ref<boolean>;
  closeAllTabs: () => void;
  handleSidebarSelect: (panel: string) => void;
  onOpenImageFromInfo: (path: string) => Promise<boolean>;
}) {
  const { resetFileSession, captureFileSession, restoreFileSession, loadFileForTab,
    loading, activePanel, showToolSettings, closeAllTabs, handleSidebarSelect, onOpenImageFromInfo } = options;
  const caseSession = useCaseSession();
  const tabManager = useTabManager();
  const { stack: raidStack } = useRaidOverview();
  const tabAliases = computed(() => raidMemberTabAliases(raidStack.value, caseSession.activeCase.value?.evidence ?? []));
  const resolveTabPath = (path: string) => tabAliases.value.get(raidTabPathKey(path)) ?? path;
  const stopTaskContext = registerTaskContext({
    activeCase: () => caseSession.activeCase.value,
    ensureDetection: ensureAutoDetectListener,
  });
  const taskCenter = useTaskCenter();
  const stopAnalysisContext = registerAnalysisContext({
    caseDirectory: () => caseSession.activeCase.value?.dir ?? null,
    completedRuns: () => taskCenter.tasks.value.flatMap(task => task.status === 'success' && task.result?.runId ? [task.result.runId] : []),
  });
  let caseEpoch = 0;
  watch(() => caseSession.activeCase.value?.dir ?? null, () => { caseEpoch++; }, { flush: 'sync' });
  const stopEvidenceContext = registerEvidenceContext(() => caseEpoch);
  const stopSnapshot = registerBitlockerSnapshotWorkflow({
    context: () => caseEpoch,
    unlocked: (imageId, partitionId) => {
      if (!caseSession.activeCase.value?.evidence.some(evidence => evidence.image_id === imageId)) return;
      const volumeKey = `part:${partitionId}`;
      if (caseSession.snapshots.value.some(snapshot => snapshot.image_id === imageId && snapshot.volume_key === volumeKey && !snapshot.stale)
        || caseSession.buildProgress.value.has(`${imageId}:${volumeKey}`)) return;
      void caseSession.buildSnapshot(imageId, volumeKey).catch(error => console.warn('解锁后构建快照失败', error));
    },
  });
  const stopRaid = registerRaidImageOpener(path => void onOpenImageFromInfo(path));
  const stopEncryptedUnlock = registerEncryptedVolumeImageUpdater(options.updateImageInfo);
  const stopEncryptedOpener = registerEncryptedVolumeImageOpener(onOpenImageFromInfo);
  const stopUnlock = registerBitlockerUnlockNavigator(() => handleSidebarSelect('dashboard'));
  /** Bundle every per-composable snapshot into one opaque object. The
   *  tab manager treats this as a black box; only this file knows how
   *  the pieces fit together. Up to 5 tabs' bundles stay live in the
   *  manager's LRU cache so the user can switch between them without
   *  paying re-load cost — the moment we go to a 6th, the oldest tab
   *  drops out of the cache and falls back to the SQLite-backed
   *  re-load path on its next activation. */
  interface AppSessionSnapshot {
    fileSession: ReturnType<typeof captureFileSession>;
  }

  const stopTabs = initTabManagerHooks({
    resolveTabPath,
    onLoadFile: loadFileForTab,
    onTeardown: () => {
      resetFileSession();             // wipes image/current-file refs
      showToolSettings.value = false; // close any open modal pointing at the previous file
      // Drop the panel pointer so the welcome `v-else` wins when no tab
      // replacement follows (last-tab close). On switch/open paths,
      // `finalizeOpenOutcome` re-sets this to the correct panel for the
      // newly loaded file before the user sees the transition.
      activePanel.value = '';
    },
    getActivePanel: () => activePanel.value,
    setActivePanel: (panelId) => { activePanel.value = panelId; },
    isBusy: () => loading.value,
    captureSession: (): AppSessionSnapshot => ({
      fileSession: captureFileSession(),
    }),
    restoreSession: (snap) => {
      const s = snap as AppSessionSnapshot;
      restoreFileSession(s.fileSession);
    },
  });

  // When detection groups the active member, focus its array once loading is idle.
  watch([tabAliases, tabManager.activeTabId, tabManager.isSwitching, loading], () => {
    const tab = tabManager.activeTab.value;
    if (!tab || tabManager.isSwitching.value || loading.value) return;
    const targetPath = resolveTabPath(tab.filePath);
    if (targetPath !== tab.filePath) void tabManager.openNewTab(targetPath);
  });

  // A case's open tabs are part of its working set — switching to or closing a
  // case must retire them so a stale tab can't re-register its image into the
  // new case. `closeAllTabs` clears the tab list + snapshot LRU and runs the
  // tab-manager teardown (wiping the live file session) in one shot.
  const stopCase = registerCaseWorkspaceTeardown(async () => {
    closeAllTabs();
    disposeRaidStack();
    await Promise.all([flushFileTagWrites(), flushFileDerivedWrites()]);
  });
  const stopOpened = registerCaseOpened(info => void useAutoMount().maybeAutoMountCase(info));
  const stopReport = registerAutoDetectReportPublished(report => {
    const session = useCaseSession();
    if (session.activeCase.value?.evidence.some(evidence => evidence.image_id === report.image_id)) {
      void session.refreshActive().catch(error => console.warn('refresh case evidence failed', error));
    }
  });
  onScopeDispose(() => {
    stopEncryptedUnlock(); stopEncryptedOpener(); disposeEncryptedVolumeUnlockJobs();
    stopEvidenceContext(); stopSnapshot(); disposeBitlockerJobs(); disposeImageHashJobs(); disposeImageExportJobs(); stopTaskContext(); stopAnalysisContext(); stopCase(); stopOpened(); stopReport(); stopRaid(); stopUnlock(); stopTabs();
    disposeAnalysisResults(); disposeTaskCenter(); disposeAutoDetect(); disposeCaseSession();
    disposeRaidStack();
    void flushFileTagWrites();
  });

}
