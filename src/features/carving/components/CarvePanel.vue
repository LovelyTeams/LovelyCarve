<template>
  <section class="global-search-panel" :aria-label="tr('文件恢复')">
    <UiTransition kind="horizontal"><CarveHistoryList v-show="historyVisible" :scans="scans" :loading-history="loadingHistory" :history-error="historyError" :scan="scan"
      :recovering="recovering" :deleting-scan-id="deletingScanId" @close="historyVisible = false" @select="selectScan" @delete="deleteScan" /></UiTransition>
    <div class="result-pane">
      <div v-if="historyError && scan" class="ui-alert danger result-read-error" role="alert"><CircleAlert :size="14" /><span>{{ tr("读取扫描记录失败：") }}{{ historyError }}</span><button class="ui-btn sm" :disabled="loadingHistory" @click="history.refreshHistory(imageId)">{{ tr("重试") }}</button></div>
      <template v-if="scan">
        <!-- 页面不设标题栏：顶部工具条放筛选与操作，扫描范围、状态与计数放在底部状态栏。 -->
        <form class="ui-toolbar result-toolbar result-filters" @submit.prevent="applyFilters">
          <button
            class="ui-btn ghost icon history-switch"
            type="button"
            :aria-pressed="historyVisible"
            :title="historyVisible ? tr('收起扫描记录') : tr('展开扫描记录')"
            :aria-label="tr('扫描记录')"
            @click="historyVisible = !historyVisible"
          ><PanelLeft :size="15" /></button>
          <label class="ui-search filter-input" :class="{ applied: query }">
            <Filter :size="14" />
            <input v-model="queryDraft" type="search" :placeholder="tr('筛选文件名、路径、类型或偏移，回车应用')" :aria-label="tr('筛选恢复结果')" @keydown.enter.prevent="applyFilters" />
            <button v-if="query || queryDraft" class="ui-icon-btn sm" type="button" :title="tr('清除筛选')" :aria-label="tr('清除筛选')" @click="clearFilters"><X :size="13" /></button>
          </label>
          <UiSelect v-model="categoryFilterDraft" class="distinct-select" :class="{ applied: categoryFilter !== 'all' }" :options="localizeOptions(categoryFilterOptions)" :aria-label="tr('结果类别')" @change="applyFilters" />
          <div class="ui-toolbar-actions">
            <button v-if="scan.status === 'running'" class="ui-btn" type="button" @click="pauseScan"><Pause :size="13" />{{ tr("暂停") }}</button>
            <button v-if="scan.status === 'paused'" class="ui-btn" type="button" @click="resumeScan"><Play :size="13" />{{ tr("继续") }}</button>
            <button v-if="isActive" class="ui-btn danger-soft" type="button" @click="cancelScan"><X :size="13" />{{ tr("取消扫描") }}</button>
            <button class="ui-btn" type="button" :disabled="!canRecover || recovering" @click="checkedCount ? recoverChecked() : recoverEverything()"><Loader2 v-if="recovering" :size="13" class="spin" /><Download v-else :size="13" />{{ checkedCount ? tr("恢复所选（{0}）", { 0: checkedCount }) : tr("恢复全部") }}</button>
            <button class="ui-btn primary" type="button" :disabled="initializing || hasActiveScan || recovering" @click="configExpanded = true"><Plus :size="14" />{{ tr("新建扫描") }}</button>
          </div>
        </form>
        <!-- 目录树、文件表与状态栏在左，单击文件后详情从右侧展开。 -->
        <div class="result-body">
          <div class="result-main">
            <div v-if="filesystemError" class="ui-alert danger result-read-error" role="alert"><CircleAlert :size="14" /><span>{{ tr("读取文件系统恢复结果失败：") }}{{ filesystemError }}</span><button class="ui-btn sm" :disabled="filesystemLoading" @click="retryFilesystem">{{ tr("重试") }}</button></div>
            <div v-if="resultsError" class="ui-alert danger result-read-error" role="alert"><CircleAlert :size="14" /><span>{{ tr("读取雕刻结果失败：") }}{{ resultsError }}</span><button class="ui-btn sm" :disabled="syncingResults" @click="retryResults">{{ tr("重试") }}</button></div>
            <div v-if="isActive" class="status-banner" :class="scan.status === 'paused' ? 'warn' : 'running'" role="status">
              <Pause v-if="scan.status === 'paused'" :size="13" /><Loader2 v-else :size="13" class="spin" /><span>{{ progressHeadline }}<template v-if="!filesystemPhase && scan.status !== 'queued'"> · {{ formatBytes(scan.bytes_scanned) }} / {{ formatBytes(scan.bytes_total) }}</template></span>
              <div class="banner-progress" :class="{ indeterminate: filesystemPhase && !scan.filesystem?.total && scan.status === 'running' }" role="progressbar" :aria-label="tr('恢复扫描进度')" :aria-valuenow="progressPercent" aria-valuemin="0" aria-valuemax="100"><span :style="{ width: `${progressPercent}%` }"></span></div>
              <span class="banner-percent mono">{{ progressPercent.toFixed(1) }}%</span>
            </div>
            <div v-if="scan.status === 'failed'" class="status-banner error" role="alert"><CircleAlert :size="13" /><span>{{ tr("扫描失败：") }}{{ scan.error || tr("未知错误") }}</span></div>
            <div v-if="scan.status === 'cancelled'" class="status-banner warn" role="status"><CircleSlash :size="13" /><span>{{ tr("扫描已取消，以下为部分结果") }}</span></div>
            <div v-if="scan.filesystem?.error && scan.status !== 'failed'" class="status-banner error" role="alert"><CircleAlert :size="13" /><span>{{ tr("文件系统扫描失败：") }}{{ scan.filesystem.error }}</span></div>
            <div v-for="volume in failedVolumes" :key="volume.key" class="status-banner warn" role="alert"><AlertTriangle :size="13" /><span>{{ volume.name }} · LBA {{ volume.volume!.partition.start_lba }}：{{ volume.volume!.error }}</span></div>
            <div v-if="scan.filesystem?.truncated" class="status-banner warn" role="alert"><AlertTriangle :size="13" /><span>{{ tr("文件系统记录达到安全上限，结果不完整。请缩小扫描范围。") }}</span></div>
            <div v-if="scan.filesystem?.read_faults_detected" class="status-banner warn" role="alert"><AlertTriangle :size="13" /><span>{{ tr("文件系统扫描遇到读取故障，部分记录可能缺失或来自合成数据。") }}</span></div>
            <div v-if="scan.truncated" class="status-banner warn" role="alert"><AlertTriangle :size="13" /><span>{{ tr("签名雕刻已达到结果数量上限，以下结果仍可恢复") }}</span></div>
            <div v-if="recoveryProgress" class="status-banner running" role="status"><Loader2 :size="13" class="spin" /><span>{{ recoveryProgress.message || tr("正在恢复文件") }}</span><div class="banner-progress"><span :style="{ width: `${recoveryPercent}%` }"></span></div><button class="ui-btn link sm" @click="cancelRecovery">{{ tr("取消恢复") }}</button></div>
            <div v-if="recoveryResult && recoveryTotals" class="status-banner recovery-result" :class="recoveryTotals.failed || recoveryResult.report_error || recoveryResult.files?.read_faults.read_faults_detected ? 'warn' : 'running'" role="status">
              <CheckCircle2 :size="13" />
              <span>{{ tr("已恢复") }} {{ recoveryTotals.recovered }} {{ tr("个文件") }}<template v-if="recoveryTotals.failed">{{ tr("，失败") }} {{ recoveryTotals.failed }} {{ tr("个") }}</template><template v-if="recoveryResult.report_error">{{ tr("；报告生成失败：") }}{{ recoveryResult.report_error }}</template><template v-else-if="recoveryResult.report_path"> · DFXML：{{ recoveryResult.report_path }}</template><template v-if="recoveryResult.files"> · {{ tr("恢复报告") }}：{{ recoveryResult.files.report_path }}</template><template v-if="recoveryResult.files?.read_faults.read_faults_detected"> · {{ tr("读取故障产生了合成数据，请查看恢复报告。") }}</template></span>
            </div>
            <RecoveryBrowser :tree-rows="treeRows" :expanded="expanded" :current-key="currentKey" :breadcrumbs="breadcrumbs" :rows="rows"
              :signature-columns="signatureColumns" :selected-key="selectedKey" :table-context="tableContext"
              :is-checked="browser.isChecked" :is-partial="browser.isPartiallyChecked" :all-checked="allVisibleChecked" :some-checked="someVisibleChecked"
              :busy="recovering" :loading="tableLoading" :filtered="hasFilters" :empty-text="emptyText" :format-label="formatLabel" :sector-size="imageInfo?.sector_size ?? 512"
              @navigate="browser.navigate" @toggle-expanded="browser.toggleExpanded" @go-up="browser.goUp" @select="selectedKey = $event.key"
              @toggle="browser.toggleChecked" @toggle-all="browser.toggleAllVisible" @clear-filters="clearFilters" />
            <footer class="ui-footbar result-footer">
              <span class="result-summary footer-task" :title="`${scan.scope_label} · ${formatScanTime(scan, locale)}`"><FileSearch :size="12" /><span>{{ scan.scope_label }}</span></span>
              <span class="ui-badge sm status-badge" :class="scanStatusTone">{{ statusLabel(scan.status, tr) }}</span>
              <span v-if="scan.filesystem" class="ui-footbar-stat">{{ tr("文件系统") }} <strong :key="scan.filesystem.files" class="ui-bump">{{ scan.filesystem.files.toLocaleString() }}</strong> {{ tr("个文件") }}</span>
              <template v-if="scan.signatures !== false">
                <span class="ui-footbar-stat">{{ tr("扫描") }} <strong>{{ formatBytes(scan.bytes_scanned) }}</strong> / {{ formatBytes(scan.bytes_total) }}</span>
                <span class="ui-footbar-stat">{{ tr("签名雕刻") }} <strong :key="scan.artifacts_found" class="ui-bump">{{ scan.artifacts_found.toLocaleString() }}</strong> {{ tr("个文件") }}</span>
              </template>
              <span v-if="checkedCount" class="ui-footbar-stat footer-checked">{{ tr("已勾选") }} <strong>{{ checkedCount.toLocaleString() }}</strong> {{ tr("项") }} · {{ formatBytes(checkedBytes) }}</span>
              <span class="ui-footbar-spacer"></span>
              <span class="footer-total mono"><Loader2 v-if="syncingResults" :size="11" class="spin" />{{ syncingResults ? tr("正在读取雕刻结果 {0} / {1}", { 0: artifacts.length.toLocaleString(), 1: resultTotal.toLocaleString() }) : tr("当前列表 {0} 项", { 0: rows.length.toLocaleString() }) }}</span>
            </footer>
          </div>
          <UiTransition kind="dock">
            <CarveResultDetail v-if="selectedEntry?.artifact" :artifact="selectedEntry.artifact" :offset="formatOffset(selectedEntry.artifact.offset)" :preview-url="previewUrl" :preview-message="previewMessage" :loading="previewLoading" :recovering="recovering" @recover="recoverArtifact" @close="selectedKey = null" />
            <RecoveryFileDetail v-else-if="selectedFile" :entry="selectedFile" :volume="selectedVolume?.volume ?? null" :volume-name="selectedVolume?.name ?? ''" :recovering="recovering" @recover="recoverFile" @close="selectedKey = null" />
          </UiTransition>
        </div>
      </template>
      <WorkspaceStartState v-else :icon="ScanSearch" :loading="initializing || loadingHistory"
        :title="initializationError || historyError ? tr('读取扫描记录失败') : initializing || loadingHistory ? tr('正在读取扫描记录…') : tr('还没有恢复扫描')"
        :description="initializationError || historyError || tr('读取删除记录、重建遗失分区，并按文件签名查找残留文件')"
        :action-label="initializationError || historyError ? tr('重试') : tr('新建扫描')" :action-icon="initializationError || historyError ? RefreshCw : Plus" @action="initializationError || historyError ? initializePanel() : configExpanded = true">
        <template #toolbar><button class="ui-btn ghost" :aria-pressed="historyVisible" @click="historyVisible = !historyVisible"><PanelLeft :size="14" />{{ tr("扫描记录") }}</button></template>
      </WorkspaceStartState>
    </div>
    <CarveCreateDialog :open="configExpanded" :formats="formats" :format-groups="formatGroups" :selected-format-ids="selectedFormatIds"
      :initializing="initializing" :starting="starting" :can-start="canStart" :partitions="partitions" :partition-options="partitionOptions" :elements="configuration.elements"
      :sector-size="sectorSize" :range-error="rangeError" :method-error="methodError"
      v-model:scope="scope" v-model:partition-id="partitionId" v-model:start-sector="startSector" v-model:sector-count="sectorCount"
      v-model:max-file-size="maxFileSize" v-model:max-results="maxResults" v-model:filesystems="filesystems" v-model:signatures="signatures"
      @close="closeConfiguration" @start="startScan" @select-all="selectAllFormats" @clear="clearFormats" @toggle-group="toggleGroup" @toggle-format="toggleFormat" />
  </section>
