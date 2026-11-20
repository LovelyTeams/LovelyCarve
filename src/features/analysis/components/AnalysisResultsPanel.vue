<template>
  <section ref="panel" class="analysis-results" :aria-label="tr('分析结果')">
    <p class="sr-only" role="status">{{ statusText }}</p>
    <!-- 搜索、阅读方式与导出共用一条工具栏，完整路径与完成时间留在页脚。 -->
    <div v-if="selectedNode" class="ui-toolbar results-toolbar" :class="{ records: selectedNode.kind === 'result' }">
      <span v-if="selectedNode.kind === 'result'" class="result-scope" :title="[nodeLabel(selectedNode), selectedNode.targetLabel].filter(Boolean).join(' · ')"><span>{{ nodeLabel(selectedNode) }}</span><small class="ui-count">{{ recordsTotal.toLocaleString() }}</small></span>
      <form v-if="selectedNode.kind === 'result'" class="ui-search record-search" :class="{ applied: keyword }" @submit.prevent="applyKeyword(keywordInput)">
        <Search :size="14" />
        <input v-model="keywordInput" :aria-label="tr('搜索当前结果')" :placeholder="tr('搜索当前结果，回车确认…')" />
        <button v-if="keywordInput || keyword" class="ui-icon-btn sm" type="button" :aria-label="tr('清除搜索')" :title="tr('清除搜索')" @click="keywordInput = ''; applyKeyword('')"><X :size="13" /></button>
      </form>
      <span v-else class="toolbar-title"><ClipboardList :size="14" />{{ tr("运行信息") }}</span>
      <div v-if="selectedNode.kind === 'result'" class="ui-seg severity-filter" role="group" :aria-label="tr('重点告警筛选')">
        <button class="ui-seg-item" type="button" :aria-pressed="severityFilter === 'all'" @click="severityFilter = 'all'">{{ tr("全部") }}</button>
        <button class="ui-seg-item" type="button" :aria-pressed="severityFilter === 'alerts'" :class="{ 'has-alerts': alertCount > 0 }" :title="tr('仅看带告警级别的记录')" @click="severityFilter = 'alerts'">
          <ShieldAlert :size="13" aria-hidden="true" />
          <span>{{ tr("重点告警") }}</span>
          <span v-if="alertCount > 0" class="ui-badge sm warn">{{ alertCount }}</span>
        </button>
      </div>
      <div v-if="selectedNode.kind === 'result'" class="ui-seg reading-mode" role="group" :aria-label="tr('结果展示方式')">
        <button class="ui-seg-item" type="button" :aria-pressed="presentation === 'properties'" :title="tr('纵向阅读属性和值')" @click="setPresentation('properties')"><List :size="13" aria-hidden="true" />{{ tr("属性视图") }}</button>
        <button class="ui-seg-item" type="button" :aria-pressed="presentation === 'table'" :title="tr('横向对比多条记录')" @click="setPresentation('table')"><Table2 :size="13" aria-hidden="true" />{{ tr("表格视图") }}</button>
      </div>
      <div v-if="selectedNode.kind === 'result' && presentation === 'table'" ref="columnPickerRef" class="column-picker-container">
        <button
          class="ui-btn ghost column-picker-trigger"
          type="button"
          :aria-expanded="showColumnPicker"
          :title="tr('列显示')"
          @click="showColumnPicker = !showColumnPicker"
        >
          <SlidersHorizontal :size="14" aria-hidden="true" />
          <span>{{ tr("列显示") }}</span>
          <span v-if="customVisibleFieldIds" class="ui-badge sm info">{{ activeTableColumns.length }}/{{ columns.length }}</span>
        </button>
        <div v-if="showColumnPicker" class="column-picker-dropdown" role="menu">
          <div class="column-picker-header">
            <span class="column-picker-title">{{ tr("列显示") }} ({{ activeTableColumns.length }}/{{ columns.length }})</span>
            <div class="column-picker-quick">
              <button class="ui-btn link sm" type="button" @click="resetColumns">{{ tr("恢复默认列") }}</button>
              <button class="ui-btn link sm" type="button" @click="showAllColumns">{{ tr("显示全部列") }}</button>
            </div>
          </div>
          <div class="column-picker-list">
            <label v-for="col in columns" :key="col.id" class="column-picker-item">
              <input
                type="checkbox"
                :checked="isColumnVisible(col.id)"
                @change="toggleColumn(col.id)"
              />
              <span class="column-name" :title="col.id">{{ columnLabel(selectedNode, col) }}</span>
              <span v-if="col.hidden" class="column-hidden-tag mono">{{ tr("默认隐藏") }}</span>
            </label>
          </div>
        </div>
      </div>
      <div class="ui-toolbar-actions">
        <template v-if="selectedNode.kind === 'result'">
          <button v-if="runNode" class="ui-btn ghost" @click="selectNode(runNode)"><ClipboardList :size="14" />{{ tr("运行信息") }}</button>
          <button v-if="exporting && exportCanCancel" class="ui-btn ghost" @click="cancelExport">{{ tr("取消导出") }}</button>
          <button class="ui-btn" :disabled="exporting || treeLoading || !!treeError || recordsLoading || recordsTotal === 0" @click="exportCsv"><Loader2 v-if="exporting" :size="14" class="spin" /><Download v-else :size="14" />{{ exporting ? tr("导出中…") : tr("导出 CSV") }}</button>
        </template>
        <button class="ui-icon-btn" :disabled="treeLoading" :aria-label="tr('刷新分析结果')" :title="tr('刷新')" @click="reload"><Loader2 v-if="treeLoading" :size="16" class="spin" /><RefreshCw v-else :size="16" /></button>
      </div>
    </div>
    <div v-if="treeError" class="ui-empty state-box error" role="alert"><span class="ui-empty-icon"><CircleAlert :size="24" /></span><p class="ui-empty-desc">{{ tr("分析结果读取失败：{0}", { 0: treeError }) }}</p><div class="ui-empty-actions"><button class="ui-btn" @click="reload">{{ tr("重试") }}</button></div></div>
    <WorkspaceStartState v-else-if="!selectedNode" :icon="DatabaseZap" :loading="treeLoading"
      :title="treeLoading ? tr('正在读取分析结果…') : tree.length ? tr('选择一个结果项') : tr('当前检材还没有分析结果')"
      :description="tree.length ? tr('在左侧结果目录中选择结果项，查看解析出的记录。') : tr('选择分析功能，完成后结果会显示在这里。')"
      :action-label="tree.length ? undefined : tr('快速分析')" :action-icon="DatabaseZap" role="status" @action="emit('quickAnalysis')">
      <template #toolbar><button class="ui-btn ghost" :disabled="treeLoading" @click="reload"><RefreshCw :size="14" />{{ tr("刷新") }}</button></template>
    </WorkspaceStartState>
    <div v-else-if="selectedNode.kind === 'function'" class="results-body">
      <AnalysisRunOverview :node="selectedNode" :audit="audit" :loading="auditLoading" :error="auditError" :locating="locating" @open-file="target => emit('openFile', target)" @open-result="selectNode" @retry="retryAudit" />
    </div>
    <div v-else class="results-body">
      <div class="result-table">
        <div class="table-scroll" :class="{ sheet: presentation === 'properties' && records.length > 0 && !recordsLoading && !recordsError }" :aria-busy="recordsLoading">
          <div v-if="recordsLoading" class="ui-empty state-box" role="status"><span class="ui-empty-icon accent"><Loader2 :size="22" class="spin" /></span><p class="ui-empty-desc">{{ tr("正在读取记录…") }}</p></div>
          <div v-else-if="recordsError" class="ui-empty state-box error" role="alert"><span class="ui-empty-icon"><CircleAlert :size="22" /></span><p class="ui-empty-desc">{{ tr("记录读取失败：{0}", { 0: recordsError }) }}</p><div class="ui-empty-actions"><button class="ui-btn" @click="retryRecords">{{ tr("重试") }}</button></div></div>
          <div v-else-if="displayedRecords.length === 0" class="ui-empty state-box" role="status"><span class="ui-empty-icon"><Inbox :size="24" /></span><p class="ui-empty-title">{{ keyword ? tr("没有匹配的记录") : severityFilter === 'alerts' ? tr("当前页无重点告警记录") : tr("此结果项暂无记录") }}</p><p v-if="!keyword && severityFilter === 'all' && runNode" class="ui-empty-desc">{{ tr("运行信息里有这次运行的输入文件与告警。") }}</p><div class="ui-empty-actions"><button v-if="keyword" class="ui-btn" @click="keywordInput = ''; applyKeyword('')">{{ tr("清除搜索") }}</button><button v-else-if="severityFilter === 'alerts' && records.length > 0" class="ui-btn" @click="severityFilter = 'all'">{{ tr("查看全部记录") }}</button><button v-else-if="runNode" class="ui-btn" @click="selectNode(runNode)"><ClipboardList :size="14" />{{ tr("查看运行信息") }}</button></div></div>
          <AnalysisPropertyList v-else-if="presentation === 'properties'" :node="selectedNode" :records="displayedRecords" :columns="columns" :properties="propertyLayout" :selected-record-id="selectedRecordId" :locating="locating" @open-record="openRecord" @open-file="target => emit('openFile', target)" @record-menu="onPropertyContextMenu" />
          <table v-else class="data-table records-table" :style="{ minWidth: `${tableWidth}px` }" :aria-label="nodeLabel(selectedNode)">
            <colgroup>
              <col :style="{ width: `${INDEX_WIDTH}px` }" />
              <col v-if="showTitleColumn" :style="{ width: `${getColWidth('title', TITLE_WIDTH)}px` }" />
              <col v-for="column in tableColumns" :key="column.id" :style="columnStyle(column)" />
              <col :style="{ width: `${getColWidth('__source__', SOURCE_WIDTH)}px` }" />
            </colgroup>
            <thead><tr>
              <th class="row-number">{{ tr("序号") }}</th>
              <th v-if="showTitleColumn" :aria-sort="ariaSort('title')">
                <button class="sort-header" type="button" :title="tr('按名称排序')" @click="toggleSort('title')"><span>{{ tr("名称") }}</span><component :is="sortIcon('title')" :size="12" aria-hidden="true" /></button>
                <div class="col-resizer" :class="{ active: resizingCol === 'title' }" :title="tr('拖拽调整列宽')" @mousedown.stop.prevent="startColResize($event, 'title', TITLE_WIDTH)" />
              </th>
              <th v-for="column in tableColumns" :key="column.id" :class="{ numeric: NUMERIC_KINDS.has(column.kind) }" :aria-sort="ariaSort(column.id)">
                <button class="sort-header" type="button" :title="`${columnLabel(selectedNode, column)} · ${column.id}`" @click="toggleSort(column.id)"><span>{{ columnLabel(selectedNode, column) }}</span><component :is="sortIcon(column.id)" :size="12" aria-hidden="true" /></button>
                <div class="col-resizer" :class="{ active: resizingCol === column.id }" :title="tr('拖拽调整列宽')" @mousedown.stop.prevent="startColResize($event, column.id, columnWidth(column.kind) ?? FLEXIBLE_COLUMN_WIDTH)" />
              </th>
              <th>
                <span>{{ tr("源文件") }}</span>
                <div class="col-resizer" :class="{ active: resizingCol === '__source__' }" :title="tr('拖拽调整列宽')" @mousedown.stop.prevent="startColResize($event, '__source__', SOURCE_WIDTH)" />
              </th>
            </tr></thead>
            <tbody><tr v-for="(record, index) in displayedRecords" :key="record.recordId" :data-record-id="record.recordId" :class="{ active: selectedRecordId === record.recordId }" :aria-selected="selectedRecordId === record.recordId" tabindex="0" @click="openRecord(record.recordId)" @keydown.enter.prevent="openRecord(record.recordId)" @keydown.down.prevent="moveRow($event, index + 1)" @keydown.up.prevent="moveRow($event, index - 1)" @contextmenu="onRecordContextMenu($event, record)">
              <td class="row-number mono">{{ pageIndex * ANALYSIS_PAGE_SIZE + index + 1 }}</td>
              <td v-if="showTitleColumn" :title="record.title"><span class="title-cell"><span v-if="record.severity" class="ui-badge sm" :class="severityTone(record.severity)">{{ severityLabel(record.severity, tr) }}</span><span class="title-text">{{ record.title }}</span></span></td>
              <td v-for="(column, columnIndex) in tableColumns" :key="column.id" :data-column="column.id" :class="{ mono: MONO_KINDS.has(column.kind), numeric: NUMERIC_KINDS.has(column.kind), empty: isEmptyValue(record.values[column.id]) }" :title="cellText(record, column, Infinity)"><span v-if="columnIndex === 0 && record.severity" class="title-cell"><span class="ui-badge sm" :class="severityTone(record.severity)">{{ severityLabel(record.severity, tr) }}</span><span class="title-text">{{ cellText(record, column) }}</span></span><template v-else>{{ cellText(record, column) }}</template></td>
              <td><span class="table-sources"><button v-if="primaryPathTarget(record)" class="ui-icon-btn sm media-preview-btn" type="button" :title="tr('预览媒体文件')" :aria-label="tr('预览媒体文件')" @click.stop="emit('openFile', primaryPathTarget(record)!)" @keydown.enter.stop><Eye :size="13" aria-hidden="true" /></button><button class="ui-btn link sm table-source" :aria-label="tr('定位源文件')" :disabled="locating || !sources(record).length" :title="sources(record)[0] ? `${tr('定位源文件')} · ${sources(record)[0]?.label}` : tr('此记录未提供源文件位置')" @click.stop="locateRecord(record)" @keydown.enter.stop><LocateFixed :size="13" aria-hidden="true" /><span class="mono">{{ sources(record)[0]?.label || tr('无来源') }}</span></button><button v-if="sources(record).length > 1" class="ui-btn ghost sm source-more mono" type="button" :title="tr('查看全部来源（{0}）', { 0: sources(record).length })" :aria-label="tr('查看全部来源（{0}）', { 0: sources(record).length })" @click.stop="openRecord(record.recordId)" @keydown.enter.stop>+{{ (sources(record).length - 1).toLocaleString() }}</button></span></td>
            </tr></tbody>
          </table>
        </div>
        <footer class="ui-footbar table-footer">
          <span class="footer-crumb" :title="crumbLabel"><Table2 :size="12" /><span>{{ crumbLabel }}</span></span>
          <span class="ui-footbar-stat"><strong :key="severityFilter === 'alerts' ? alertCount : recordsTotal" class="ui-bump">{{ (severityFilter === 'alerts' ? alertCount : recordsTotal).toLocaleString() }}</strong> {{ keyword ? tr("条匹配记录") : severityFilter === 'alerts' ? tr("条重点告警") : tr("条记录") }}</span>
          <span v-if="completedLabel" class="ui-footbar-stat footer-completed">{{ tr("完成于（本地）") }} <strong>{{ completedLabel }}</strong></span>
          <span class="ui-footbar-spacer"></span>
          <div class="pager">
            <button v-if="pageCount > 2" class="ui-icon-btn sm" :disabled="pageIndex === 0 || recordsLoading" :aria-label="tr('第一页')" :title="tr('第一页')" @click="goToPage(0)"><ChevronsLeft :size="16" /></button>
            <button class="ui-icon-btn sm" :disabled="pageIndex === 0 || recordsLoading" :aria-label="tr('上一页')" :title="tr('上一页')" @click="goToPage(pageIndex - 1)"><ChevronLeft :size="16" /></button>
            <span class="mono">{{ pageIndex + 1 }} / {{ pageCount }}</span>
            <button class="ui-icon-btn sm" :disabled="pageIndex >= pageCount - 1 || recordsLoading" :aria-label="tr('下一页')" :title="tr('下一页')" @click="goToPage(pageIndex + 1)"><ChevronRight :size="16" /></button>
            <button v-if="pageCount > 2" class="ui-icon-btn sm" :disabled="pageIndex >= pageCount - 1 || recordsLoading" :aria-label="tr('最后一页')" :title="tr('最后一页')" @click="goToPage(pageCount - 1)"><ChevronsRight :size="16" /></button>
          </div>
        </footer>
      </div>
      <UiTransition kind="dock">
        <DetailDock v-if="selectedRecordId" :label="tr('记录详情')" :close-label="tr('关闭详情')" :title="detail?.title || tr('记录详情')" :content-key="selectedRecordId" @close="closeDetail">
          <template #actions>
            <button class="ui-icon-btn" type="button" :disabled="detailIndex <= 0" :title="tr('上一条记录')" :aria-label="tr('上一条记录')" @click="openSibling(-1)"><ChevronUp :size="16" /></button>
            <button class="ui-icon-btn" type="button" :disabled="detailIndex < 0 || detailIndex >= displayedRecords.length - 1" :title="tr('下一条记录')" :aria-label="tr('下一条记录')" @click="openSibling(1)"><ChevronDown :size="16" /></button>
          </template>
          <div v-if="detailLoading" class="ui-empty state-box" role="status"><span class="ui-empty-icon accent"><Loader2 :size="20" class="spin" /></span><p class="ui-empty-desc">{{ tr("加载中…") }}</p></div>
          <div v-else-if="detailError" class="ui-empty state-box error" role="alert"><p class="ui-empty-desc">{{ tr("详情读取失败：{0}", { 0: detailError }) }}</p><div class="ui-empty-actions"><button class="ui-btn" @click="openRecord(selectedRecordId)">{{ tr("重试") }}</button></div></div>
          <FileDetailLayout v-else-if="detail" :title="detail.title">
            <template #icon><Table2 :size="20" /></template>
            <template #badges><span class="ui-badge">{{ nodeLabel(selectedNode) }}</span><span v-if="detail.severity" class="ui-badge" :class="severityTone(detail.severity)">{{ severityLabel(detail.severity, tr) }}</span><span v-if="detail.legacy" class="ui-badge">{{ tr("旧版") }}</span></template>
            <FileDetailSection v-if="activePreviewTarget || previewLoading || previewError" :title="tr('检材原图/媒体文件预览')" wide>
              <div class="dock-media-card">
                <div v-if="previewLoading" class="dock-media-state">
                  <Loader2 :size="18" class="spin" />
                  <span>{{ tr("正在读取检材原图…") }}</span>
                </div>
                <div v-else-if="previewError" class="dock-media-state error">
                  <CircleAlert :size="16" />
                  <span>{{ previewError }}</span>
                </div>
                <template v-else-if="previewUrl">
                  <div
                    v-if="previewKind === 'image'"
                    class="dock-media-stage"
                    :title="tr('点击在右侧查看全图')"
                    @click="activePreviewTarget && emit('openFile', activePreviewTarget)"
                  >
                    <img :src="previewUrl" :alt="previewFilename" class="dock-media-img" />
                    <div class="dock-media-badge">{{ tr("原图预览") }}</div>
                  </div>
                  <div v-else-if="previewKind === 'video'" class="dock-media-stage">
                    <video :src="previewUrl" controls class="dock-media-video" playsinline />
                  </div>
                  <div v-else-if="previewKind === 'audio'" class="dock-media-stage">
                    <audio :src="previewUrl" controls class="dock-media-audio" />
                  </div>
                  <div class="dock-media-footer">
                    <span class="dock-media-path mono" :title="activePreviewTarget?.path">{{ previewFilename }}</span>
                    <button
                      v-if="activePreviewTarget"
                      class="ui-btn ghost sm"
                      type="button"
                      :title="tr('打开该文件（图片等将直接预览）')"
                      @click="emit('openFile', activePreviewTarget)"
                    >
                      <Eye :size="13" />
                      <span>{{ tr("完整预览") }}</span>
                    </button>
                  </div>
                </template>
              </div>
            </FileDetailSection>
            <FileDetailSection :title="tr('记录内容')" wide>
              <FileDetailFields :fields="detailFields" />
            </FileDetailSection>
            <FileDetailSection v-if="pathTargets.length" :title="tr('记录内文件')" wide>
              <div class="source-files">
                <div v-for="item in pathTargets" :key="item.target.path" class="source-file">
                  <button class="ui-btn link sm" :disabled="locating" :title="tr('打开该文件（图片等将直接预览）')" @click="emit('openFile', item.target)"><LocateFixed :size="14" /><span class="mono">{{ item.target.path }}</span></button>
                </div>
              </div>
            </FileDetailSection>
            <FileDetailSection :title="tr('源文件')" wide>
              <div v-if="sources(detail).length" class="source-files">
                <div v-for="source in sources(detail)" :key="`${source.label}:${source.note}`" class="source-file">
                  <button class="ui-btn link sm" :disabled="locating" :title="source.label" @click="emit('openFile', source.target)"><LocateFixed :size="14" /><span>{{ source.label }}</span></button>
                  <span v-if="source.note" class="source-note">{{ source.note }}</span>
                </div>
              </div>
              <p v-else class="source-missing">{{ tr("此记录未提供源文件位置。") }}</p>
            </FileDetailSection>
            <FileDetailSection :key="selectedRecordId" :title="tr('技术信息')" collapsible wide>
              <FileDetailFields :fields="[
                { label: tr('记录 ID'), value: detail.recordId, mono: true, copyable: true },
                { label: tr('记录类型'), value: detail.recordType, mono: true, copyable: true },
                { label: tr('运行 ID'), value: detail.runId, mono: true, copyable: true },
              ]" />
            </FileDetailSection>
          </FileDetailLayout>
        </DetailDock>
      </UiTransition>
    </div>
    <UiContextMenu
      v-model:open="recordMenu.open"
      :x="recordMenu.x"
      :y="recordMenu.y"
      :entries="recordMenuEntries"
      :label="tr('记录操作')"
      :heading="recordMenu.target?.record.title"
      :subheading="selectedNode ? nodeLabel(selectedNode) : undefined"
    />
  </section>
