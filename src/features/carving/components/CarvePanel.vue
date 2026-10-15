<template>
  <section class="global-search-panel" :aria-label="tr('文件雕刻')">
    <UiTransition kind="horizontal"><CarveHistoryList v-show="historyVisible" :scans="scans" :loading-history="loadingHistory" :history-error="historyError" :scan="scan"
      :recovering="recovering" :deleting-scan-id="deletingScanId" @close="historyVisible = false" @select="selectScan" @delete="deleteScan" /></UiTransition>
    <div class="result-pane">
      <div v-if="historyError && scan" class="ui-alert danger result-read-error" role="alert"><CircleAlert :size="14" /><span>{{ tr("读取雕刻记录失败：") }}{{ historyError }}</span><button class="ui-btn sm" :disabled="loadingHistory" @click="history.refreshHistory(imageId)">{{ tr("重试") }}</button></div>
      <template v-if="scan">
        <!-- 页面不设标题栏：顶部工具条放筛选与操作，扫描范围、状态与计数放在底部状态栏。 -->
        <form class="ui-toolbar result-toolbar result-filters" @submit.prevent="applyFilters">
          <button
            class="ui-btn ghost icon history-switch"
            type="button"
            :aria-pressed="historyVisible"
            :title="historyVisible ? tr('收起雕刻记录') : tr('展开雕刻记录')"
            :aria-label="tr('雕刻记录')"
            @click="historyVisible = !historyVisible"
          ><PanelLeft :size="15" /></button>
          <label class="ui-search filter-input" :class="{ applied: query }">
            <Filter :size="14" />
            <input v-model="queryDraft" type="search" :placeholder="tr('筛选文件名、类型或偏移，回车应用')" :aria-label="tr('筛选雕刻结果')" @keydown.enter.prevent="applyFilters" />
            <button v-if="query || queryDraft" class="ui-icon-btn sm" type="button" :title="tr('清除筛选')" :aria-label="tr('清除筛选')" @click="clearFilters"><X :size="13" /></button>
          </label>
          <UiSelect v-model="categoryFilterDraft" class="distinct-select" :class="{ applied: categoryFilter !== 'all' }" :options="localizeOptions(categoryFilterOptions)" :aria-label="tr('结果类别')" @change="applyFilters" />
          <div class="ui-toolbar-actions">
            <button v-if="scan.status === 'running'" class="ui-btn" type="button" @click="pauseScan"><Pause :size="13" />{{ tr("暂停") }}</button>
            <button v-if="scan.status === 'paused'" class="ui-btn" type="button" @click="resumeScan"><Play :size="13" />{{ tr("继续") }}</button>
            <button v-if="isActive" class="ui-btn danger-soft" type="button" @click="cancelScan"><X :size="13" />{{ tr("取消扫描") }}</button>
            <button class="ui-btn" type="button" :disabled="!scan.artifacts_found || recovering" @click="checkedIds.size ? recoverSelected() : recoverAll()"><Loader2 v-if="recovering" :size="13" class="spin" /><Download v-else :size="13" />{{ checkedIds.size ? tr("恢复所选（{0}）", { 0: checkedIds.size }) : tr("恢复全部") }}</button>
            <button class="ui-btn primary" type="button" :disabled="initializing || hasActiveScan || recovering" @click="configExpanded = true"><Plus :size="14" />{{ tr("新建扫描") }}</button>
          </div>
        </form>
        <!-- 表格与状态栏在左，单击结果后详情从右侧展开。 -->
        <div class="result-body">
          <div class="result-main">
            <div v-if="resultsError" class="ui-alert danger result-read-error" role="alert"><CircleAlert :size="14" /><span>{{ tr("读取雕刻结果失败：") }}{{ resultsError }}</span><button class="ui-btn sm" :disabled="loadingMore || syncingResults" @click="retryResults">{{ tr("重试") }}</button></div>
            <div v-if="hasFilters && (loadingMore || syncingResults)" class="status-banner running" role="status"><Loader2 :size="13" class="spin" /><span>{{ tr("正在读取全部结果以完成筛选，已加载") }} {{ artifacts.length.toLocaleString() }} / {{ resultTotal.toLocaleString() }} {{ tr("条") }}</span><button class="ui-btn sm ghost" @click="clearFilters">{{ tr("清除筛选") }}</button></div>
            <div v-if="isActive" class="status-banner" :class="scan.status === 'paused' ? 'warn' : 'running'" role="status">
              <Pause v-if="scan.status === 'paused'" :size="13" /><Loader2 v-else :size="13" class="spin" /><span>{{ progressHeadline }} · {{ formatBytes(scan.bytes_scanned) }} / {{ formatBytes(scan.bytes_total) }}</span>
              <div class="banner-progress" role="progressbar" :aria-label="tr('雕刻扫描进度')" :aria-valuenow="progressPercent" aria-valuemin="0" aria-valuemax="100"><span :style="{ width: `${progressPercent}%` }"></span></div>
              <span class="banner-percent mono">{{ progressPercent.toFixed(1) }}%</span>
            </div>
            <div v-if="scan.status === 'failed'" class="status-banner error" role="alert"><CircleAlert :size="13" /><span>{{ tr("扫描失败：") }}{{ scan.error || tr("未知错误") }}</span></div>
            <div v-if="scan.status === 'cancelled'" class="status-banner warn" role="status"><CircleSlash :size="13" /><span>{{ tr("扫描已取消，以下为部分结果") }}</span></div>
            <div v-if="scan.truncated" class="status-banner warn" role="alert"><AlertTriangle :size="13" /><span>{{ tr("已达到结果数量上限，以下结果仍可恢复") }}</span></div>
            <div v-if="recoveryProgress" class="status-banner running" role="status"><Loader2 :size="13" class="spin" /><span>{{ recoveryProgress.message || tr("正在恢复文件") }}</span><div class="banner-progress"><span :style="{ width: `${recoveryPercent}%` }"></span></div><button class="ui-btn link sm" @click="cancelRecovery">{{ tr("取消恢复") }}</button></div>
            <div v-if="recoveryResult" class="status-banner" :class="recoveryResult.failed || recoveryResult.report_error ? 'warn' : 'running'" role="status"><CheckCircle2 :size="13" /><span>{{ tr("已恢复") }} {{ recoveryResult.recovered }} {{ tr("个文件") }}<template v-if="recoveryResult.failed">{{ tr("，失败") }} {{ recoveryResult.failed }} {{ tr("个") }}</template><template v-if="recoveryResult.report_error">{{ tr("；报告生成失败：") }}{{ recoveryResult.report_error }}</template><template v-else-if="recoveryResult.report_path"> · DFXML：{{ recoveryResult.report_path }}</template></span></div>
            <CarveResultTable :visible-artifacts="visibleArtifacts" :all-visible-selected="allVisibleSelected" :checked-ids="checkedIds" :selected-id="selectedId"
              :recovering="recovering" :syncing-results="syncingResults || loadingMore" :read-error="resultsError" :filter-incomplete="hasFilters && Boolean(nextCursor)" :context-key="`${scan.scan_id}:${query}:${categoryFilter}`" :is-active="isActive" :scan="scan" :has-filters="hasFilters"
              @select="selectedId = $event" @recover="recoverOne" @toggle-checked="toggleChecked" @toggle-all="toggleAllVisible" @clear-filters="clearFilters" />
            <footer class="ui-footbar result-footer">
              <span class="result-summary footer-task" :title="`${scan.scope_label} · ${formatScanTime(scan, locale)}`"><FileSearch :size="12" /><span>{{ scan.scope_label }}</span></span>
              <span class="ui-badge sm status-badge" :class="scanStatusTone">{{ statusLabel(scan.status, tr) }}</span>
              <span class="ui-footbar-stat">{{ tr("扫描") }} <strong>{{ formatBytes(scan.bytes_scanned) }}</strong> / {{ formatBytes(scan.bytes_total) }}</span>
              <span class="ui-footbar-stat">{{ tr("签名") }} <strong :key="scan.signatures_found" class="ui-bump">{{ scan.signatures_found.toLocaleString() }}</strong></span>
              <span class="ui-footbar-stat">{{ tr("发现") }} <strong :key="scan.artifacts_found" class="ui-bump">{{ scan.artifacts_found.toLocaleString() }}</strong> {{ tr("个文件") }}</span>
              <span v-if="checkedIds.size" class="ui-footbar-stat footer-checked">{{ tr("已勾选") }} <strong>{{ checkedIds.size.toLocaleString() }}</strong> {{ tr("项") }}</span>
              <span class="ui-footbar-spacer"></span>
              <button v-if="nextCursor" class="ui-btn link sm" :disabled="loadingMore || syncingResults" @click="loadMore"><Loader2 v-if="loadingMore" :size="11" class="spin" />{{ tr("加载更多") }}</button>
              <span class="footer-total mono">{{ hasFilters ? `${nextCursor || loadingMore || syncingResults ? tr("当前匹配") : tr("筛选")} ${visibleArtifacts.length.toLocaleString()} / ` : '' }}{{ tr("已加载") }} {{ artifacts.length.toLocaleString() }} / {{ resultTotal.toLocaleString() }} {{ tr("条结果") }}</span>
            </footer>
          </div>
          <UiTransition kind="dock"><CarveResultDetail v-if="selectedArtifact" :artifact="selectedArtifact" :offset="formatOffset(selectedArtifact.offset)" :preview-url="previewUrl" :preview-message="previewMessage" :loading="previewLoading" :recovering="recovering" @recover="recoverOne" @close="selectedId = null" /></UiTransition>
        </div>
      </template>
      <WorkspaceStartState v-else :icon="ScanSearch" :loading="initializing || loadingHistory"
        :title="initializationError || historyError ? tr('读取雕刻任务失败') : initializing || loadingHistory ? tr('正在读取雕刻任务…') : tr('还没有雕刻记录')"
        :description="initializationError || historyError || tr('选择文件类型，从检材中识别并恢复文件')"
        :action-label="initializationError || historyError ? tr('重试') : tr('新建扫描')" :action-icon="initializationError || historyError ? RefreshCw : Plus" @action="initializationError || historyError ? initializePanel() : configExpanded = true">
        <template #toolbar><button class="ui-btn ghost" :aria-pressed="historyVisible" @click="historyVisible = !historyVisible"><PanelLeft :size="14" />{{ tr("雕刻记录") }}</button></template>
      </WorkspaceStartState>
    </div>
    <CarveCreateDialog :open="configExpanded" :formats="formats" :format-groups="formatGroups" :selected-format-ids="selectedFormatIds"
      :initializing="initializing" :starting="starting" :can-start="canStart" :partitions="partitions" :partition-options="partitionOptions" :elements="configuration.elements"
      v-model:scope="scope" v-model:partition-id="partitionId" v-model:max-file-size="maxFileSize" v-model:max-results="maxResults"
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
import CarveResultTable from './CarveResultTable.vue';
import CarveCreateDialog from './CarveCreateDialog.vue';
import { AlertTriangle, RefreshCw, CheckCircle2, Download, FileSearch, Loader2, Pause, Play, Filter, Plus, ScanSearch, PanelLeft, CircleAlert, CircleSlash, X } from 'lucide-vue-next';
import type { ImageInfo } from '../../../lib/ipc/image';
import { formatBytes } from '../../../utils/format';
import { statusLabel, formatScanTime, formatOffset, categoryFilterOptions } from '../model/carveView';
import { useCarveConfiguration } from '../composables/useCarveConfiguration';
import { useCarveHistory } from '../composables/useCarveHistory';
import { useCarveResults } from '../composables/useCarveResults';
import { useCarvePreview } from '../composables/useCarvePreview';
import { useCarveRecovery } from '../composables/useCarveRecovery';
const props = defineProps<{ imageId: string; imageInfo: ImageInfo | null }>();
const initializing = ref(true);
const initializationError = ref('');
const history = useCarveHistory({ imageId: () => props.imageId, recovering: () => recovering.value,
  onSelected: () => results.loadFirstPage(), onReset: () => results.resetResults(), onStarted: () => { configExpanded.value = false; },
  onMoreResults: () => { if (scan.value && scan.value.artifacts_found > artifacts.value.length) void results.syncNewResults(); } });
