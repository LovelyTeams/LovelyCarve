<template>
  <div class="disk-file-list">
    <FileListToolbar :stack="stack" :current-directory-path="currentDirectoryPath" :loading="loading"
      :deepsearch="deepsearch" :selected-category-count="selectedCategories.size"
      :docked-preview-visible="dockedPreviewVisible" :preview-detached="previewDetached"
      v-model:tree-visible="treeVisible" v-model:category-filter-open="categoryFilterOpen"
      v-model:settings-collapsed="settingsCollapsed" v-model:search-keyword="searchKeyword"
      @go-up="goUp" @go-to="goTo" @copy-path="copyCurrentPath" @search-input="onSearchKeywordInput"
      @clear-search="clearSearchKeyword" @toggle-deepsearch="toggleDeepsearch" @refresh="refresh" @toggle-preview="togglePreview"
    />

    <div v-if="indexOnly" class="ui-alert info offline-notice" role="status"><CircleAlert :size="16" /><span>{{ tr("离线索引 · 原镜像不可用，可浏览已索引目录、筛选文件和查看属性。正文预览、导出、哈希及分析需恢复原镜像。") }}</span></div>
    <div v-if="error" class="error-banner" role="alert"><CircleAlert :size="14" /><span class="mono">{{ error }}</span></div>

    <div class="body">
      <UiTransition kind="horizontal"><DiskFileTree
        :key="imageId + ':' + (volumeRevision ?? '')"
        v-show="treeVisible"
        :image-id="imageId"
        :path-stack="stack"
        @navigate="onTreeNavigate"
        @open-file="onTreeOpenFile"
      /></UiTransition>
      <div class="right-area">
        <UiTransition><FileCategoryBar
          v-if="categoryFilterOpen"
          :counts="displayedCategoryCounts.counts"
          :counts-complete="deepsearch || directoryClientFilter"
          :total="displayedCategoryCounts.total"
          :selected="selectedCategories"
          @toggle="toggleCategory"
          @clear="clearCategories"
        /></UiTransition>

        <div class="content-row">
          <div class="left-col">
            <FileListTable
              :visible-columns="visibleColumns" :visible-items="visibleItems" :row-count="filteredEntries.length" :loaded-count="entries.length"
              :loading="loading" :deepsearch="deepsearch" :sort-busy-key="sortBusyKey" :sort-key="sortKey" :sort-dir="sortDir"
              :header-checked="headerChecked" :header-indeterminate="headerIndeterminate"
              :padding-top="paddingTop" :padding-bottom="paddingBottom" :selected-key="selectedKey" :checked-ids="checkedIds"
              :dir-counts="dirCounts" :digest-cache="digestCache" :ftype-cache="ftypeCache" :marquee-rect="marqueeRect"
              :full-path-of="fullPathOf" :tag-of-node="tagOfNode" :is-column-filtered="isColumnFiltered" :filter-summary-of="filterSummaryOf"
              @container-change="tableWrapRef = $event" @marquee-start="onMarqueeMouseDown" @keydown="onTableKeydown"
              @toggle-all="toggleAll" @sort="cycleSort" @filter="toggleFilterPopover" @row-click="onRowClick"
              @row-double-click="onRowDblClick" @row-mouse-down="onRowMouseDown" @context-menu="onRowContextMenu"
              @name-click="onNameClick" @check-click="onCheckClick"
            />
          </div>

          <FileColumnSettings :open="!settingsCollapsed" :columns="COLUMNS" :column-visible="columnVisible" @toggle="toggleColumn" />
        </div>

        <FileListFooter :deepsearch="deepsearch" :flat-total="flatTotal" :flat-truncated="flatTruncated"
          :index-only="indexOnly"
          :directory-total="directoryTotal" :directory-cursor="directoryCursor" :loaded-count="entries.length"
          :displayed-count="filteredEntries.length" :active-filter-count="activeFilterCount" :snapshot-status="snapshotStatus"
          :checked-stats="checkedStats" :active-row="activeRow" :checked-files="checkedFiles" :checked-dirs="checkedDirs"
          @clear-filters="clearAllFilters" @clear-checked="clearChecked"
          @export-file="exportSelectedFile" @export-directory="exportSelectedDir"
          @preview="detailNode = activeRow; previewVisible = true"
        />

      </div>
      <UiTransition kind="dock">
      <FilePreviewDock v-if="dockedPreviewVisible" :name="detailNode?.name ?? tr('文件预览')" :opening="previewOpening" :can-detach="!!detailNode"
        @detach="detachPreview" @close="previewVisible = false">
        <DiskFileDetail
          v-if="detailNode"
          ref="detailPreview"
          standalone
          :image-id="imageId"
          :node="detailNode"
          :index-only="indexOnly"
          :reading-state="previewReadingState"
          :focus-target="detailFocus?.nodeId === detailNode.id ? detailFocus : undefined"
          :full-path="fullPathOf(detailNode)"
          :digest="digestCache.get(detailNode.id)"
          :ident="ftypeCache.get(detailNode.id) ?? null"
          @identified="onDetailIdentified"
        />
        <div v-else class="ui-empty preview-empty"><span class="ui-empty-icon accent"><PanelRightOpen :size="26" /></span><p class="ui-empty-title">{{ tr("选择文件，即刻预览") }}</p><p class="ui-empty-desc">{{ tr("单击文件查看内容，双击目录进入") }}</p><p class="ui-empty-desc">{{ tr("拖动上方标题栏，可拆分为独立窗口") }}</p></div>
      </FilePreviewDock>
      </UiTransition>
    </div>

    <DiskRowContextMenu
      :visible="rowMenu.visible"
      :x="rowMenu.x"
      :y="rowMenu.y"
      :has-node="!!rowMenu.node"
      :copy-count="copyCount"
      :checked-file-count="checkedFiles.length"
      :checked-dir-count="checkedDirs.length"
      :can-export-file="!indexOnly && canExportFile"
      :can-export-dir="!indexOnly && canExportDir"
      :can-hash="!indexOnly && canHash"
      :can-identify="!indexOnly && canIdentify"
      :can-plugin-task="!indexOnly && canPluginTask"
      :can-return-to-original-dir="activeFilterCount > 0 || deepsearch"
      :can-mount="!indexOnly && rowCanMount"
      :mount-busy="mountBusy"
      :mounted="!!rowMountView"
      :mount-label="rowMountLabel"
      v-model:hash-menu-open="hashMenuOpen"
      :hash-menu-flip="hashMenuFlip"
      :hash-options="HASH_MENU_OPTIONS"
      @tag-pick="onTagPick"
      @show-detail="onShowDetail"
      @return-to-original-dir="onReturnToOriginalDir"
      @toggle-mount="onToggleRowMount"
      @export-file="onExportFile"
      @export-dir="onExportDir"
      @compute-hash="onComputeHash"
      @identify-type="onIdentifyType"
      @plugin-task="onPluginTask"
      @copy-names="onCopyNames"
      @copy-paths="onCopyPaths"
      @copy-forensic="onCopyForensic"
    />

    <PluginTaskDialog
      :visible="pluginTaskDialog.visible"
      :image-id="imageId"
      :node-id="pluginTaskDialog.node?.id ?? null"
      :file-name="pluginTaskDialog.node?.name ?? ''"
      @close="pluginTaskDialog.visible = false"
    />

    <FileColumnFilterPopover :controls="fileFilters" />

    <ExportModePicker :open="modePickerOpen" @choose="chooseExportMode" />

  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import UiTransition from '../../../components/ui/UiTransition.vue';