</template>

<script setup lang="ts">
import { tr, useLocale } from '../../../composables/useLocale';

import UiTransition from '../../../components/ui/UiTransition.vue';
import WorkspaceStartState from '../../../components/layout/WorkspaceStartState.vue';
import DetailDock from '../../../components/layout/DetailDock.vue';
import FileDetailLayout from '../../../components/layout/FileDetailLayout.vue';
import FileDetailSection from '../../../components/layout/FileDetailSection.vue';
import FileDetailFields, { type FileDetailField } from '../../../components/layout/FileDetailFields.vue';
import AnalysisRunOverview from './AnalysisRunOverview.vue';
import AnalysisPropertyList from './AnalysisPropertyList.vue';
import { computed, nextTick, onActivated, onBeforeUnmount, onDeactivated, onMounted, ref, watch } from 'vue';
import { ArrowDown, ArrowUp, ArrowUpDown, ChevronDown, ChevronLeft, ChevronRight, ChevronsLeft, ChevronsRight, ChevronUp, CircleAlert, ClipboardList, Copy, DatabaseZap, Download, Eye, Inbox, List, Loader2, LocateFixed, PanelRightOpen, RefreshCw, Search, ShieldAlert, SlidersHorizontal, Table2, X } from 'lucide-vue-next';
import UiContextMenu, { type UiContextMenuEntry } from '../../../components/ui/UiContextMenu.vue';
import { copyText } from '../../../composables/clipboard';
import { useContextMenu } from '../../../composables/useContextMenu';
import { tsvEscape } from '../../../utils/exportFormat';
import { ANALYSIS_PAGE_SIZE, useAnalysisResults } from '../composables/useAnalysisResults';
import { useAnalysisLabels } from '../composables/useAnalysisLabels';
import { useAnalysisPresentation } from '../composables/useAnalysisPresentation';
import { saveTextWithDialog } from '../../../composables/export';
import { notifyError, notifyInfo, pushPersistentToast } from '../../../composables/useToast';
import { recordsToCsv, sanitizeExportFilename } from '../../../utils/exportFormat';
import type { AnalysisRecord, AnalysisResultNode, RecordColumn, RecordFieldKind } from '../../../lib/ipc/analysisResults';
import { columnWidth, FLEXIBLE_COLUMN_WIDTH } from '../model/analysisPresentation';
import { analysisSources } from '../model/analysisSource';
import { isEmptyValue, recordCsvValue, recordValueText, severityLabel, severityTone, truncateText } from '../model/analysisValue';
import type { FileLocationTarget } from '../../../lib/models/fileLocation';
import { fsSourceLocate } from '../../../lib/ipc/fs/query';
import { fsRead } from '../../../lib/ipc/fs/read';
import type { ImageId, NodeId } from '../../../lib/ipc/ids';
const props = defineProps<{ imageId: ImageId; locating?: boolean }>();
const emit = defineEmits<{ quickAnalysis: []; openFile: [target: FileLocationTarget] }>();
const sources = (record: AnalysisRecord) => analysisSources(record, selectedNode.value?.imageId || props.imageId);
function locateRecord(record: AnalysisRecord) {
  const source = sources(record)[0];
  if (source && !props.locating) emit('openFile', source.target);
}
function primaryPathTarget(record: AnalysisRecord): FileLocationTarget | null {
  const first = sources(record)[0];
  const rootId = first?.target.rootId;
  if (!rootId) return null;
  for (const column of columns.value) {
    if (column.kind !== 'path') continue;
    const value = record.values[column.id];
    if (typeof value === 'string' && value.trim()) {
      return { imageId: record.imageId || props.imageId, rootId, path: value.trim() };
    }
  }
  return null;
}
const { tree, treeLoading, treeError, dirty, selectedNode, records, columns, properties, recordsTotal,
  pageIndex, pageCount, keyword, sort, recordsLoading, recordsError, detail, detailColumns, detailLoading,
  detailError, selectedRecordId, audit, auditLoading, auditError, closeDetail: clearDetail, retryRecords, retryAudit,
  setImageId, ensureFresh, selectNode, goToPage, applyKeyword, toggleSort, openRecord: loadRecord, exportRecords, reload } = useAnalysisResults();
