<template>
  <Titlebar
    :tabs="visibleTabs"
    :active-tab-id="activeTabId"
    :is-switching="isSwitching"
    :is-busy="loading"
    :case-name="activeCase?.name ?? null"
    @open-settings="openSettings"
    @open-case="showCaseManager = true"
    @activate-tab="(id: string) => activateTab(id)"
    @close-tab="handleTabClose"
    @new-tab="handleOpenFile"
  >
    <template #workspace-actions>
      <button ref="taskHubTrigger" type="button" class="ui-btn sm titlebar-chip task-hub-trigger" :class="{ active: taskHubOpen }" :aria-expanded="taskHubOpen" aria-controls="task-hub" :aria-label="tr('任务中心')" :title="tr('任务列表与快速分析')" @click="toggleTaskHub">
        <ListTodo :size="14" /><span class="titlebar-chip-label">{{ tr("任务中心") }}</span><span v-if="taskActiveCount" :key="taskActiveCount" class="ui-count accent ui-bump">{{ taskActiveCount.toLocaleString() }}</span>
      </button>
    </template>
  </Titlebar>
  <TaskHub
    v-if="taskHubMounted"
    ref="taskHubRef"
    v-model:tab="taskHubTab"
    :open="taskHubOpen"
    :evidence="taskHubEvidence"
    :session-image="imageInfo"
    :evidence-name="taskHubEvidenceName"
    :has-case="Boolean(activeCase)"
    :active-count="taskActiveCount"
    @close="closeTaskHub"
    @open-case-manager="showCaseManager = true; closeTaskHub()"
    @open-panel="openTaskResult"
  />
  <ToolSettingsModal
    :visible="showToolSettings"
    :initial-section="settingsInitialSection"
    @close="showToolSettings = false"
  />
  <CaseManagerModal
    :visible="showCaseManager"
    :image-id="imageId"
    :image-info="imageInfo"
    @close="handleCaseManagerClose"
    @open-evidence="onOpenCaseEvidence"
    @evidence-removed="onCaseEvidenceRemoved"
    @add-evidence="showCaseManager = false; handleOpenFile()"
  />
  <AddImageDialog
    :visible="showAddImageDialog"
    @close="showAddImageDialog = false"
    @select="onAddImageSelect"
  />
  <PhysicalDiskPicker
    :visible="showPhysicalDiskPicker"
    @close="onPhysicalDiskPickerClose"
    @select="onPhysicalDiskSelect"
  />
  <RaidMemberDetectDialog
    :visible="showRaidMemberDialog"
    :members="raidMemberQueue"
    :stack="raidMemberStack"
    :detecting="raidMemberDetecting"
    :opening="raidMemberOpening"
    :error="raidMemberError"
    :dragging="isDragOver"
    @close="showRaidMemberDialog = false"
    @add-members="addRaidMemberFiles"
    @redetect="detectRaidMemberQueue"
    @remove-member="removeRaidMember"
    @confirm-open="confirmRaidMemberOpen"
  />
  <!-- Drag & Drop Overlay -->
  <Transition name="fade">
    <div v-if="isDragOver && !showRaidMemberDialog" class="drag-overlay">
      <div class="drag-overlay-content">
        <Upload :size="48" />
        <span>{{ tr("松开以添加证据") }}</span>
      </div>
    </div>
  </Transition>
  <div class="main-layout">
    <!-- Category rail and second-level navigation; pages share the same workspace. -->
    <SidebarNav
      v-if="showSidebarNav"
      :groups="displaySidebarGroups"
      :active-id="activePanel"
      @select="handleSidebarSelect"
    >
      <template v-if="imageId" #context="{ select }"><AnalysisResultsNav :image-id="imageId" @select="select('analysis')" /></template>
    </SidebarNav>
    <div class="editor-area">
      <!-- Main Content Area -->
      <div v-content-motion.rise="activePanel + ':' + imageId" class="content-area">
        <EvidenceLoadPanel v-if="activeTab?.loadTaskId" :task-id="activeTab.loadTaskId" :path="activeTab.filePath" @tasks="toggleTaskHub" />
        <template v-else>
        <!-- 文件列表与搜索结果面板使用 KeepAlive：切换界面时保留目录状态，以及在途搜索的
             事件订阅、进度、增量结果和滚动位置。其余面板与欢迎页照常按需挂载。 -->
        <KeepAlive>
          <DiskFileList
            v-if="activePanel === 'files' && imageId"
            ref="diskFileListRef"
            :image-id="imageId"
            :index-only="imageInfo?.index_only === true"
            :volume-revision="imageInfo?.unlocked_partitions?.join(',') ?? ''"
            :activate-preview="activateFilePreview"
          />
          <SearchResultsPanel
            v-else-if="activePanel === 'search-results' && imageId"
            :image-id="imageId"
            @close="handleSidebarSelect('files')"
            :locating="locatingResult"
            @open-file="openResultLocation"
          />
          <CarvePanel
            v-else-if="activePanel === 'carve' && imageId"
            :image-id="imageId"
            :image-info="imageInfo"
          />
          <TimelinePanel
            v-else-if="activePanel === 'timeline' && imageId"
            :image-id="imageId"
            :locating="locatingResult"
            @open-file="openResultLocation"
          />
          <AnalysisResultsPanel
            v-else-if="activePanel === 'analysis' && imageId"
            :image-id="imageId"
            :locating="locatingResult"
            @open-file="openResultLocation"
            @quick-analysis="openQuickAnalysis()"
          />
        </KeepAlive>

        <!-- 缓存面板激活时，其余面板链整体不渲染（否则末尾欢迎页会同时出现）。 -->
        <template v-if="!cachedPanelActive">
        <DiskImageInfo
          v-if="activePanel === 'dashboard' && imageId"
          :info="imageInfo"
          @open-image="onOpenImageFromInfo"
        />

        <BcpPanel
          v-else-if="activePanel.startsWith('bcp') && imageId"
          :image-id="imageId"
          :catalog="bcpCatalogData"
          :catalog-error="bcpCatalogError"
          :preferred-file="bcpPreferredFile"
          :category="activePanel.startsWith('bcp:') ? activePanel.slice(4) : ''"
        />

        <WelcomeScreen
          v-else
          :busy="loading || isSwitching || isOpeningRecentCase"
          :dragging="isDragOver"
          :load-label="loading ? loadLabel : isOpeningRecentCase ? tr('正在打开案件…') : loadLabel"
          :load-percent="loading && loadProgress?.total ? loadPercent : null"
          :recent-cases="recentCases"
          @open="handleOpenFile"
          @open-case="showCaseManager = true"
          @open-recent="handleOpenRecentCase"
          @clear-recent="clearRecentCases"
        />
        </template>
        </template>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { tr } from './composables/useLocale';