import { ref, computed, watch, reactive, shallowRef, onActivated, onDeactivated, onUnmounted, onScopeDispose } from 'vue';
import { type FsTypeIdent, type StoredDigest } from '../../../lib/ipc/fs/derived';
import {
  PanelRightOpen,
  CircleAlert,
} from 'lucide-vue-next';
import DiskFileDetail from '../../preview/components/DiskFileDetail.vue';
import FilePreviewDock from '../../preview/components/FilePreviewDock.vue';
import { useFilePreviewBridge } from '../composables/useFilePreviewBridge';
import type { FilePreviewReadingState } from '../../../lib/models/filePreviewReadingState';
import { useFileRowMenu } from '../composables/useFileRowMenu';
import FileCategoryBar from './FileCategoryBar.vue';
import DiskFileTree from './DiskFileTree.vue';
import FileListTable from './FileListTable.vue';
import FileListFooter from './FileListFooter.vue';
import FileListToolbar from './FileListToolbar.vue';
import FileColumnSettings from './FileColumnSettings.vue';
import DiskRowContextMenu from './DiskRowContextMenu.vue';
import PluginTaskDialog from '../../tasks/components/PluginTaskDialog.vue';
import ExportModePicker from './ExportModePicker.vue';
import { useFileExports } from '../composables/useFileExports';
import { projectFileRows } from '../model/clientRows';
import { notifyError } from '../../../composables/useToast';
import { type FileTag } from '../model/tags';
import { useFileTags } from '../composables/useFileTags';
import { useEmptyDirFilter } from '../composables/useEmptyDirFilter';
import {
  COLUMNS,
  DEFAULT_VISIBLE,
  formatBytes,
  pathFromStack,
  rowKeyOf,
  type DiskFsNode,
} from '../model/model';
import { useFileCategoryFilter } from '../composables/useFileCategoryFilter';
import {
  FLAT_PREFETCH_ROWS,
  FLAT_SERVER_FILTER_COLUMNS,
  buildFlatFilter,
  flatSortColumn,
} from '../model/flatQuery';
import { useFileFilters } from '../composables/useFileFilters';
import FileColumnFilterPopover from './FileColumnFilterPopover.vue';
import { useMarqueeSelection } from '../composables/useMarqueeSelection';
import { useVolumeSnapshotStatus } from '../composables/useVolumeSnapshotStatus';
import { useUiPreferences } from '../../../composables/useUiPreferences';
import { useDirectoryQuery } from '../composables/useDirectoryQuery';
import { useFlatFileQuery } from '../composables/useFlatFileQuery';
import { useDirectoryMetadata } from '../composables/useDirectoryMetadata';
import { useFileDerivedData } from '../composables/useFileDerivedData';
import { useFileDerivedActions } from '../composables/useFileDerivedActions';
import { useFileSelection } from '../composables/useFileSelection';
import { useFileActions } from '../composables/useFileActions';
import { useFileBrowseMode } from '../composables/useFileBrowseMode';
import { useFileTableViewport } from '../composables/useFileTableViewport';
import { useFileNavigation } from '../composables/useFileNavigation';
import { useFileTableKeyboard } from '../composables/useFileTableKeyboard';
import { useReadonlyMountActions } from '../composables/useReadonlyMountActions';
import { HASH_MENU_OPTIONS } from '../model/derivedData';