const { nodeLabel, columnLabel, choiceLabel } = useAnalysisLabels();
const { propertyLayout, presentation, setPresentation } = useAnalysisPresentation(selectedNode, columns, properties);
const panel = ref<HTMLElement | null>(null);
let detailReturnFocus: HTMLElement | null = null;
function openRecord(recordId: string) {
  const focused = document.activeElement;
  if (focused instanceof HTMLElement && !focused.closest('.detail-dock')) detailReturnFocus = focused;
  return loadRecord(recordId);
}
/** Closing returns focus to the control the record was opened from, or to
 *  the record shown last when the dock stepped to another one. */
function closeDetail() {
  const recordId = selectedRecordId.value;
  const origin = detailReturnFocus;
  detailReturnFocus = null;
  clearDetail();
  void nextTick(() => {
    const holder = (element: Element | null | undefined) => element?.closest<HTMLElement>('[data-record-id]');
    if (origin?.isConnected && holder(origin)?.dataset.recordId === recordId) { origin.focus(); return; }
    const row = [...(panel.value?.querySelectorAll<HTMLElement>('[data-record-id]') ?? [])].find(element => element.dataset.recordId === recordId);
    const target = row ? (row.tabIndex >= 0 ? row : row.querySelector<HTMLElement>('button')) : origin;
    if (target?.isConnected) target.focus();
  });
}

