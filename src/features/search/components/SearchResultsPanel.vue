<template>
  <section class="global-search-panel" :aria-label="tr('全局搜索')">
    <!-- 左：搜索任务树（检材 → 搜索任务节点）。任务即历史：运行中排最前，
         删除任务 = 删除整次搜索。 -->
    <UiTransition kind="horizontal"><SearchHistoryList v-show="historyVisible" :tasks="tasks" :loading-tasks="loadingTasks" :history-error="historyError"
      :selected-search-id="selectedSearchId" :deleting-task-id="deletingTaskId" :progress="runningProgress"
      @close="historyVisible = false" @select="selectTask" @delete="deleteTask" /></UiTransition>

    <!-- 右：任务命中表（服务端分页 / 过滤 / 去重）。 -->
    <div class="result-pane">
      <div v-if="historyError && selectedTask" class="ui-alert danger result-read-error" role="alert"><CircleAlert :size="14" /><span>{{ tr("读取搜索记录失败：{0}", { 0: historyError }) }}</span><button class="ui-btn sm" :disabled="loadingTasks" @click="history.refreshTasks()">{{ tr("重试") }}</button></div>
      <template v-if="selectedTask">
        <!-- 页面不设标题栏：顶部一行即工作工具条（筛选在左、操作在右），
             当前任务的模式、状态与命中数放在底部状态栏。 -->
        <form class="ui-toolbar result-toolbar result-filters" @submit.prevent="applyFilters">
          <button
            class="ui-btn ghost icon history-switch"
            type="button"
            :aria-pressed="historyVisible"
            :title="historyVisible ? tr('收起搜索记录') : tr('展开搜索记录')"
            :aria-label="tr('搜索记录')"
            @click="historyVisible = !historyVisible"
          ><PanelLeft :size="15" /></button>
          <label class="ui-search filter-input" :class="{ applied: filterKeyword }">
            <Filter :size="14" />
            <input
              v-model="filterKeywordDraft"
              type="text"
              :placeholder="tr('在结果中筛选内容、文件名或路径，回车应用')"
              :aria-label="tr('筛选搜索结果')"
              :disabled="selectedRunning || loadingHits"
              @keydown.enter.prevent="applyFilters"
            />
            <button v-if="filterKeyword || filterKeywordDraft" class="ui-icon-btn sm" type="button" :title="tr('清除筛选')" :aria-label="tr('清除筛选')" :disabled="selectedRunning || loadingHits" @click="clearResultFilter"><X :size="13" /></button>
          </label>
          <UiSelect
            v-model="distinctMode"
            class="distinct-select"
            :class="{ applied: distinctMode !== 'none' }"
            :options="distinctOptions"
            :disabled="selectedRunning || loadingHits"
            :title="tr('去重显示（不影响已存结果）')"
            :aria-label="tr('结果去重方式')"
            @change="applyFilters"
          />
          <div class="ui-toolbar-actions">
            <button
              v-if="selectedRunning"
              class="ui-btn danger-soft"
              type="button"
              :disabled="cancelling"
              @click="cancelSelectedTask"
            >
              <Loader2 v-if="cancelling" :size="13" class="spin" />
              <X v-else :size="13" />
              <span>{{ cancelling ? tr("正在取消") : tr("取消搜索") }}</span>
            </button>
            <button
              class="ui-btn"
              type="button"
              :disabled="exporting || hits.length === 0"
              @click="exportResults"
            >
              <Loader2 v-if="exporting" :size="13" class="spin" />
              <Download v-else :size="13" />
              <span>{{ tr("导出") }}</span>
            </button>
            <button class="ui-btn primary" type="button" @click="openCreateDialog"><Plus :size="14" />{{ tr("新建搜索") }}</button>
          </div>
        </form>

        <!-- 表格与状态栏在左，单击命中后详情从右侧展开。 -->
        <div class="result-body">
          <div class="result-main">
            <div v-if="hitsError" class="ui-alert danger result-read-error" role="alert">
              <CircleAlert :size="14" /><span>{{ tr("读取搜索结果失败：{0}", { 0: hitsError }) }}</span>
              <button class="ui-btn sm" @click="retryHits">{{ tr("重试") }}</button>
            </div>
            <SearchTaskFeedback :selected-task="selectedTask" :selected-running="selectedRunning" :progress-percent="progressPercent"
              :files-done-label="filesDoneLabel" :files-total-label="filesTotalLabel" :total-matches-label="totalMatchesLabel" />
            <SearchHitTable :hits="hits" :hits-total="hitsTotal" :loading-hits="loadingHits" :selected-running="selectedRunning"
              :filter-keyword="filterKeyword" :read-error="hitsError" :selected-hit-seq="selectedHitSeq" :locating="locating" :context-key="`${imageId}:${selectedSearchId}`"
              @select="selectedHitSeq = $event" @open="openHit" @load-through="ensureHitsThrough" @clear-filter="clearResultFilter" />

            <SearchResultsFooter :selected-task="selectedTask" :selected-patterns="selectedPatterns" :search-status-tone="searchStatusTone"
              :scanned-label="scannedLabel" :matched-file-count-label="matchedFileCountLabel" :total-matches-label="totalMatchesLabel"
              :loading-hits="loadingHits" :hits-total="hitsTotal" />
          </div>
          <UiTransition kind="dock"><SearchHitDetail v-if="selectedHit" :hit="selectedHit" :locating="locating" @open="openHit" @close="selectedHitSeq = null" /></UiTransition>
        </div>
      </template>

      <WorkspaceStartState v-else :icon="Search" :loading="loadingTasks"
        :title="historyError ? tr('读取搜索记录失败') : loadingTasks ? tr('正在读取搜索记录…') : tr('还没有搜索记录')"
        :description="historyError || tr('输入关键词或正则，查找检材中的文件内容')"
        :action-label="historyError ? tr('重试') : tr('新建搜索')" :action-icon="historyError ? RefreshCw : Plus" @action="historyError ? history.refreshTasks() : openCreateDialog()">
        <template #toolbar><button class="ui-btn ghost" :aria-pressed="historyVisible" @click="historyVisible = !historyVisible"><PanelLeft :size="14" />{{ tr("搜索记录") }}</button></template>
      </WorkspaceStartState>
    </div>

    <SearchCreateDialog :open="configuration.dialogVisible.value" :draft="configuration.dialog" :elements="configuration.elements"
      :starting="configuration.starting.value" :loading-partitions="configuration.loadingPartitions.value" :partition-options="configuration.partitionOptions.value"
      :scope-summary="configuration.scopeSummary.value" :advanced-summary="configuration.advancedSummary.value" :dialog-error="configuration.dialogError.value"
      @close="configuration.closeCreateDialog" @submit="configuration.submitCreateDialog" @toggle-code-page="configuration.toggleCodePage" />
  </section>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed } from 'vue';