type FsNode = DiskFsNode;

// 稳定组件名：本面板被 AppShell 用 <KeepAlive> 缓存（切到镜像信息再切回时
// 保留导航栈 / 滚动 / 勾选状态）。显式命名便于 devtools 辨识，也不依赖文件名推断。
defineOptions({ name: 'DiskFileList' });

const props = defineProps<{ imageId: string; indexOnly?: boolean; volumeRevision?: string; activatePreview?: (imageId: string) => boolean }>();
const { mountTarget, mountFolderRoot } = useUiPreferences();

const {
  modePickerOpen,
  chooseExportMode,
  exportFile: exportSelectedFile,
  exportDir: exportSelectedDir,
} = useFileExports({
  imageId: () => props.imageId,
  formatBytes,
});

// shallowRef：大目录可达数十万条目，深层 Proxy 包装在过滤/排序全量遍历时是
// 主要开销；列表只做整体赋值（全显分页追加也是整段 concat），浅响应足够。
const entries = shallowRef<FsNode[]>([]);

const loading = ref(false);
const error = ref<string | null>(null);
// 单选高亮的「光标行」标识：用行的唯一 rowKey，不是裸 id。全显（递归平铺）
// 下硬链接文件按 (文件, 父目录) 边各出一行、共享同一个 MFT 记录号（= 同 id），
// 若按 id 高亮，点其中一行会连带点亮所有同 id 兄弟行（「选一个选中俩」）。
// 逐级浏览无 rowKey 时回退到 id（每条记录一行，等价旧行为）。
const selection = useFileSelection({ rows: () => filteredEntries.value, marqueeJustEnded: () => marqueeJustEnded() });
const { selectedKey, checkedIds, headerChecked, headerIndeterminate, toggleAll, onCheckClick,
  checkedFiles, checkedDirs, checkedStats, clearChecked } = selection;
// 「隐藏空目录」开关（与左侧目录树共用同一单例），及当前列表里各目录的子树
// 判定缓存（id → 整棵子树是否无文件；缺值时过滤退回 dirCounts===0 直接判断）。
const { hideEmptyDirs } = useEmptyDirFilter();
const detailNode = ref<FsNode | null>(null);
const detailPreview = ref<{ getReadingState: () => FilePreviewReadingState } | null>(null);
const previewReadingState = ref<FilePreviewReadingState>();
const detailFocus = ref<{ nodeId: string; offset: number; length: number } | null>(null);
const previewVisible = ref(false);
const treeVisible = ref(true);
const categoryFilterOpen = ref(false);
const activeRow = computed(() => {
  const list = filteredEntries.value;
  return list[selectedIndexIn(list)] ?? null;
});
const { previewOpening, previewDetached, dockedPreviewVisible, togglePreview: togglePreviewVisibility, detachPreview, onDetailIdentified } = useFilePreviewBridge({
  indexOnly: () => props.indexOnly === true,
  imageId: () => props.imageId, activate: imageId => props.activatePreview?.(imageId) ?? true,
  node: detailNode, focusTarget: detailFocus, visible: previewVisible, fullPath: fullPathOf,
  readingState: previewReadingState,
  snapshotReadingState: () => detailPreview.value?.getReadingState() ?? previewReadingState.value,
  digest: nodeId => digestCache.get(nodeId), ident: nodeId => ftypeCache.get(nodeId),
  identified: (imageId, nodeId, ident) => {
    ftypeCache.set(nodeId, ident);
    columnVisible.ftype = true;
    persistFtypes(imageId, { [nodeId]: ident });
  },
});
function togglePreview() {
  // Opening preview explicitly must use the current cursor, including directories.
  if (!previewDetached.value && !previewVisible.value && activeRow.value) detailNode.value = activeRow.value;
  togglePreviewVisibility();
}
let loadGeneration = 0;
const navigation = useFileNavigation({
  imageId: () => props.imageId, loadGeneration: () => loadGeneration,
  entries, selectedKey, detailNode, detailFocus, previewVisible, error,
  filteredRows: () => filteredEntries.value,
  container: () => tableWrapRef.value,
  scrollToRow: index => scrollRowIntoView(index),
  rebasePath: () => rebasePathFilterToStack(), loadCurrent,
  prepareLocate: () => { closeDeepSearch(); browseMode.consumeReload(); clearAllFilters(); hideFilterPopover(); },
});
const { stack, currentDirectoryPath, pendingRestore, directoryTargetId, onTreeNavigate, onTreeOpenFile,
  openFileByNodeId, openFileBySource, onActivate, withinActivateCooldown, goUp, goTo } = navigation;