const severityFilter = ref<'all' | 'alerts'>('all');
const alertCount = computed(() => records.value.filter(r => !!r.severity).length);
const displayedRecords = computed(() => severityFilter.value === 'alerts' ? records.value.filter(r => !!r.severity) : records.value);
watch([selectedNode, pageIndex], () => { severityFilter.value = 'all'; });

const showColumnPicker = ref(false);
const columnPickerRef = ref<HTMLElement | null>(null);
const customVisibleFieldIds = ref<Set<string> | null>(null);

watch(selectedNode, () => {
  customVisibleFieldIds.value = null;
  showColumnPicker.value = false;
});

const activeTableColumns = computed(() => {
  if (customVisibleFieldIds.value) return columns.value.filter(c => customVisibleFieldIds.value!.has(c.id));
  return columns.value.filter(c => !c.hidden);
});
const tableColumns = activeTableColumns;

function isColumnVisible(fieldId: string): boolean {
  if (customVisibleFieldIds.value) return customVisibleFieldIds.value.has(fieldId);
  const col = columns.value.find(c => c.id === fieldId);
  return col ? !col.hidden : true;
}

function toggleColumn(fieldId: string) {
  const currentSet = new Set(customVisibleFieldIds.value ?? columns.value.filter(c => !c.hidden).map(c => c.id));
  if (currentSet.has(fieldId)) {
    if (currentSet.size > 1) {
      currentSet.delete(fieldId);
    }
  } else {
    currentSet.add(fieldId);
  }
  customVisibleFieldIds.value = currentSet;
}

function resetColumns() {
  customVisibleFieldIds.value = null;
}

function showAllColumns() {
  customVisibleFieldIds.value = new Set(columns.value.map(c => c.id));
}

function onDocumentClick(e: MouseEvent) {
  if (showColumnPicker.value && columnPickerRef.value && !columnPickerRef.value.contains(e.target as Node)) {
    showColumnPicker.value = false;
  }
}

/** Position of the open record on this page (the dock steps through it). */
const detailIndex = computed(() => displayedRecords.value.findIndex(record => record.recordId === selectedRecordId.value));
function openSibling(step: number) {
  const record = displayedRecords.value[detailIndex.value + step];
  if (record) void loadRecord(record.recordId);
}
/** Arrow keys walk the table rows; with the dock open the record follows. */
function moveRow(event: KeyboardEvent, index: number) {
  const record = displayedRecords.value[index];
  const row = (event.currentTarget as HTMLElement).parentElement?.children[index];
  if (!record || !(row instanceof HTMLElement)) return;
  row.focus();
  if (selectedRecordId.value) void openRecord(record.recordId);
}
const panelActive = ref(false);
function activatePanel() { panelActive.value = true; setImageId(props.imageId); ensureFresh(); }
onMounted(() => {
  activatePanel();
  document.addEventListener('pointerdown', onDocumentClick);
});
onActivated(activatePanel);
onDeactivated(() => { panelActive.value = false; detailReturnFocus = null; });
watch(() => props.imageId, () => { if (panelActive.value) activatePanel(); });
watch(dirty, value => { if (value && panelActive.value && !treeLoading.value) reload(); });
const keywordInput = ref(keyword.value);
const exporting = ref(false);
const exportCanCancel = ref(false);
let exportController: AbortController | null = null;
let stopColResize: (() => void) | null = null;
function cancelExport() {
  exportCanCancel.value = false;
  exportController?.abort();
}
onBeforeUnmount(() => {
  exportController?.abort();
  stopColResize?.();
  document.removeEventListener('pointerdown', onDocumentClick);
  if (previewUrl.value && typeof URL.revokeObjectURL === 'function') {
    URL.revokeObjectURL(previewUrl.value);
    previewUrl.value = null;
  }
});