</template>

<script setup lang="ts">
const { locale } = useLocale();
import { tr, localizeOptions, useLocale } from '../../../composables/useLocale';

import { computed, onMounted, onScopeDispose, ref } from 'vue';
import UiSelect from '../../../components/ui/UiSelect.vue';
import UiTransition from '../../../components/ui/UiTransition.vue';
import WorkspaceStartState from '../../../components/layout/WorkspaceStartState.vue';
import CarveResultDetail from './CarveResultDetail.vue';
import CarveHistoryList from './CarveHistoryList.vue';
import CarveCreateDialog from './CarveCreateDialog.vue';
import RecoveryBrowser from './RecoveryBrowser.vue';
import RecoveryFileDetail from './RecoveryFileDetail.vue';
import { AlertTriangle, RefreshCw, CheckCircle2, Download, FileSearch, Loader2, Pause, Play, Filter, Plus, ScanSearch, PanelLeft, CircleAlert, CircleSlash, X } from 'lucide-vue-next';
import type { RecoveryCandidate } from '../../../lib/ipc/carve';
import type { ImageInfo } from '../../../lib/ipc/image';
import { formatBytes } from '../../../utils/format';
import { statusLabel, formatScanTime, formatOffset, categoryFilterOptions } from '../model/carveView';
import { RECOVERY_ROOT, SIGNATURE_ROOT, type RecoveryEntry } from '../model/recoveryTree';
import { useCarveConfiguration } from '../composables/useCarveConfiguration';
import { useCarveHistory } from '../composables/useCarveHistory';
import { useCarveResults } from '../composables/useCarveResults';
import { useCarveFilesystem } from '../composables/useCarveFilesystem';
import { useRecoveryBrowser } from '../composables/useRecoveryBrowser';
import { useCarvePreview } from '../composables/useCarvePreview';
import { useCarveRecovery } from '../composables/useCarveRecovery';
const props = defineProps<{ imageId: string; imageInfo: ImageInfo | null }>();
const initializing = ref(true);
const initializationError = ref('');
// Selecting a record starts reading its results; the drain must not hold the page's initialization.
const history = useCarveHistory({ imageId: () => props.imageId, recovering: () => recovering.value,
  onSelected: async () => { void results.loadFirstPage(); }, onReset: () => results.resetResults(), onStarted: () => { configExpanded.value = false; },
  onMoreResults: () => { if (scan.value && scan.value.artifacts_found > artifacts.value.length) results.syncNewResults(); } });