const {
  selectedCategories,
  categoryOfNode,
  toggleCategory,
  clearCategories,
  resetCategoryCache,
  categoryCounts,
} = useFileCategoryFilter(entries);

const derivedData = useFileDerivedData({
  imageId: () => props.imageId,
  queryGeneration: () => loadGeneration,
  revealTypeColumn: () => { columnVisible.ftype = true; },
});
const { digestCache, ftypeCache, hydrateDigests, hydrateFtypes, persistDigests, persistFtypes } = derivedData;
const { digestCheckedFiles, identifyCheckedFiles } = useFileDerivedActions({
  imageId: () => props.imageId,
  data: derivedData,
  revealColumns: keys => { for (const key of keys) columnVisible[key] = true; },
});
const { tagOf, setTag, hydrateTagsForNodes, resetHydration } = useFileTags();
const { rowMenu, hashMenuOpen, hashMenuFlip, hideRowMenu, onRowContextMenu } = useFileRowMenu({
  selectedKey, previewVisible,
  filterVisible: () => filterPopover.visible, closeFilter: () => hideFilterPopover(),
  exportModeOpen: () => modePickerOpen.value, cancelExportMode: () => chooseExportMode(null),
  hasChecked: () => checkedIds.value.size > 0, clearChecked,
  refreshMounts: () => refreshReadonlyMounts(),
});

function hydrateDerivedForNodes(nodes: FsNode[], gen: number) {
  void hydrateDigests(nodes, gen);
  void hydrateFtypes(nodes, gen);
  void hydrateTagsForNodes(
    props.imageId,
    nodes.map(node => node.id),
  );
}

function tagOfNode(n: FsNode): FileTag | undefined {
  return tagOf(props.imageId, n.id);
}

// ── 文件全显（deepsearch / allowAllFiles）──
// 列表请求增加一个布尔（前端 deepsearch，后端 allowAllFiles）。
// 开启后当前节点从「列一层」变成「以该节点为根递归平铺全部文件」；keyword /
// 列漏斗 / 排序全部由服务端在索引上完成，按 skip/limit 分页，总数随行一次
// 返回；表格按虚拟滚动位置逐页拉取（infinite 模型），永不把整棵子树拉到
// 前端。目录行不出现在平铺结果里，导航交给左侧树与面包屑。
const browseMode = useFileBrowseMode({
  filters: () => filters, effectiveFilters: () => effectiveFilters,
  rebasePath: () => rebasePathFilterToStack(), loadCurrent,
});
const { deepsearch, searchKeyword, appliedKeyword, toggleDeepsearch, onSearchKeywordInput,
  clearSearchKeyword, closeDeepSearch } = browseMode;

const settingsCollapsed = ref(true);

const { activeCase, snapshotStatus } = useVolumeSnapshotStatus({
  imageId: () => props.imageId,
  stack,
});

// 案件在镜像装载之后才创建/打开时：先把会话里已算的哈希补存进案件目录
// （此前 persist 因无案件被丢弃），再按当前列表节点水合存档里可能已有的
// 数据（切换到另一个案件时镜像若已登记过哈希/类型）。镜像未变 imageId
// watch 不会重跑，必须单独听。
watch(
  () => activeCase.value?.id ?? null,
  caseId => {
    navigation.invalidateLocate();
    if (!caseId) return;
    if (digestCache.size > 0) {
      const entries: Record<string, StoredDigest> = {};
      for (const [nodeId, digest] of digestCache) entries[nodeId] = digest;
      persistDigests(props.imageId, entries);
    }
    derivedData.clearHydration();
    void hydrateDigests(entries.value, loadGeneration);
    // 类型识别同理：换案件后把会话内已识别的补存进新案件目录，再水合
    // 存档里可能已有的。
    if (ftypeCache.size > 0) {
      const entries: Record<string, FsTypeIdent> = {};
      for (const [nodeId, ident] of ftypeCache) entries[nodeId] = ident;
      persistFtypes(props.imageId, entries);
    }
    void hydrateFtypes(entries.value, loadGeneration);
    // 标签同理：换案件后已水合标记作废，按当前列表节点重拉；本地优先
    // 的差异会通过增量 upsert 回写，不覆盖未水合的其它案件标签。
    resetHydration();
    void hydrateTagsForNodes(
      props.imageId,
      entries.value.map(node => node.id),
      true,
    );
  },
);

defineExpose({ openFileByNodeId, openFileBySource, imageId: computed(() => props.imageId) });

const columnVisible = reactive<Record<string, boolean>>(
  Object.fromEntries(COLUMNS.map(c => [c.key, DEFAULT_VISIBLE.has(c.key)])),
);