watch(keyword, value => { keywordInput.value = value; });

/** Values set in the monospace face: offsets, hashes, addresses, paths and counts. */
const MONO_KINDS = new Set<RecordFieldKind>(['int', 'float', 'size', 'duration', 'hash', 'ip', 'path', 'timestamp']);
const NUMERIC_KINDS = new Set<RecordFieldKind>(['int', 'float', 'size', 'duration']);
// Schema fields already describe the record; keep a title fallback for results without visible fields.
const showTitleColumn = computed(() => tableColumns.value.length === 0);
/** Fixed widths of the row number, the title fallback and the sticky source column. */
const INDEX_WIDTH = 56;
const TITLE_WIDTH = 240;
const SOURCE_WIDTH = 220;
const userColWidths = ref<Record<string, number>>({});
const resizingCol = ref<string | null>(null);

function getColWidth(colId: string, defaultWidth?: number | null): number {
  if (userColWidths.value[colId]) return userColWidths.value[colId];
  return defaultWidth ?? FLEXIBLE_COLUMN_WIDTH;
}

/** Fixed columns keep their kind's width and free text shares the rest; below
 *  this width the table scrolls sideways instead of squeezing a column. */
const tableWidth = computed(() => {
  const titleW = showTitleColumn.value ? getColWidth('title', TITLE_WIDTH) : 0;
  const sourceW = getColWidth('__source__', SOURCE_WIDTH);
  const colsW = tableColumns.value.reduce((sum, col) => sum + getColWidth(col.id, columnWidth(col.kind)), 0);
  return INDEX_WIDTH + titleW + sourceW + colsW;
});

function columnStyle(column: RecordColumn) {
  if (userColWidths.value[column.id]) {
    return { width: `${userColWidths.value[column.id]}px` };
  }
  const width = columnWidth(column.kind);
  return width === null ? undefined : { width: `${width}px` };
}

function startColResize(e: MouseEvent, colId: string, initialWidth: number) {
  stopColResize?.();
  resizingCol.value = colId;
  const startX = e.clientX;
  const startW = userColWidths.value[colId] || initialWidth;

  const onMouseMove = (moveEvent: MouseEvent) => {
    const delta = moveEvent.clientX - startX;
    const newW = Math.max(60, startW + delta);
    userColWidths.value = { ...userColWidths.value, [colId]: newW };
  };

  const cleanup = () => {
    resizingCol.value = null;
    window.removeEventListener('mousemove', onMouseMove);
    window.removeEventListener('mouseup', cleanup);
    if (stopColResize === cleanup) stopColResize = null;
  };

  stopColResize = cleanup;
  window.addEventListener('mousemove', onMouseMove);
  window.addEventListener('mouseup', cleanup);
}

/** The run the selected record type belongs to (its audit). */
const runNode = computed(() => {
  const node = selectedNode.value;
  if (!node) return null;
  const find = (nodes: AnalysisResultNode[]): AnalysisResultNode | null => {
    for (const item of nodes) {
      if (item.kind === 'function' && item.children.some(child => child.nodeId === node.nodeId)) return item;
      const found = find(item.children);
      if (found) return found;
    }
    return null;
  };
  return find(tree.value);
});

const statusText = computed(() => {
  const node = selectedNode.value;
  if (treeError.value || recordsError.value || !node) return '';
  if (node.kind === 'function') return auditLoading.value ? tr("正在读取运行信息…") : nodeLabel(node);
  if (recordsLoading.value) return tr("正在读取记录…");
  return tr("{0}，共 {1} 条{2}，第 {3} 页", { 0: nodeLabel(node), 1: recordsTotal.value.toLocaleString(), 2: keyword.value ? tr("匹配记录") : tr("记录"), 3: pageIndex.value + 1 });
});

function ariaSort(column: string): 'ascending' | 'descending' | 'none' {
  if (sort.value?.column !== column) return 'none';
  return sort.value.descending ? 'descending' : 'ascending';
}

function sortIcon(column: string) {
  if (sort.value?.column !== column) return ArrowUpDown;
  return sort.value.descending ? ArrowDown : ArrowUp;
}

function valueText(value: unknown, column: RecordColumn, detailed = false): string {
  return recordValueText(value, column.kind, { choice: id => choiceLabel(selectedNode.value, column, id), detailed, tr });
}

function cellText(record: AnalysisRecord, column: RecordColumn, maxLength = 120): string {
  return truncateText(valueText(record.values[column.id], column), maxLength);
}

// ── 记录右键菜单 ── 表格与属性视图共用：查看、定位，复制右键处的值或整条记录。
const { menu: recordMenu, openFromEvent: openRecordMenu } = useContextMenu<{ record: AnalysisRecord; columnId: string | null }>();
function onRecordContextMenu(event: MouseEvent, record: AnalysisRecord) {
  const columnId = (event.target as Element | null)?.closest<HTMLElement>('[data-column]')?.dataset.column ?? null;
  openRecordMenu(event, { record, columnId });
}
function onPropertyContextMenu(event: MouseEvent, recordId: string, columnId: string | null) {
  const record = records.value.find(candidate => candidate.recordId === recordId);
  if (record) openRecordMenu(event, { record, columnId });
}
/** 可见列的表头与值，制表符分隔，可直接粘进表格；值经 tsvEscape 中和公式与控制字符。 */
function recordTsv(record: AnalysisRecord): string {
  const node = selectedNode.value;
  const visible = tableColumns.value;
  const header = visible.length ? visible.map(column => columnLabel(node, column)) : [tr("名称")];
  const values = visible.length ? visible.map(column => valueText(record.values[column.id], column, true)) : [record.title];
  return [header, values].map(cells => cells.map(tsvEscape).join('\t')).join('\n');
}
const recordMenuEntries = computed<UiContextMenuEntry[]>(() => {
  const target = recordMenu.target;
  const node = selectedNode.value;
  if (!target || !node) return [];
  const { record } = target;
  const column = columns.value.find(candidate => candidate.id === target.columnId) ?? null;
  const recordSources = sources(record);
  const entries: UiContextMenuEntry[] = [
    { id: 'detail', label: tr("查看详情"), icon: PanelRightOpen, hint: 'Enter', action: () => void openRecord(record.recordId) },
    {
      id: 'locate', label: tr("定位源文件"), icon: LocateFixed, disabled: props.locating || !recordSources.length,
      title: recordSources[0]?.label ?? tr("此记录未提供源文件位置"), action: () => locateRecord(record),
    },
    { kind: 'separator' },
  ];
  if (column) {
    const label = columnLabel(node, column);
    entries.push({ id: 'copy-value', label: tr("复制「{0}」", { 0: label }), icon: Copy, action: () => void copyText(valueText(record.values[column.id], column, true), label) });
  }
  entries.push({ id: 'copy-record', label: tr("复制整条记录"), icon: ClipboardList, action: () => void copyText(recordTsv(record), tr("{0} 条记录", { 0: 1 })) });
  return entries;
});