const { historyVisible, scans, loadingHistory, historyError, deletingScanId, scan, starting, isActive, hasActiveScan,
  progressPercent, progressHeadline, scanStatusTone, selectScan, deleteScan, cancelScan, pauseScan, resumeScan } = history;
const results = useCarveResults({ imageId: () => props.imageId, scan: () => scan.value });
const { artifacts, nextCursor, resultTotal, loadingMore, syncingResults, resultsError, retryResults, query, queryDraft, categoryFilterDraft, categoryFilter,
  hasFilters, selectedId, checkedIds, visibleArtifacts, selectedArtifact, allVisibleSelected,
  applyFilters, clearFilters, toggleChecked, toggleAllVisible, loadMore } = results;
const recovery = useCarveRecovery({ imageId: () => props.imageId, scan: () => scan.value, checkedIds: () => checkedIds.value });
const { recovering, recoveryProgress, recoveryResult, recoveryPercent, recoverSelected, recoverAll, recoverOne, cancelRecovery } = recovery;
const { previewLoading, previewUrl, previewMessage } = useCarvePreview({ imageId: () => props.imageId, scan: () => scan.value, artifact: () => selectedArtifact.value });
const configuration = useCarveConfiguration({ imageId: () => props.imageId, imageInfo: () => props.imageInfo, starting: () => starting.value });
const { formats, formatGroups, selectedFormatIds, scope, partitionId, maxFileSize, maxResults, partitions, partitionOptions,
  configExpanded, toggleGroup, toggleFormat, selectAllFormats, clearFormats, closeConfiguration } = configuration;
const canStart = computed(() => !initializing.value && !hasActiveScan.value && !recovering.value && selectedFormatIds.value.size > 0 && (scope.value !== 'partition' || partitionId.value != null));
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