const fileFilters = useFileFilters({
  imageId: () => props.imageId, entries, stack, flatMode: deepsearch, selectedCategories, loading, clearCategories,
  container: () => tableWrapRef.value, fullPathOf, loadCurrent, closeRowMenu: hideRowMenu,
});
const { filters, effectiveFilters, sizeFilter, timeFilters, activeTimeFilterKeys, appliedSizePredicate, matchesTimeFilter,
  resetSizeFilter, resetTimeFilter, activeFilterCount, isColumnFiltered, filterSummaryOf, clearAllFilters,
  filterPopover, toggleFilterPopover, hideFilterPopover,
  rebasePathFilterToStack, resolveAbsoluteDirStack, invalidatePathLevels, invalidatePathFollow } = fileFilters;

const sortKey = ref<string | null>(null);
const sortDir = ref<'asc' | 'desc'>('asc');

const visibleColumns = computed(() => COLUMNS.filter(c => columnVisible[c.key]));

async function loadCurrent() {
  if (!props.imageId) {
    entries.value = [];
    return;
  }
  loadGeneration += 1;
  // 任何装载（含手动导航、刷新、切镜像）都作废在途的路径跟随，否则慢速取层
  // 返回后会把面包屑改回旧的筛选路径、覆盖用户刚做的导航。跟随自身的
  // loadCurrent 在其最后一步，此处自增不会误伤。
  invalidatePathFollow();
  const gen = loadGeneration;
  const parentId = stack.value[stack.value.length - 1].id;
  loading.value = true;
  error.value = null;
  selection.reset();
  // 进行中的框选一并中止：装载会清空勾选并换掉列表，留着框选的下一个
  // mousemove 会按旧锚点把新列表的行勾回来。
  cancelMarquee();
  detailNode.value = null;
  // 菜单挂着旧列表的节点，任何重载都得收掉，免得菜单动作落在过期目标上。
  hideRowMenu();
  directoryMetadata.clear();
  directoryCursor.value = null;
  try {
    if (deepsearch.value) {
      await loadFlatFirstPage(parentId, gen);
    } else {
      const targetId = directoryTargetId.value ?? pendingRestore.value?.childId;
      directoryTargetId.value = null;
      await loadDirectoryFirstPage(parentId, gen, { targetId, scrollTop: pendingRestore.value?.scrollTop });

    }
  } catch (e) {
    if (gen !== loadGeneration) return;
    error.value = String(e);
    entries.value = [];
  } finally {
    if (gen === loadGeneration) {
      loading.value = false;
      sortBusyKey.value = null;
    }
  }
}

// Derived fields and recursive empty-directory checks require the complete
// directory. Fetch bounded pages before applying those legacy predicates.
const directoryClientFilter = computed(() => hideEmptyDirs.value
  || Boolean(effectiveFilters.path?.trim())
  || Object.entries(effectiveFilters).some(([key, value]) => value && !FLAT_SERVER_FILTER_COLUMNS.has(key))
  || Boolean(sortKey.value && !flatSortColumn(sortKey.value)));
const directoryRequest = computed(() => directoryClientFilter.value ? {} : {
  filter: buildFlatRequest().filter,
  sort: flatSortColumn(sortKey.value), descending: sortDir.value === 'desc',
});
const directoryRequestSignature = computed(() => JSON.stringify({
  request: directoryRequest.value, client: directoryClientFilter.value,
  filters: effectiveFilters, hideEmpty: hideEmptyDirs.value,
}));

// ── 全显分页装载 ──

/** 当前全显请求的服务端参数：关键词、列漏斗
 *  编成的表达式过滤、排序列与方向。 */
function buildFlatRequest() {
  return {
    keyword: appliedKeyword.value || null,
    filter: buildFlatFilter({
      keyword: '',
      columnFilters: effectiveFilters,
      sizeFilter,
      timeFilters,
      selectedCategories: selectedCategories.value,
    }),
    sort: flatSortColumn(sortKey.value),
    sortdesc: sortDir.value === 'desc',
  };
}

// 请求签名：关键词 / 过滤 / 排序任一变化都意味着当前已加载页作废，重新从
// 第一页装载。
const flatRequestSignature = computed(() => JSON.stringify(buildFlatRequest()));

/** 后端路径相对当前节点；拼上面包屑前缀得到与逐级浏览一致的绝对全路径。
 *  行键带全局行号：硬链接同一节点 id 可出现多行。 */