const selectedCrumb = computed<string[]>(() => {
  const crumb: string[] = [];
  const walk = (nodes: AnalysisResultNode[]): boolean => {
    for (const node of nodes) {
      crumb.push(node.kind === 'function' && node.targetLabel ? `${nodeLabel(node)} (${node.targetLabel})` : nodeLabel(node));
      if (node.nodeId === selectedNode.value?.nodeId) return true;
      if (walk(node.children)) return true;
      crumb.pop();
    }
    return false;
  };
  if (!selectedNode.value) return [];
  walk(tree.value);
  return crumb;
});

/** 状态栏的结果路径：检材 › 分类 › 分析器 › 记录类型。 */
const crumbLabel = computed(() => selectedCrumb.value.join(' › ') || (selectedNode.value ? nodeLabel(selectedNode.value) : ''));
const { locale } = useLocale();
const completedLabel = computed(() => {
  const completedAt = runNode.value?.completedAt;
  if (!completedAt) return '';
  const completed = new Date(completedAt);
  return Number.isNaN(completed.getTime()) ? '' : completed.toLocaleString(locale.value, { hour12: false });
});

/** 记录字段里指向检材文件的路径（kind=path）：一键跳到文件预览（图片可直接渲染）。 */
const pathTargets = computed(() => {
  const record = detail.value;
  if (!record) return [];
  const first = sources(record)[0];
  const rootId = first?.target.rootId;
  if (!rootId) return [];
  const out: { label: string; target: FileLocationTarget }[] = [];
  const seen = new Set<string>();
  for (const column of detailColumns.value) {
    if (column.kind !== 'path') continue;
    const value = record.values[column.id];
    if (typeof value !== 'string' || !value.trim()) continue;
    const path = value.trim();
    if (seen.has(path)) continue;
    seen.add(path);
    out.push({
      label: `${columnLabel(selectedNode.value, column)}：${path}`,
      target: { imageId: record.imageId || props.imageId, rootId, path },
    });
  }
  return out;
});

const PREVIEW_IMAGE_EXTS = new Set(['heic', 'heif', 'jpg', 'jpeg', 'png', 'gif', 'webp', 'bmp', 'svg', 'tiff', 'tif', 'ico']);
const PREVIEW_VIDEO_EXTS = new Set(['mov', 'mp4', 'm4v', 'webm', 'mkv', 'avi']);
const PREVIEW_AUDIO_EXTS = new Set(['mp3', 'm4a', 'wav', 'aac', 'flac', 'ogg']);

function getMediaKind(path: string): 'image' | 'video' | 'audio' | null {
  const ext = path.split('.').pop()?.toLowerCase() ?? '';
  if (PREVIEW_IMAGE_EXTS.has(ext)) return 'image';
  if (PREVIEW_VIDEO_EXTS.has(ext)) return 'video';
  if (PREVIEW_AUDIO_EXTS.has(ext)) return 'audio';
  return null;
}

function getMediaMime(path: string, kind: 'image' | 'video' | 'audio'): string {
  const ext = path.split('.').pop()?.toLowerCase() ?? '';
  if (kind === 'image') {
    if (ext === 'heic' || ext === 'heif') return 'image/heic';
    if (ext === 'jpg' || ext === 'jpeg') return 'image/jpeg';
    if (ext === 'png') return 'image/png';
    if (ext === 'gif') return 'image/gif';
    if (ext === 'webp') return 'image/webp';
    if (ext === 'svg') return 'image/svg+xml';
    if (ext === 'bmp') return 'image/bmp';
    return 'image/jpeg';
  }
  if (kind === 'video') {
    if (ext === 'mov') return 'video/quicktime';
    if (ext === 'mp4') return 'video/mp4';
    return 'video/webm';
  }
  if (ext === 'mp3') return 'audio/mpeg';
  if (ext === 'm4a') return 'audio/mp4';
  if (ext === 'wav') return 'audio/wav';
  return 'audio/aac';
}

const previewLoading = ref(false);
const previewError = ref<string | null>(null);
const previewUrl = ref<string | null>(null);
const previewKind = ref<'image' | 'video' | 'audio' | null>(null);
const activePreviewTarget = ref<FileLocationTarget | null>(null);
const previewFilename = computed(() => {
  const target = activePreviewTarget.value;
  if (!target) return '';
  const p = target.path || '';
  return p ? (p.split(/[\/\\]/).pop() || p) : '';
});
let previewGeneration = 0;

function findMediaCandidatePaths(record: AnalysisRecord): { directTarget?: FileLocationTarget; sniffCandidates?: { rootId: string; filename: string; paths: string[] } } | null {
  const first = sources(record)[0];
  const rootId = first?.target.rootId;
  if (!rootId) return null;
  const imageId = record.imageId || props.imageId;

  for (const col of detailColumns.value) {
    if (col.kind !== 'path') continue;
    const val = record.values[col.id];
    if (typeof val === 'string' && val.trim()) {
      const p = val.trim();
      if (getMediaKind(p)) {
        return { directTarget: { imageId, rootId, path: p } };
      }
    }
  }

  for (const src of sources(record)) {
    const p = src.target.path;
    if (typeof p === 'string' && p.trim() && getMediaKind(p)) {
      return { directTarget: src.target };
    }
  }

  const nameCandidates = [
    record.values.filename,
    record.values.original_filename,
    record.values.file,
    record.values.asset_name,
    record.title,
  ].filter((v): v is string => typeof v === 'string' && !!v.trim());

  for (const rawName of nameCandidates) {
    const clean = rawName.trim();
    const kind = getMediaKind(clean);
    if (kind) {
      const filename = clean.split(/[\/\\]/).pop() || clean;
      const paths: string[] = [];
      if (clean.includes('/') || clean.includes('\\')) {
        paths.push(clean.replace(/\\/g, '/'));
      }
      paths.push(
        `var/mobile/Media/DCIM/100APPLE/${filename}`,
        `var/mobile/Media/DCIM/101APPLE/${filename}`,
        `var/mobile/Media/DCIM/102APPLE/${filename}`,
        `var/mobile/Media/Downloads/${filename}`,
        `var/mobile/Media/PhotoData/CPLAssets/${filename}`,
        `var/mobile/Media/${filename}`,
        `Media/DCIM/100APPLE/${filename}`,
        `Media/DCIM/101APPLE/${filename}`,
        `Media/${filename}`,
        `HomeDomain/Media/${filename}`,
        `CameraRollDomain/Media/${filename}`,
        filename,
      );
      return { sniffCandidates: { rootId, filename, paths } };
    }
  }

  return null;
}