import { CircleAlert, RefreshCw, PanelLeft, Filter, Download, Loader2, Plus, Search, X } from 'lucide-vue-next';
import UiSelect from '../../../components/ui/UiSelect.vue';
import UiTransition from '../../../components/ui/UiTransition.vue';
import WorkspaceStartState from '../../../components/layout/WorkspaceStartState.vue';
import SearchCreateDialog from './SearchCreateDialog.vue';
import SearchHistoryList from './SearchHistoryList.vue';
import SearchHitTable from './SearchHitTable.vue';
import SearchTaskFeedback from './SearchTaskFeedback.vue';
import SearchResultsFooter from './SearchResultsFooter.vue';
import SearchHitDetail from './SearchHitDetail.vue';
import { useSearchConfiguration } from '../composables/useSearchConfiguration';
import { useSearchHistory } from '../composables/useSearchHistory';
import { useSearchHits } from '../composables/useSearchHits';
import { useSearchExecution } from '../composables/useSearchExecution';
import { useSearchExport } from '../composables/useSearchExport';
import type { GlobalSearchHit } from '../../../lib/ipc/search';
import type { FileLocationTarget } from '../../../lib/models/fileLocation';
const props = defineProps<{ imageId: string; locating?: boolean }>();
const emit = defineEmits<{ close: []; 'open-file': [target: FileLocationTarget] }>();
const history = useSearchHistory({ imageId: () => props.imageId });
const { historyVisible, tasks, loadingTasks, historyError, selectedSearchId, deletingTaskId, runningProgress,
  selectedTask, selectedRunning, progressPercent, filesDoneLabel, filesTotalLabel, totalMatchesLabel,
  scannedLabel, selectedPatterns, matchedFileCountLabel, searchStatusTone, cancelling,
  selectTask, deleteTask, cancelSelectedTask } = history;
const query = useSearchHits({ imageId: () => props.imageId, searchId: () => selectedSearchId.value, running: () => selectedRunning.value });
const { hits, hitsTotal, loadingHits, hitsError, retryHits, selectedHitSeq, selectedHit, filterKeywordDraft, filterKeyword,
  distinctMode, clearResultFilter, applyFilters, ensureHitsThrough } = query;
const distinctOptions = computed(() => [
  { value: 'none' as const, get label() { return tr("全部记录"); } },
  { value: 'file' as const, get label() { return tr("按文件去重"); } },
  { value: 'sha256' as const, get label() { return tr("按哈希去重"); }, disabled: !selectedTask.value?.calc_hash },
]);
const { startSearch } = useSearchExecution({
  imageId: () => props.imageId, selectedSearchId: () => selectedSearchId.value,
  refreshTasks: history.refreshTasks, acceptProgress: history.acceptProgress,
  pollIncremental: query.pollIncremental, loadFirstPage: query.loadHitsFirstPage,
});
const configuration = useSearchConfiguration({ imageId: () => props.imageId, onSubmit: startSearch });
const { openCreateDialog } = configuration;
const { exporting, exportResults } = useSearchExport({
  imageId: () => props.imageId, selectedTask: () => selectedTask.value,
  keyword: () => filterKeyword.value, distinct: () => distinctMode.value,
});
function openHit(hit: GlobalSearchHit) {
  if (props.locating) return;
  emit('open-file', { imageId: props.imageId, nodeId: hit.node_id, offset: hit.file_offset, length: hit.match_len });
}
</script>
<style scoped src="../../../styles/analysis-workspace.css"></style>