const directoryMetadata = useDirectoryMetadata({ imageId: () => props.imageId, generation: () => loadGeneration });
const { dirCounts, emptySubtree, fetchEmptySubtrees } = directoryMetadata;
const directoryQuery = useDirectoryQuery({
  imageId: () => props.imageId,
  parentId: () => stack.value.at(-1)!.id,
  generation: () => loadGeneration,
  enabled: () => !deepsearch.value,
  query: () => directoryRequest.value,
  signature: () => directoryRequestSignature.value,
  clientFilter: () => directoryClientFilter.value,
  entries, loading,
  hydrated: hydrateDerivedForNodes,
  firstPageLoaded: async (nodes, generation) => { if (hideEmptyDirs.value) await fetchEmptySubtrees(nodes, generation); },
  beforeAppend: () => viewport.beforeAppend(),
  wantedThrough: () => range.value.end + FLAT_PREFETCH_ROWS,
});
const { directoryTotal, directoryCursor, loadDirectoryFirstPage } = directoryQuery;
const flatQuery = useFlatFileQuery({
  imageId: () => props.imageId,
  parentId: () => stack.value.at(-1)!.id,
  pathPrefix: () => currentDirectoryPath.value === '/' ? '' : currentDirectoryPath.value,
  generation: () => loadGeneration,
  enabled: () => deepsearch.value,
  request: buildFlatRequest,
  signature: () => flatRequestSignature.value,
  entries, loading,
  hydrated: hydrateDerivedForNodes,
  beforeAppend: () => viewport.beforeAppend(),
});
const { flatTotal, flatTruncated, flatCategoryCounts, loadFlatFirstPage } = flatQuery;

const displayedCategoryCounts = computed(() =>
  deepsearch.value ? flatCategoryCounts.value : categoryCounts.value,
);

// 列表已展示时打开「隐藏空目录」：为当前条目补一次子树判定，让过滤即时生效
// （filteredEntries 读 hideEmptyDirs，开关一变即重算，这里补齐它依赖的数据）。
watch(hideEmptyDirs, on => {
  if (on) void fetchEmptySubtrees(entries.value, loadGeneration);
});

const sortContext = {
  digestOf: (nodeId: string) => digestCache.get(nodeId),
  tagOf: (node: FsNode) => tagOfNode(node),
  ftypeLabelOf: (nodeId: string) => ftypeCache.get(nodeId)?.label ?? '',
  fullPathOf,
};

const filteredEntries = computed(() => projectFileRows({
  entries: entries.value, deepsearch: deepsearch.value, directoryClientFilter: directoryClientFilter.value,
  effectiveFilters, digestCache, ftypeCache, tagOfNode, fullPathOf,
  selectedCategories: selectedCategories.value, categoryOfNode, sizeFilter, appliedSizePredicate,
  activeTimeFilterKeys: activeTimeFilterKeys.value, matchesTimeFilter,
  hideEmptyDirs: hideEmptyDirs.value, stackDepth: stack.value.length, emptySubtree, dirCounts,
  sortKey: sortKey.value, sortDir: sortDir.value, sortContext,
}, tr));

// Virtualize the table body so a 10k-entry directory renders only the visible
// rows of DOM instead of 10k <tr>. Row height is pinned in CSS (.disk-file-row
// > td) so this fixed itemHeight stays exact and rows don't drift against the
// scroll position.
const ROW_HEIGHT = 36;
const tableWrapRef = ref<HTMLElement | null>(null);
// 全显下滚动高度按服务端总数撑开（infinite 模型）：未加载的行先留白，可视
// 区间越过已加载末尾时按需补页。
const flatVirtualCount = computed(() => deepsearch.value ? flatTotal.value : directoryClientFilter.value ? 0 : directoryTotal.value);
const viewport = useFileTableViewport({
  entries, rows: filteredEntries, loading, flatMode: deepsearch, container: tableWrapRef, rowHeight: ROW_HEIGHT,
  virtualCount: flatVirtualCount, selectedKey, detailNode, pendingRestore, sortKey, sortDir,
  flatSignature: flatRequestSignature, directorySignature: directoryRequestSignature, flatQuery, directoryQuery,
  generation: () => loadGeneration, loadCurrent, hydrateVisible: directoryMetadata.hydrateVisible,
  scrollToRow: index => scrollRowIntoView(index),
});
const { visibleItems, paddingTop, paddingBottom, range, sortBusyKey, cycleSort } = viewport;

// 单击文件行立即打开详情；目录行在预览已打开或拆分时同步目录属性，
// 预览关闭时只选中，仍可从预览按钮或右键「查看详情」主动打开。
// 激活导航后 300ms 内三个入口都忽略事件：导航后列表瞬间重载（快照级响应在
// 双击间隙内就渲染完），双击的第二击/dblclick 会落在新行上——轻则误开详情，
// 重则误入第二层目录。窗口不做一次性消费：一次双击会同时产生杂散 click 与
// dblclick 两个事件，消费一次就放过后者。
function onRowClick(node: FsNode) {
  // 框选结束的 mouseup 若与按下落在同一行，会派生一次杂散 click——
  // 不能让它把刚框好的勾选覆盖成单行详情选中。
  if (marqueeJustEnded()) return;
  if (withinActivateCooldown()) return;
  selectedKey.value = rowKeyOf(node);
  if (node.kind !== 'dir' || previewVisible.value || previewDetached.value) {
    detailNode.value = node;
    previewVisible.value = true;
  }
}

function onRowDblClick(node: FsNode) {
  // 同一行内两次快速框选会被浏览器凑成 dblclick——不当导航
  if (marqueeJustEnded()) return;
  if (withinActivateCooldown()) return;
  if (node.kind !== 'dir') return;
  onActivate(node);
}