const { historyVisible, scans, loadingHistory, historyError, deletingScanId, scan, starting, isActive, hasActiveScan, filesystemPhase,
  progressPercent, progressHeadline, scanStatusTone, selectScan, deleteScan, cancelScan, pauseScan, resumeScan } = history;
const results = useCarveResults({ imageId: () => props.imageId, scan: () => scan.value });
const { artifacts, resultTotal, syncingResults, resultsError, retryResults } = results;
const filesystemResults = useCarveFilesystem({ scan: () => scan.value });
const { result: filesystemResult, loading: filesystemLoading, error: filesystemError, retry: retryFilesystem } = filesystemResults;
const configuration = useCarveConfiguration({ imageId: () => props.imageId, imageInfo: () => props.imageInfo, starting: () => starting.value });
const { formats, formatGroups, selectedFormatIds, scope, partitionId, startSector, sectorCount, sectorSize, maxFileSize, maxResults, filesystems, signatures,
  partitions, partitionOptions, rangeError, methodError, configExpanded, toggleGroup, toggleFormat, selectAllFormats, clearFormats, closeConfiguration } = configuration;
function formatLabel(formatId: string) {
  return formats.value.find(format => format.id === formatId)?.label ?? formatId.toUpperCase();
}
const browser = useRecoveryBrowser({
  context: () => scan.value?.scan_id ?? null, filesystem: () => filesystemResult.value, artifacts: () => artifacts.value,
  signatures: () => scan.value?.signatures !== false, formatLabel,
});
const { currentKey, breadcrumbs, rows, treeRows, expanded, selectedKey, selectedEntry, checkedKeys, query, queryDraft, categoryFilter, categoryFilterDraft,
  hasFilters, allVisibleChecked, someVisibleChecked, checkedFileIds, checkedArtifactIds, checkedBytes, recoverableFileIds, applyFilters, clearFilters } = browser;