import './styles/workspace.css';
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import { vContentMotion } from './composables/uiMotionDirectives';
import WelcomeScreen from './app/components/WelcomeScreen.vue';
import EvidenceLoadPanel from './app/components/EvidenceLoadPanel.vue';
import { useResultNavigation } from './app/workflows/useResultNavigation';
import { type FileListNavigator } from './features/files/model/navigation';
import TaskHub from './features/tasks/components/TaskHub.vue';
import { useRecentCases } from './features/cases/api';
import Titlebar from './app/components/Titlebar.vue';
import ToolSettingsModal from './features/settings/components/ToolSettingsModal.vue';
import CaseManagerModal from './features/cases/components/CaseManagerModal.vue';
import AddImageDialog from './features/evidence/components/AddImageDialog.vue';
import PhysicalDiskPicker from './features/evidence/components/PhysicalDiskPicker.vue';
import RaidMemberDetectDialog from './features/raid/components/RaidMemberDetectDialog.vue';
import SidebarNav from './app/components/SidebarNav.vue';
import AnalysisResultsNav from './features/analysis/components/AnalysisResultsNav.vue';
import { BcpPanel, CarvePanel, DiskFileList, DiskImageInfo, AnalysisResultsPanel, SearchResultsPanel, TimelinePanel, KEPT_ALIVE_PANELS } from './app/workspace/panels';
import { ListTodo, Upload } from 'lucide-vue-next';
import { useFileSession } from './features/evidence/api';
import { useCaseSession } from './features/cases/api';
import { useWorkspaceShell } from './app/workspace/useWorkspaceShell';
import { useRaidMemberOpen } from './app/workflows/useRaidMemberOpen';
import { useEvidenceOpen } from './app/workflows/useEvidenceOpen';
import { useCaseWorkspace } from './app/workflows/useCaseWorkspace';
import { bindWorkspaceServices } from './app/workflows/bindWorkspaceServices';
import { useBcpWorkspace } from './app/workspace/useBcpWorkspace';
import { useWorkspaceTaskHub } from './app/workspace/useWorkspaceTaskHub';
import { useTabManager } from './app/workspace/useTabManager';
import { notifyError } from './composables/useToast';
import { useAppUpdater } from './features/settings/api';
import { bcpNavGroup } from './app/workspace/bcpNavigation';
import type { SettingsSection } from './features/settings/model/types';