function onNameClick(e: MouseEvent, node: FsNode) {
  // 文件名不拦截，冒泡给行点击开详情；目录名是「进入」链接。
  if (node.kind !== 'dir') return;
  e.stopPropagation();
  if (withinActivateCooldown()) return;
  selectedKey.value = rowKeyOf(node);
  onActivate(node);
}

function onRowMouseDown(e: MouseEvent) {
  // 双击是进入目录的主手势：第二次 mousedown 的默认行为是按词选中单元格
  // 文本，阻掉它。普通左键拖动现在是框选勾选（见下方），文本拖选复制走
  // Alt+拖动。
  if (e.detail > 1) e.preventDefault();
}

// ── 左键框选勾选 ──
const { marqueeRect, onMarqueeMouseDown, cancelMarquee, marqueeJustEnded } = useMarqueeSelection({
  container: tableWrapRef,
  rows: filteredEntries,
  rowHeight: ROW_HEIGHT,
  checkedIds,
  isBlocked: () => loading.value,
});

// 返回上级（goUp/goTo）要恢复的滚动位置与「刚退出的子目录」行——后者选中
// 高亮作视线锚点。前进导航与树/地址栏跳转不武装，仍回到顶部。openDetail：
// 树里单击文件触发的导航，装载后除选中该子项外，还打开其详情面板。
function refresh() {
  // 刷新意味着重新读盘（快照可能已重建），补全的层缓存一并作废；全显下还要
  // 让后端丢掉该起点的实时遍历缓存重走。
  invalidatePathLevels();
  if (deepsearch.value) flatQuery.requestRefresh();
  loadCurrent();
}
watch(() => props.volumeRevision, () => refresh());

// ── 表格键盘导航 ──
// 容器（tabindex=0）持焦时生效。过滤气泡/弹窗都 Teleport 到 body，焦点不在
// 容器内时不会进到这里，不会劫持输入框里的按键。
// ↑/↓/PageUp/PageDown/Home/End 移动选中光标，Enter 进目录/开文件详情，
// Backspace / Alt+← 返回上级，Space 勾选，Ctrl+A 全选；可打印字符累积进
// 短暂缓冲串做前缀环形查找（type-ahead，纯定位，不改过滤器）。
const { selectedIndexIn, scrollRowIntoView, onTableKeydown } = useFileTableKeyboard({
  rows: () => filteredEntries.value,
  selectedKey, checkedIds, container: tableWrapRef, rowHeight: ROW_HEIGHT,
  menuOpen: () => rowMenu.visible,
  loading: () => loading.value,
  toggleAll, goUp, activate: onActivate,
  preview: node => { detailNode.value = node; previewVisible.value = true; },
  setAnchor: selection.setAnchor,
});

function toggleColumn(key: string) {
  columnVisible[key] = !columnVisible[key];
  // 隐藏列时清掉它的过滤条件：列头漏斗随列一起消失，留着条件会变成
  // 看不见也清不掉（只剩“清除全部”）的隐形过滤，对取证列表太危险。
  if (!columnVisible[key]) {
    if (filters[key]) filters[key] = '';
    if (key === 'size') resetSizeFilter();
    resetTimeFilter(key);
    if (filterPopover.visible && filterPopover.key === key) hideFilterPopover();
  }
}

// 文件类别 → 图标与配色统一在 FileNodeIcon（目录树共用同一份）。

function fullPathOf(n: FsNode): string {
  return pathFromStack(n, stack.value);
}

function parentDirectoryPathOf(node: FsNode): string {
  const fullPath = fullPathOf(node);
  const normalized = fullPath.length > 1 ? fullPath.replace(/\/+$/, '') : fullPath;
  const cut = normalized.lastIndexOf('/');
  return cut <= 0 ? '/' : normalized.slice(0, cut);
}

const { mountBusy, rowCanMount, rowMountView, rowMountLabel, refreshReadonlyMounts, onToggleRowMount } = useReadonlyMountActions({
  imageId: () => props.imageId,
  node: () => rowMenu.node,
  target: () => mountTarget.value,
  folderRoot: () => mountFolderRoot.value,
  caseDirectory: () => activeCase.value?.dir ?? null,
  closeMenu: hideRowMenu,
});