const recovery = useCarveRecovery({ imageId: () => props.imageId, scan: () => scan.value });
const { recovering, recoveryProgress, recoveryResult, recoveryTotals, recoveryPercent, cancelRecovery } = recovery;
const { previewLoading, previewUrl, previewMessage } = useCarvePreview({ imageId: () => props.imageId, scan: () => scan.value, artifact: () => selectedEntry.value?.artifact ?? null });

const checkedCount = computed(() => checkedKeys.value.size);
const canRecover = computed(() => (scan.value?.artifacts_found ?? 0) > 0 || recoverableFileIds.value.length > 0);
const signatureColumns = computed(() => currentKey.value === SIGNATURE_ROOT || currentKey.value.startsWith(`${SIGNATURE_ROOT}:`));
const tableContext = computed(() => `${scan.value?.scan_id}:${currentKey.value}:${query.value}:${categoryFilter.value}`);
const tableLoading = computed(() => filesystemLoading.value || (syncingResults.value && artifacts.value.length === 0) || (isActive.value && rows.value.length === 0 && currentKey.value === RECOVERY_ROOT));
const emptyText = computed(() => {
  if (tableLoading.value) return isActive.value ? tr("正在扫描，发现的卷和文件会显示在这里") : tr("正在读取恢复结果…");
  if (hasFilters.value) return tr("没有符合当前筛选条件的文件");
  if (currentKey.value === RECOVERY_ROOT) return tr("本次扫描没有发现可恢复的文件");
  return tr("此目录为空");
});
const failedVolumes = computed(() => browser.childrenOf(RECOVERY_ROOT).filter(item => item.volume?.error));
const selectedFile = computed(() => selectedEntry.value?.candidate ? selectedEntry.value as RecoveryEntry & { candidate: RecoveryCandidate } : null);
const selectedVolume = computed(() => selectedFile.value ? browser.lookup(`v:${selectedFile.value.volumeIndex}`) : null);