async function loadRecordMedia(record: AnalysisRecord | null) {
  const currentGen = ++previewGeneration;
  if (previewUrl.value && typeof URL.revokeObjectURL === 'function') {
    URL.revokeObjectURL(previewUrl.value);
    previewUrl.value = null;
  } else {
    previewUrl.value = null;
  }
  previewKind.value = null;
  previewError.value = null;
  activePreviewTarget.value = null;

  if (!record) return;
  const candidateInfo = findMediaCandidatePaths(record);
  if (!candidateInfo) return;

  previewLoading.value = true;
  const imageId = record.imageId || props.imageId;

  try {
    let resolvedTarget: FileLocationTarget | null = null;
    let resolvedNodeId: NodeId | null = null;

    if (candidateInfo.directTarget) {
      const target = candidateInfo.directTarget;
      if (target.rootId && target.path) {
        try {
          const loc = await fsSourceLocate({ imageId: target.imageId, rootId: target.rootId, path: target.path });
          if (loc?.node_id) {
            resolvedTarget = target;
            resolvedNodeId = loc.file_id || loc.node_id;
          }
        } catch {
          // locate failed
        }
      }
    } else if (candidateInfo.sniffCandidates) {
      const { rootId, paths } = candidateInfo.sniffCandidates;
      for (const path of paths) {
        if (currentGen !== previewGeneration) return;
        try {
          const loc = await fsSourceLocate({ imageId, rootId, path });
          if (loc?.node_id) {
            resolvedTarget = { imageId, rootId, path };
            resolvedNodeId = loc.file_id || loc.node_id;
            break;
          }
        } catch {
          // try next candidate
        }
      }
    }

    if (currentGen !== previewGeneration) return;

    if (!resolvedTarget || !resolvedNodeId) {
      if (candidateInfo.sniffCandidates) {
        previewError.value = tr("检材中未找到对应的原图文件");
      }
      return;
    }

    activePreviewTarget.value = resolvedTarget;
    const targetPath = resolvedTarget.path || '';
    const kind = getMediaKind(targetPath);
    previewKind.value = kind;

    if (kind === 'image') {
      const buf = await fsRead({ imageId: resolvedTarget.imageId, nodeId: resolvedNodeId });
      if (currentGen !== previewGeneration) return;
      if (buf && buf.byteLength > 0 && typeof URL.createObjectURL === 'function') {
        const mime = getMediaMime(targetPath, 'image');
        const blob = new Blob([new Uint8Array(buf)], { type: mime });
        previewUrl.value = URL.createObjectURL(blob);
      }
    }
  } catch {
    if (currentGen !== previewGeneration) return;
    previewError.value = tr("原图读取失败（可能超出大小限制）");
  } finally {
    if (currentGen === previewGeneration) {
      previewLoading.value = false;
    }
  }
}

watch(
  () => detail.value,
  newDetail => {
    void loadRecordMedia(newDetail);
  },
  { immediate: true },
);

/** 详情字段:类型声明的列(含只在详情显示的列)在前,未声明的值补在后。 */
const detailFields = computed<FileDetailField[]>(() => {
  const record = detail.value;
  if (!record) return [];
  const declared = detailColumns.value;
  const declaredIds = new Set(declared.map(column => column.id));
  const fields: FileDetailField[] = declared.map(column => ({
    key: column.id,
    label: columnLabel(selectedNode.value, column),
    value: valueText(record.values[column.id], column, true),
    mono: MONO_KINDS.has(column.kind),
    copyable: true,
  }));
  for (const key of Object.keys(record.values).filter(key => !declaredIds.has(key)).sort()) {
    fields.push({ key, label: key, value: recordValueText(record.values[key], null, { tr }), copyable: true });
  }
  return fields;
});

async function exportCsv(): Promise<void> {
  const node = selectedNode.value;
  if (!node || node.kind !== 'result' || exporting.value) return;
  const label = nodeLabel(node);
  const controller = new AbortController();
  exportController = controller;
  exporting.value = true;
  exportCanCancel.value = true;
  const progress = pushPersistentToast({
    title: tr("正在导出分析结果"),
    detail: tr("{0} · 正在读取记录…", { 0: label }),
    progress: 0,
    action: {
      get label() { return tr("取消"); },
      onClick: cancelExport,
    },
  });
  controller.signal.addEventListener('abort', () => {
    progress.update({ detail: tr("{0} · 正在取消…", { 0: label }), action: { get label() { return tr("取消中…"); }, onClick: () => {}, disabled: true } });
  }, { once: true });
  try {
    const count = await exportRecords(
      (items, exportColumns) => {
        const nameColumn = tr("名称");
        // Headers key the CSV rows: a label that repeats gets its column id.
        const used = new Set([nameColumn]);
        const fields = exportColumns.map(column => {
          let label = columnLabel(node, column);
          if (used.has(label)) label = `${label} (${column.id})`;
          used.add(label);
          return { id: column.id, label };
        });
        const headers = [nameColumn, ...fields.map(field => field.label)];
        return recordsToCsv(
          items.map(item => ({
            [nameColumn]: item.title,
            ...Object.fromEntries(fields.map(field => [field.label, recordCsvValue(item.values[field.id])])),
          })),
          headers,
        );
      },
      csv => {
        exportCanCancel.value = false;
        progress.update({ detail: tr("{0} · 请选择保存位置", { 0: label }), progress: 100, action: undefined });
        return saveTextWithDialog({
          title: tr("导出分析结果"),
          defaultPath: tr("分析结果-{0}.csv", { 0: sanitizeExportFilename(label) }),
          filters: [{ name: 'CSV', extensions: ['csv'] }],
          content: csv,
        });
      },
      {
        signal: controller.signal,
        onProgress: ({ loaded, total }) => progress.update({
          detail: tr("{0} · 已读取 {1} / {2} 条记录", { 0: label, 1: loaded.toLocaleString(), 2: total.toLocaleString() }),
          progress: total ? loaded / total * 100 : 100,
        }),
      },
    );
    if (count === 0) notifyInfo(tr("没有可导出的记录"), tr("请刷新分析结果后重试。"));
  } catch (error) {
    if (controller.signal.aborted) notifyInfo(tr("已取消导出"), label);
    else notifyError(tr("导出失败"), String(error));
  } finally {
    progress.dismiss();
    exportController = null;
    exportCanCancel.value = false;
    exporting.value = false;
  }
}
</script>

<style scoped src="../../../styles/data-table.css"></style>

<style scoped>
.analysis-results { height: 100%; min-height: 0; display: flex; flex-direction: column; background: var(--surface-secondary); color: var(--text-primary); container: analysis-results / inline-size; }
.result-scope { display: flex; align-items: center; gap: 8px; min-width: 0; max-width: 200px; font-size: 13px; font-weight: 600; }
.result-scope > span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.result-scope > small { flex-shrink: 0; }
.reading-mode { flex-shrink: 0; }
.reading-mode button { gap: 6px; }
.severity-filter { flex-shrink: 0; }
.severity-filter button { gap: 6px; }
.severity-filter .has-alerts { color: var(--sev-warn-fg); }

.column-picker-container { position: relative; display: inline-flex; }
.column-picker-trigger { gap: 6px; }
.column-picker-dropdown {
  position: absolute;
  top: calc(100% + 4px);
  right: 0;
  z-index: 20;
  width: 280px;
  max-height: 380px;
  display: flex;
  flex-direction: column;
  background: var(--surface-secondary);
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  box-shadow: var(--modal-shadow);
  overflow: hidden;
}
.column-picker-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 12px;
  border-bottom: 1px solid var(--border-color);
  background: var(--surface-tertiary);
  font-size: 12px;
}
.column-picker-title { font-weight: 600; color: var(--text-primary); }
.column-picker-quick { display: flex; gap: 8px; }
.column-picker-quick .ui-btn.link { font-size: 11px; --ui-btn-h: auto; padding: 0; }
.column-picker-list {
  display: flex;
  flex-direction: column;
  padding: 4px;
  overflow-y: auto;
  max-height: 300px;
}
.column-picker-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 8px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  font-size: 12px;
  color: var(--text-primary);
  user-select: none;
}
.column-picker-item:hover {
  background: var(--hover-bg);
}
.column-picker-item input[type='checkbox'] {
  margin: 0;
  cursor: pointer;
}
.column-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.column-hidden-tag {
  font-size: 10px;
  color: var(--text-secondary);
  background: var(--surface-tertiary);
  padding: 1px 4px;
  border-radius: var(--radius-sm);
}
.record-search { flex: 0 1 320px; min-width: 180px; padding-right: 4px; }
.record-search.applied { border-color: color-mix(in srgb, var(--primary-color) 55%, var(--border-color)); background: color-mix(in srgb, var(--primary-color) 5%, var(--surface-secondary)); }
.record-search.applied > svg { color: var(--primary-color); opacity: 1; }
.toolbar-title { display: inline-flex; align-items: center; gap: 6px; color: var(--text-secondary); font-size: 12px; font-weight: 500; }