watch(
  () => props.imageId,
  () => {
    stack.value = [{ id: 'root', name: '/' }];
    invalidatePathLevels();
    // 哈希/研判结果按节点 id 缓存，只在本镜像内有效。
    derivedData.reset();
    // 案件目录里可能存着本镜像此前算过的哈希/类型；loadCurrent 取得
    // 当前条目后会按节点集合异步回填列缓存。
    // 标签存档由 loadCurrent 取得当前条目后按节点集合水合，避免切镜像时
    // 先拉整镜像标签表。
    // 路径筛选是上一块盘的地址，对新镜像毫无意义，留着会把新镜像的根
    // 过滤成空表；热切标签页时组件不重建，必须显式清。
    if ((filters.path ?? '').trim().startsWith('/')) filters.path = '';
    // 分类选择与名称→类别缓存都是上一块盘的，切镜像清掉（缓存按名缓存、跨镜像
    // 同名同类别本可复用，但顺手清掉也能给长会话的缓存设个上界）。
    selectedCategories.value = new Set();
    resetCategoryCache();
    // 全显状态是上一块盘的视图状态，
    // 切镜像回到逐级浏览；关键词与类别计数缓存一并清掉。
    browseMode.reset();
    flatQuery.resetCategoryCounts();
    void refreshReadonlyMounts();
    loadCurrent();
  },
  { immediate: true },
);

const { copyCount, copyCurrentPath, onCopyNames, onCopyPaths, onCopyForensic,
  canExportFile, canExportDir, canHash, canIdentify, canPluginTask, pluginTaskDialog, onPluginTask,
  onExportFile, onExportDir, onComputeHash, onIdentifyType, onTagPick } = useFileActions({
  imageId: () => props.imageId, rows: () => filteredEntries.value, node: () => rowMenu.node,
  currentPath: () => currentDirectoryPath.value, fullPathOf, selection, closeMenu: hideRowMenu,
  exportFile: exportSelectedFile, exportDirectory: exportSelectedDir, digest: digestCheckedFiles, identify: identifyCheckedFiles,
  setTag, revealTagColumn: () => { columnVisible.tag = true; },
});

function onShowDetail() {
  const node = rowMenu.node;
  hideRowMenu();
  if (!node) return;
  selectedKey.value = rowKeyOf(node);
  detailNode.value = node;
  previewVisible.value = true;
}

let originalDirectoryRequest = 0;
watch(() => props.imageId, () => { originalDirectoryRequest++; }, { flush: 'sync' });
onScopeDispose(() => { originalDirectoryRequest++; });
async function onReturnToOriginalDir() {
  const request = ++originalDirectoryRequest;
  const node = rowMenu.node;
  hideRowMenu();
  if (!node) return;
  const parentPath = parentDirectoryPathOf(node);
  const targetStack = await resolveAbsoluteDirStack(parentPath);
  if (request !== originalDirectoryRequest) return;
  if (!targetStack) {
    notifyError(tr("回到原始目录失败"), tr("无法定位目录：{0}", { 0: parentPath }));
    return;
  }
  clearAllFilters();
  // 回到原始目录 = 退出全显、按逐级浏览定位到该文件所在层。
  deepsearch.value = false;
  searchKeyword.value = '';
  appliedKeyword.value = '';
  pendingRestore.value = { scrollTop: 0, childId: node.id };
  stack.value = targetStack;
  loadCurrent();
}

// 打标：有勾选按勾选集（与复制/导出口径一致），没有按光标行。
// 本面板被 <KeepAlive> 缓存，切走不销毁。文档级键盘监听只在面板可见（激活）时
// 挂载——否则在「镜像信息」等其他面板按 Esc 会误消费到这里（关详情 / 清勾选）。
onActivated(() => {
  void refreshReadonlyMounts();
  // 切走时退出了全显：回来按逐级浏览把当前目录重载一次。
  if (browseMode.consumeReload()) loadCurrent();
});

onDeactivated(() => {
  originalDirectoryRequest++;
  navigation.invalidateLocate();
  // 离开面板时收起可能还开着的右键菜单 / 过滤气泡，免得它们悬在别的面板上。
  hideRowMenu();
  hideFilterPopover();
  // 跳转离开即复位全显状态；重载留到下次激活。
  closeDeepSearch();
});

onUnmounted(() => {
  navigation.invalidateLocate();
  hideFilterPopover();
});
</script>

<style scoped>

.disk-file-list {
  display: flex;
  flex-direction: column;
  height: 100%;
  font-family: var(--font-ui);
  color: var(--text-primary);
  background: var(--surface-secondary);
}

.error-banner {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  background: var(--sev-critical-bg);
  color: var(--sev-critical-fg);
  font-size: 12px;
  border-bottom: 1px solid var(--sev-critical-border);
}

.error-banner > svg { flex-shrink: 0; }

.offline-notice { flex-shrink: 0; margin: 8px 12px 0; }
.offline-notice > svg { flex-shrink: 0; }

/* ── Body ── */
.body {
  position: relative;
  container: file-workspace / inline-size;
  overflow: hidden;
  flex: 1;
  display: flex;
  min-height: 0;
}

/* Central workspace keeps navigation, actions, and file results together. */
.right-area {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
}

.content-row {
  flex: 1;
  display: flex;
  min-width: 0;
  min-height: 0;
  /* Anchor + clip the floating settings rail: it's absolutely positioned and
   *  slides in/out past the right edge, so clip the spill here. The table's own
   *  scroll lives in `.table-wrap`, so this doesn't affect column scrolling. */
  position: relative;
  overflow: hidden;
}

.left-col {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
}

.mono { font-size: 12px; }

.preview-empty {
  flex: 1;
}

</style>