function recoverChecked() {
  void recovery.recover({ artifactIds: checkedArtifactIds.value, fileIds: checkedFileIds.value });
}
function recoverEverything() {
  void recovery.recover({ artifactIds: (scan.value?.artifacts_found ?? 0) > 0 ? null : [], fileIds: recoverableFileIds.value });
}
function recoverArtifact(id: string) { void recovery.recover({ artifactIds: [id], fileIds: [] }); }
function recoverFile(entry: RecoveryEntry) {
  if (entry.candidate) void recovery.recover({ artifactIds: [], fileIds: [entry.candidate.id] });
}

const canStart = computed(() => !initializing.value && !hasActiveScan.value && !recovering.value && !methodError.value && !rangeError.value
  && (scope.value !== 'partition' || partitionId.value != null));
function startScan() { if (canStart.value) void history.startScan(configuration.buildOptions()); }
let disposed = false;
async function initializePanel() {
  initializing.value = true; initializationError.value = '';
  try { await configuration.loadFormats(); if (!disposed) await history.refreshHistory(props.imageId); }
  catch (error) { if (!disposed) initializationError.value = String(error); }
  finally { if (!disposed) initializing.value = false; }
}
onMounted(() => { void initializePanel(); });
onScopeDispose(() => { disposed = true; });
</script>


<style scoped src="../../../styles/analysis-workspace.css"></style>
<style scoped src="../styles/carve-workspace.css"></style>
<style scoped>
.recovery-result span { min-width: 0; overflow-wrap: anywhere; }
</style>