.records-table { table-layout: fixed; }
.records-table th:last-child, .records-table td:last-child { position: sticky; right: 0; background: var(--surface-secondary); box-shadow: inset 1px 0 0 color-mix(in srgb, var(--border-color) 70%, transparent); }
.records-table th:last-child { z-index: 3; background: color-mix(in srgb, var(--surface-tertiary) 60%, var(--surface-secondary)); }
.records-table .numeric { text-align: right; font-variant-numeric: tabular-nums; }
.records-table .numeric .sort-header, .records-table .numeric .title-cell { justify-content: flex-end; }
/* 吸附的末列画在行描边之上：选中行由它补齐上、右、下三段描边（下边借底边框，与行描边同一像素行），
   选中浅底下再垫一层不透明底色，横向滚动时下面的列不会透出来。 */
.records-table tr.active td:last-child {
  border-bottom-color: var(--row-selected-ring);
  background: linear-gradient(var(--sidebar-active-bg), var(--sidebar-active-bg)), var(--surface-secondary);
  box-shadow: inset 0 1px 0 var(--row-selected-ring), inset -1px 0 0 var(--row-selected-ring), inset 1px 0 0 color-mix(in srgb, var(--border-color) 70%, transparent);
}
/* 悬停行的吸附列跟着换底，避免行中间出现一块白。 */
.records-table tbody tr:hover:not(.active) td:last-child { background: linear-gradient(var(--surface-tertiary), var(--surface-tertiary)), var(--surface-secondary); }
.records-table td.empty { color: var(--text-secondary); }
.sort-header { display: flex; align-items: center; gap: 4px; width: 100%; min-width: 0; padding: 0; border: 0; background: transparent; color: inherit; font: inherit; text-align: left; cursor: pointer; }
.sort-header > span { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sort-header > svg { flex-shrink: 0; color: var(--text-secondary); opacity: 0.6; }
th[aria-sort='ascending'] .sort-header > svg, th[aria-sort='descending'] .sort-header > svg { color: var(--primary-color); opacity: 1; }
.sort-header:hover > svg { opacity: 1; }
.sort-header:focus-visible { outline: 2px solid var(--primary-color); outline-offset: 2px; }
.title-cell { display: flex; align-items: center; gap: 6px; min-width: 0; }
.title-cell .ui-badge { flex-shrink: 0; }
.title-text { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.records-table td.mono { font-family: var(--font-mono); font-size: 12px; }
.source-files { display: flex; flex-direction: column; align-items: flex-start; gap: 8px; }
.source-file { display: flex; flex-wrap: wrap; align-items: baseline; gap: 4px 8px; max-width: 100%; }
.source-file .ui-btn { max-width: 100%; height: auto; text-align: left; }
.source-file .ui-btn span { overflow-wrap: anywhere; white-space: normal; }
.source-note { color: var(--text-secondary); font-size: 11px; }
.source-missing { margin: 0; color: var(--text-secondary); font-size: 12px; }
.records-table th { position: relative; overflow: visible; }
.col-resizer { position: absolute; top: 0; right: -3px; width: 7px; height: 100%; cursor: col-resize; z-index: 5; user-select: none; }
.col-resizer:hover::after, .col-resizer.active::after { content: ''; position: absolute; top: 0; left: 3px; bottom: 0; width: 2px; background: var(--primary-color); }
.records-table tr { cursor: pointer; }
.records-table .row-number { text-align: right; font-variant-numeric: tabular-nums; }
.records-table td.row-number { color: var(--text-secondary); font-size: 12px; }
.records-table .ui-btn.link { --ui-btn-h: 24px; padding: 0; }
.table-source { max-width: 100%; min-width: 0; }
.table-sources { display: flex; align-items: center; min-width: 0; }
.media-preview-btn { margin-right: 4px; flex-shrink: 0; color: var(--primary-color); }
.media-preview-btn:hover { background: var(--sidebar-active-bg); }
.table-source { flex-shrink: 1; }
.table-source > span { overflow: hidden; text-overflow: ellipsis; }
.source-more { --ui-btn-h: 24px; padding: 0 4px; font-size: 11px; }

.results-body { container: detail-host / inline-size; position: relative; display: flex; flex: 1; min-height: 0; }
.result-table { flex: 1; min-width: 0; display: flex; flex-direction: column; container: analysis-records / inline-size; }
.table-scroll { flex: 1; min-height: 0; overflow: auto; }
.table-scroll.sheet { background: var(--surface-primary); }
.state-box { flex: 1; min-height: 180px; overflow-wrap: anywhere; }
.state-box.error { color: var(--sev-critical-fg); }
.state-box.error .ui-empty-icon { background: var(--sev-critical-bg); color: var(--sev-critical-fg); }

.table-footer { padding-right: 10px; }
.footer-crumb { display: inline-flex; align-items: center; gap: 4px; min-width: 0; max-width: 55%; overflow: hidden; color: var(--text-primary); font-weight: 500; white-space: nowrap; }
.footer-crumb > svg { flex-shrink: 0; color: var(--primary-color); }
.footer-crumb > span { overflow: hidden; text-overflow: ellipsis; }
.pager { display: flex; align-items: center; gap: 2px; margin-left: auto; white-space: nowrap; }
.pager .mono { min-width: 56px; padding: 0 4px; color: var(--text-primary); text-align: center; font-variant-numeric: tabular-nums; }

.dock-media-card {
  display: flex;
  flex-direction: column;
  gap: 8px;
  background: var(--surface-primary);
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  padding: 10px;
}
.dock-media-stage {
  position: relative;
  display: flex;
  align-items: center;
  justify-content: center;
  max-height: 240px;
  background: var(--surface-tertiary);
  border: 1px solid var(--border-color);
  border-radius: var(--radius-sm);
  overflow: hidden;
  cursor: pointer;
}
.dock-media-img {
  max-width: 100%;
  max-height: 240px;
  object-fit: contain;
  transition: transform var(--transition-fast);
}
.dock-media-stage:hover .dock-media-img {
  transform: scale(1.02);
}
.dock-media-badge {
  position: absolute;
  bottom: 6px;
  right: 6px;
  font-size: 11px;
  background: color-mix(in srgb, var(--surface-primary) 85%, transparent);
  color: var(--text-primary);
  padding: 2px 6px;
  border-radius: var(--radius-sm);
  border: 1px solid var(--border-color);
  pointer-events: none;
}
.dock-media-video {
  max-width: 100%;
  max-height: 240px;
  border-radius: var(--radius-sm);
}
.dock-media-audio {
  width: 100%;
}
.dock-media-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  font-size: 12px;
}
.dock-media-path {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-secondary);
}
.dock-media-state {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 16px;
  color: var(--text-secondary);
  font-size: 12px;
}
.dock-media-state.error {
  color: var(--sev-warn-fg);
}

@container analysis-results (max-width: 760px) {
  .footer-completed { display: none; }
  .result-scope { max-width: 140px; }
  .record-search { flex: 1; }
  .results-toolbar.records .ui-toolbar-actions { width: 100%; }
  .severity-filter { display: none; }
}
</style>