const { recentCases, clearRecentCases } = useRecentCases();
const { checkForUpdates } = useAppUpdater();
const showToolSettings = ref(false);
const settingsInitialSection = ref<SettingsSection>('appearance');

function openSettings(section: SettingsSection = 'appearance') {
  settingsInitialSection.value = section;
  showToolSettings.value = true;
}

const {
  isEvidenceMode,
  loading,
  loadProgress,
  imageId,
  imageInfo,
  updateImageInfo,
  pickFolder,
  pickBcpFiles,
  pickFiles,
  openByPath,
  resetSession: resetFileSession,
  captureSnapshot: captureFileSession,
  restoreSnapshot: restoreFileSession,
} = useFileSession();

// Forensic case + volume-snapshot session (global singleton). The active
// case name shows in the titlebar; the manager modal handles create/open/
// close + per-volume snapshot build/list.
const { activeCase, refreshActive: refreshActiveCase, removeEvidence } = useCaseSession();
const supportsRawAnalysis = computed(() => !imageInfo.value?.index_only && imageInfo.value?.source_format !== 'folder');
const { bcpCatalogData, bcpCatalogError, preferredBcpFiles, bcpPreferredFile } = useBcpWorkspace(imageId, imageInfo);
const {
  showRaidMemberDialog, raidMemberQueue, raidMemberStack, raidMemberDetecting,
  raidMemberOpening, raidMemberError, openRaidMemberDialog, addRaidMemberFiles, addRaidMemberPaths,
  detectRaidMemberQueue, removeRaidMember, confirmRaidMemberOpen, detectRaid,
} = useRaidMemberOpen({
  activeCase, removeEvidence, pickFiles,
  openImagePath: (path, options) => openImagePath(path, options),
});
// The "检测到完整 RAID 阵列，是否打开重组镜像？" confirm dialog lives in the
// shared composable; opening the tab needs this shell's tab manager.
const showCaseManager = ref(false);
const {
  taskActiveCount, taskHubOpen, taskHubMounted, taskHubTab,
  taskHubEvidence, taskHubEvidenceName, closeTaskHub, openTaskResult, toggleTaskHub, openQuickAnalysis,
} = useWorkspaceTaskHub({ activeCase, imageId, showCaseManager, showPanel: panel => { activePanel.value = panel; } });

const {
  showAddImageDialog, showPhysicalDiskPicker, onPhysicalDiskSelect,
  onPhysicalDiskPickerClose, onAddImageSelect, onOpenImageFromInfo,
} = useEvidenceOpen({
  pickFiles, pickFolder, pickBcpFiles, openRaidMemberDialog, preferredBcpFiles,
  openImagePath: (path, options) => openImagePath(path, options),
  showPanel: panel => { activePanel.value = panel; },
});

const { onOpenCaseEvidence, onCaseEvidenceRemoved } = useCaseWorkspace({
  openImagePath: (path, options) => openImagePath(path, options),
  detectRaid,
  tabs: () => tabs.value,
  evidence: () => activeCase.value?.evidence ?? [],
  caseDirectory: () => activeCase.value?.dir ?? null,
  closeTab: id => { void closeTab(id); },
});

const {
  activePanel,
  isDragOver,
  sidebarGroups,
  handleOpenFile,
  handleOpenRecentCase,
  isOpeningRecentCase,
  openImagePath,
  handleSidebarSelect,
  clearPendingOpenRequest,
  loadFileForTab,
} = useWorkspaceShell({
  isEvidenceMode,
  supportsRawAnalysis,
  openByPath,
  onNeedCase: () => { showCaseManager.value = true; },
  onOpenCaseEvidence,
  onBcpFileOpen: (folder, fileName) => preferredBcpFiles.set(folder, fileName),
  onRequestAddImage: () => { showAddImageDialog.value = true; },
  onCaseGateSatisfied: () => { showCaseManager.value = false; },
  onDroppedPaths: async paths => {
    if (!showRaidMemberDialog.value) return false;
    await addRaidMemberPaths(paths);
    return true;
  },
});

// Search and timeline share evidence-aware navigation, including virtual file nodes.
const diskFileListRef = ref<FileListNavigator | null>(null);

function activateFilePreview(previewImageId: string): boolean {
  if (imageId.value !== previewImageId) return false;
  activePanel.value = 'files';
  return true;
}

const { locating: locatingResult, openLocation: openResultLocation } = useResultNavigation({
  caseKey: () => activeCase.value?.dir ?? null,
  imageId: () => imageId.value,
  openImage: async targetImageId => {
    const evidence = activeCase.value?.evidence.find(item => item.image_id === targetImageId);
    if (!evidence) throw new Error(tr("来源检材不在当前案件中"));
    const result = await openImagePath(evidence.path, { forceImage: !evidence.path.startsWith('assembled://'), quiet: true, waitUntilReady: true });
    if (!result.ok) throw new Error(tr("无法打开来源检材：{0}", { 0: result.error }));
    return true;
  },
  showFiles: () => { activePanel.value = 'files'; },
  isFilesActive: () => activePanel.value === 'files',
  panel: () => diskFileListRef.value,
  onError: message => notifyError(tr("定位失败"), message),
});

function handleCaseManagerClose() {
  showCaseManager.value = false;
  clearPendingOpenRequest();
}

// 「检测到 BitLocker 加密分区」提示的「前往解锁」按钮把用户带到镜像信息面板
// （BitLocker 解锁控件所在）。useFileSession 不持有面板状态，故在此注入跳转。

// ── Multi-tab orchestrator wiring ─────────────────────────────────────
//
// Tabbar UI lives in Titlebar.vue; the singleton state lives in
// `useTabManager`. We bridge the two here by registering the shell's
// load / teardown / panel hooks. Doing this once at App setup keeps
// `useTabManager` itself decoupled from the workspace details — the
// composable is purely a state machine over `tabs[]`.
const { tabs, visibleTabs, activeTab, activeTabId, isSwitching, activateTab, closeTab, closeAllTabs } = useTabManager();

/** Tab × click → tear the tab down. */
function handleTabClose(tabId: string): void {
  const tab = tabs.value.find(t => t.id === tabId);
  if (!tab) return;
  closeTab(tabId);
}
bindWorkspaceServices({
  resetFileSession, captureFileSession, restoreFileSession, updateImageInfo, loadFileForTab, loading,
  activePanel, showToolSettings, closeAllTabs, handleSidebarSelect, onOpenImageFromInfo,
});

const loadPercent = computed(() => {
  if (!loadProgress.value?.total) return 0;
  return Math.min(100, Math.round((loadProgress.value.current / loadProgress.value.total) * 100));
});

// Loading-overlay text: phase + percent when a total is known. Falls back to
// a generic spinner label before any image-load progress event arrives.
const loadLabel = computed(() => {
  const p = loadProgress.value;
  if (!p) return tr("正在载入镜像...");
  let label = p.stage;
  if (p.total > 0) label += ` ${loadPercent.value}%`;
  if (p.entries > 0) label += tr(" · {0} 条", { 0: p.entries.toLocaleString() });
  return label;
});

const displaySidebarGroups = computed(() => bcpCatalogData.value
  ? [...sidebarGroups.value, bcpNavGroup(bcpCatalogData.value)]
  : sidebarGroups.value);
const showSidebarNav = computed(() => displaySidebarGroups.value.length > 0);

/** Panels kept alive by the `<KeepAlive>` block; while one of them is shown
 *  the dashboard / welcome chain below must not render. */
const cachedPanelActive = computed(() => Boolean(imageId.value) && KEPT_ALIVE_PANELS.has(activePanel.value));

// Right-click is silent by default: only editable fields keep the native
// copy / paste menu. File rows, the hex view and other surfaces with their own
// menus handle `contextmenu` themselves.
function suppressNativeContextMenu(event: MouseEvent) {
  const target = event.target as HTMLElement | null;
  if (target?.closest('input, textarea') || target?.isContentEditable) return;
  event.preventDefault();
}

onMounted(() => {
  void refreshActiveCase();
  void checkForUpdates();
  document.addEventListener('contextmenu', suppressNativeContextMenu);
});

onBeforeUnmount(() => {
  document.removeEventListener('contextmenu', suppressNativeContextMenu);
});
</script>
