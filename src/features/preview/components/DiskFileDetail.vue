<template>
  <!-- 文件预览：视图栏（切换视图 + 当前视图的工具）、按需出现的查找栏、内容区、状态栏
       （文件的真实类型、大小、编码与翻页）。各查看器经 PreviewChromeSlot 把自己的工具与
       统计放进视图栏和状态栏，不再各自叠一层工具条。 -->
  <section class="file-preview" @keydown="onPreviewKeydown">
    <PreviewViewBar
      ref="viewBar"
      v-model:active-tab="activeTab"
      v-model:charset-menu-open="charsetMenuOpen"
      :index-only="indexOnly"
      :file-content="fileContent"
      :focus-target="!!focusTarget"
      :category="category"
      :searchable="searchable"
      :find-visible="findVisible"
      :find-bar-id="findBarId"
      :selected-charsets="selectedCharsets"
      :qr-scanning="qrScanning"
      :ocr-scanning="ocrScanning"
      @toggle-find="toggleFind"
      @toggle-charset="toggleCharset"
      @qr-scan="runQrScan"
      @ocr-scan="runOcrScan"
    />
    <UiTransition kind="collapse">
      <PreviewFindBar
        v-if="findVisible"
        :id="findBarId"
        ref="findBar"
        v-model:query="searchQuery"
        :match-count="matchCount"
        :current-match="currentMatch"
        :page-only="searchesSqlitePage"
        @goto="gotoMatch"
        @close="closeFind"
        @keydown="onSearchKeydown"
      />
    </UiTransition>

    <div ref="previewBody" v-content-motion="node.id + ':' + activeTab" class="preview-body" tabindex="-1">
      <FileMetadataPanel v-if="activeTab === 'detail'" :node="node" :full-path="fullPath" :extension="ext"
        :ident="effectiveIdent" :digest="digest" :ext-mismatch="extMismatch" />
      <template v-else>
        <PreviewState v-if="indexOnly" :icon="Unplug" :title="tr('源镜像不可用')" />
        <PreviewState v-else-if="node.kind !== 'file'" :icon="FolderOpen" :title="tr('没有内容可预览')" />
        <FileRangePreview v-else-if="activeTab === 'hex' && focusTarget" class="viewer-fill" :image-id="imageId" :node-id="node.id" :size="node.size" :target="focusTarget" />
        <PreviewState v-else-if="loading" tone="loading" :title="tr('加载中…')" />
        <PreviewState v-else-if="error" tone="error" :icon="CircleAlert" :title="tr('读取文件失败')" :description="error" mono-description>
          <button type="button" class="ui-btn sm" @click="loadPreview">{{ tr("重试") }}</button>
        </PreviewState>
        <PreviewState v-else-if="node.size === 0" :icon="FileX" :title="tr('空文件')" />

        <!-- 原类型视图：按文件原生类型渲染 -->
        <template v-else-if="activeTab === 'raw'">
          <div v-if="category === 'markdown'" :ref="previewSearch.mdEl" class="md-body document"></div>

          <StructuredViewer v-else-if="category === 'structured'" :key="node.id" class="viewer-fill"
            :text="decodedText" :format="structuredFormat" :query="searchQuery" />

          <CsvViewer v-else-if="category === 'csv'" :ref="previewSearch.csvViewerRef" :key="node.id" class="viewer-fill"
            :image-id="imageId" :node-id="node.id" :delimiter="csvDelimiter" :query="searchQuery" @matches="csvMatchCount = $event" />

          <CodeViewer v-else-if="category === 'code'" :ref="previewSearch.codeViewerRef" class="viewer-fill"
            :code="decodedText" :filename="node.name" :dark="dark" :query="searchQuery" />

          <BinaryViewer v-else-if="category === 'binary'" :key="node.id" class="viewer-fill" :image-id="imageId" :node-id="node.id" />

          <GgufViewer v-else-if="category === 'gguf'" :key="node.id" class="viewer-fill" :image-id="imageId" :node-id="node.id" />

          <SqliteViewer v-else-if="category === 'sqlite'" :key="node.id" :ref="previewSearch.sqliteViewerRef" class="viewer-fill"
            :image-id="imageId" :node-id="node.id" :parent-id="node.parent_id" :query="searchQuery" @reset-match="resetMatchNav" />

          <RealmViewer v-else-if="category === 'realm'" :key="node.id" class="viewer-fill" :image-id="imageId" :node-id="node.id" />

          <KdbxViewer v-else-if="category === 'kdbx'" :key="node.id" class="viewer-fill" :image-id="imageId" :node-id="node.id" :parent-id="node.parent_id" />

          <VimSwapViewer v-else-if="category === 'vimswap'" :key="node.id" class="viewer-fill" :image-id="imageId" :node-id="node.id" :dark="dark" />

          <!-- 文档 / 办公 / 邮件 / 非 zip 压缩包 / CAD / 3D / 图表等「rich」类别统一交给
               Flyfish File Viewer（RichPreviewViewer），库内按扩展名分派渲染器。 -->
          <RichPreviewViewer v-else-if="category === 'rich'" :key="node.id" class="viewer-fill" :image-id="imageId" :node="node" />

          <ZipViewer v-else-if="category === 'zip'" :key="node.id" class="viewer-fill" :image-id="imageId" :node-id="node.id" />

          <FileMediaPreview v-else-if="isMediaCategory" :node="node" :category="category" :is-stream-media="isStreamMedia"
            :media-too-large="mediaTooLarge" :media-url="mediaUrl" :media-loading="mediaLoading" :media-error="mediaError"
            :stream-url="streamUrl" :stream-loading="streamLoading" :stream-error="streamError"
            :recognition="recognitionView" :elements="recognitionElements" v-model:qr-expected="qrExpected" v-model:qr-active-index="qrActiveIndex"
            @media-error="onMediaError" @image-load="onImageLoad" @cancel-qr="cancelQrScan" @copy-qr="copyQrText" @copy-ocr="copyAllOcrText"
          />

          <PreviewState v-else :icon="FileQuestionMark" :title="tr('无法按原类型显示')">
            <button type="button" class="ui-btn sm" @click="activeTab = 'text'"><TextAlignStart :size="13" />{{ tr("查看文本") }}</button>
            <button type="button" class="ui-btn sm" @click="activeTab = 'hex'"><Binary :size="13" />{{ tr("查看十六进制") }}</button>
            <button v-if="!effectiveIdent" type="button" class="ui-btn sm" :disabled="identifying" @click="identifyType">
              <Loader2 v-if="identifying" :size="13" class="spin" /><ScanSearch v-else :size="13" />{{ tr("识别真实类型") }}
            </button>
          </PreviewState>
        </template>

        <template v-else-if="activeTab === 'text'">
          <!-- 只读取了文件开头时由状态栏提示。无关键字时按纯文本渲染，且切成块、用 content-visibility 让浏览器只布局可视块——
               否则上百万字符（尤其二进制文件解码后几乎无换行）一次性整体布局会卡死（切到文本视图很卡）。
               仅在搜索时才走单个 <pre v-html>（注入 <mark> 高亮，命中导航需整块在 DOM 里）。 -->
          <pre v-if="searchQuery" :ref="previewSearch.textEl" class="text-view document mono" v-html="highlightedTextHtml"></pre>
          <div v-else :ref="previewSearch.textEl" class="text-view document text-chunks mono">
            <div v-for="(chunk, ci) in textChunks" :key="ci" class="text-chunk">{{ chunk }}</div>
          </div>
        </template>

        <div
          v-else-if="activeTab === 'hex'"
          :ref="previewSearch.hexEl"
          class="hex-view document mono"
          :style="{ '--hex-grid-cols': hexGridCols }"
          @mousedown="onHexMouseDown"
          @contextmenu="onHexContextMenu"
        >
          <div class="hex-row hex-header-row">
            <span class="hex-offset">Offset</span>
            <span class="hex-bytes">{{ hexHeaderBytes }}</span>
            <span v-for="c in selectedCharsetList" :key="c.id" class="hex-text-col" :data-cs="c.id">{{ c.label }}</span>
          </div>
          <div v-for="row in hexRows" :key="row.offset" class="hex-row">
            <span class="hex-offset">{{ row.offset }}</span>
            <span class="hex-bytes"><span
              v-for="b in row.bytes"
              :key="b.off"
              class="hb"
              :data-off="b.off"
              :class="{ m: b.m, cur: b.cur, sel: inSel(b.off) }"
            >{{ b.hex }}</span>{{ row.padStr }}</span>
            <span
              v-for="(col, ci) in row.cols"
              :key="ci"
              class="hex-text-col"
              :data-cs="selectedCharsetList[ci]?.id"
            ><span
              v-for="(c, k) in col"
              :key="k"
              class="hc"
              :data-off="c.off"
              :class="{ m: c.m, cur: c.cur, sel: inSel(c.off) }"
            >{{ c.ch }}</span></span>
          </div>
        </div>
      </template>
    </div>

    <PreviewStatusBar
      v-if="node.kind === 'file'"
      ref="statusBar"
      v-model:encoding="encodingOverride"
      v-model:page="page"
      v-model:page-size="pageSize"
      :index-only="indexOnly"
      :size="node.size"
      :extension="ext"
      :ident="effectiveIdent"
      :identifying="identifying"
      :ident-title="identBtnTitle"
      :ext-mismatch="extMismatch"
      :truncated="prefixTruncated"
      :preview-bytes="bytes.length"
      :encoding-visible="needsDecodedText"
      :encoding-warn="encodingWarn"
      :encoding-title="encodingTitle"
      :encoding-options="encodingOptions"
      :hex-paging="hexPaging"
      :total-pages="totalPages"
      :has-selection="hasSelection"
      :sel-count="selCount"
      :sel-min="selMin"
      :sel-max="selMax"
      @identify="identifyType"
    >
      <template #start>
        <span v-if="imageSize" class="ui-footbar-stat mono">{{ imageSize }}</span>
      </template>
    </PreviewStatusBar>

    <UiContextMenu
      v-model:open="hexMenu.open"
      :x="hexMenu.x"
      :y="hexMenu.y"
      :entries="hexMenuEntries"
      :label="tr('十六进制选区操作')"
      :heading="hexMenuHeading"
      :subheading="hexMenuSubheading"
      :heading-icon="Binary"
    />
  </section>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import FileMetadataPanel from './FileMetadataPanel.vue';
import { vContentMotion } from '../../../composables/uiMotionDirectives';
import { ref, computed, watch, nextTick, defineAsyncComponent, useId } from 'vue';
import type { FsTypeIdent } from '../../../lib/ipc/fs/derived';
import type { FsNode } from '../../../lib/ipc/fs/types';
import { useMediaPreview } from '../composables/useMediaPreview';
import { usePreviewSession } from '../composables/usePreviewSession';
import { usePreviewReadingState } from '../composables/usePreviewReadingState';
import type { FilePreviewReadingState } from '../../../lib/models/filePreviewReadingState';
import { PREVIEW_CAP, MEDIA_CAP, type DetailTab } from '../model/preview';
import { renderMarkdownPreview } from '../composables/useMarkdown';
import { usePreviewSearch } from '../composables/usePreviewSearch';
import { useTheme } from '../../../composables/useTheme';
import { useImageRecognition } from '../composables/useImageRecognition';
import { usePreviewCategory } from '../composables/usePreviewCategory';
import { useTextPreview } from '../composables/useTextPreview';
import { useHexPreview } from '../composables/useHexPreview';
import { providePreviewChrome } from '../composables/previewChrome';
import { CHARSETS, type CharsetOption } from '../model/charsets';
import {
  Binary, CircleAlert, FileQuestionMark, FileX, FolderOpen, Hash, Languages,
  Loader2, ScanSearch, TextAlignStart, Type, Unplug, X,
} from 'lucide-vue-next';
import UiContextMenu, { type UiContextMenuEntry } from '../../../components/ui/UiContextMenu.vue';
import UiTransition from '../../../components/ui/UiTransition.vue';
import { useContextMenu } from '../../../composables/useContextMenu';
import { compactMenuEntries } from '../../../utils/menuPlacement';
import PreviewViewBar from './PreviewViewBar.vue';
import PreviewFindBar from './PreviewFindBar.vue';
import PreviewStatusBar from './PreviewStatusBar.vue';
import PreviewState from './PreviewState.vue';
import FileMediaPreview from './FileMediaPreview.vue';
import FileRangePreview from './viewers/FileRangePreview.vue';
import type { UiSelectOption } from '../../../components/ui/UiSelect.vue';
import type { ImageId, NodeId } from '../../../lib/ipc/ids';
// 重型原类型视图懒加载：CodeMirror 语言包、Flyfish File Viewer（rich）等只在打开对应
// 文件时拉取，不拖累文件列表/详情面板的常用路径。
const CodeViewer = defineAsyncComponent(() => import('./viewers/CodeViewer.vue'));
const StructuredViewer = defineAsyncComponent(() => import('./viewers/StructuredViewer.vue'));
const CsvViewer = defineAsyncComponent(() => import('./viewers/CsvViewer.vue'));
const BinaryViewer = defineAsyncComponent(() => import('./viewers/BinaryViewer.vue'));
const GgufViewer = defineAsyncComponent(() => import('./viewers/GgufViewer.vue'));
const SqliteViewer = defineAsyncComponent(() => import('./viewers/SqliteViewer.vue'));
const RealmViewer = defineAsyncComponent(() => import('./viewers/RealmViewer.vue'));
const KdbxViewer = defineAsyncComponent(() => import('./viewers/KdbxViewer.vue'));
const VimSwapViewer = defineAsyncComponent(() => import('./viewers/VimSwapViewer.vue'));
const ZipViewer = defineAsyncComponent(() => import('./viewers/ZipViewer.vue'));
const RichPreviewViewer = defineAsyncComponent(() => import('./viewers/RichPreviewViewer.vue'));

const props = defineProps<{
  indexOnly?: boolean;
  imageId: ImageId;
  node: FsNode;
  fullPath: string;
  /** 已计算的内容哈希（来自文件列表的 digestCache）；按算法可缺省——只算过
   *  MD5 就只显示 MD5 行。整组缺省时哈希行不渲染——不显示「—」以免被解读
   *  成「该文件无哈希」。 */
  digest?: { md5?: string | null; sha1?: string | null; sha256?: string | null; sm3?: string | null } | null;
  /** Magika 按内容识别的真实类型（来自列表 ftypeCache）。驱动「原类型视图」
   *  按真实类型而非扩展名渲染——可看穿改后缀伪装；缺省时退回扩展名判定。
   *  状态栏也可就地识别填充（见 identifyType）。 */
  ident?: FsTypeIdent | null;
  focusTarget?: { offset: number; length: number };
  readingState?: FilePreviewReadingState;
}>();

const emit = defineEmits<{
  /** 就地识别成功后上报，父组件据此回灌列表「识别类型」列缓存。 */
  (e: 'identified', payload: { nodeId: NodeId; ident: FsTypeIdent }): void;
}>();

const activeTab = ref<DetailTab>('detail');
const searchQuery = ref('');
const page = ref(1);
const pageSize = ref(2048);
const previewBody = ref<HTMLElement | null>(null);

// 视图栏与状态栏是本实例的「外框」：查看器把工具和统计放进来（保持挂载的隐藏标签页
// 各有自己的一份，不会画到当前标签页上）。
const viewBar = ref<InstanceType<typeof PreviewViewBar> | null>(null);
const statusBar = ref<InstanceType<typeof PreviewStatusBar> | null>(null);
const findBar = ref<InstanceType<typeof PreviewFindBar> | null>(null);
providePreviewChrome({
  tools: computed(() => viewBar.value?.tools ?? null),
  statusStart: computed(() => statusBar.value?.start ?? null),
  statusEnd: computed(() => statusBar.value?.end ?? null),
});

const { loading, error, bytes, localIdent, identifying, loadPreview, identifyType } = usePreviewSession({
  indexOnly: () => props.indexOnly === true,
  imageId: () => props.imageId, node: () => props.node, activeTab, page,
  category: () => category.value,
  resetForNode: () => {
    resetMedia(); resetImageRecognition();
    resetTextPreview(); clearHexSelection(); resetMatchNav();
  },
  resetMedia: () => resetMedia(),
  loadNativeMedia: () => { void loadMedia(); void loadStreamMedia(); },
  ensureDecodedText: () => ensureDecodedText(),
  identified: payload => emit('identified', payload),
});

const { currentTheme } = useTheme();
const dark = computed(() => currentTheme.value === 'dark');

const ext = computed(() => {
  if (props.node.kind === 'dir') return '';
  const i = props.node.name.lastIndexOf('.');
  return i > 0 && i < props.node.name.length - 1 ? props.node.name.slice(i + 1) : '';
});

/** 有内容可读的文件：目录与离线索引只有「详细信息」。 */
const fileContent = computed(() => props.node.kind === 'file' && !props.indexOnly);

// ---- Magika 识别结果驱动的类别 ------------------------------------------
// 识别结果可能来自列表（prop.ident）或本面板就地识别（localIdent）。
const effectiveIdent = computed<FsTypeIdent | null>(() => props.ident ?? localIdent.value);
const {
  category,
  structuredFormat,
  csvDelimiter,
  extMismatch,
  isMediaCategory,
  isBlobMedia,
  isStreamMedia,
  mediaTooLarge,
  effectiveMime,
} = usePreviewCategory({
  nodeKind: () => props.node.kind,
  nodeSize: () => props.node.size,
  ext: () => ext.value,
  bytes: () => bytes.value,
  effectiveIdent: () => effectiveIdent.value,
  mediaCap: MEDIA_CAP,
});

const { view: recognitionView, elements: recognitionElements, qrScanning, qrExpected, qrActiveIndex, ocrScanning,
  onImageLoad, resetImageRecognition, runQrScan, cancelQrScan, copyQrText, runOcrScan, copyAllOcrText,
} = useImageRecognition({ imageId: () => props.imageId, nodeId: () => props.node.id, category: () => category.value });

// Whether the loaded preview is a truncated prefix of the real file.
const truncated = computed(() => props.node.kind === 'file' && props.node.size > bytes.value.length);

const renderedMarkdown = computed(() => renderMarkdownPreview(decodedText.value));

// 状态栏「真实类型」的提示：未识别时说明用途，识别后给出类型、依据与伪装判断。
const identBtnTitle = computed<string>(() => {
  const t = effectiveIdent.value;
  if (!t) return tr("Magika 按文件内容识别真实类型，不依赖扩展名，可发现修改扩展名伪装的文件");
  const parts = [`${t.label}（${t.mime_type}）`, t.description];
  parts.push(t.inferred ? tr("模型置信度 {0}%", { 0: (t.score * 100).toFixed(1) }) : tr("规则判定"));
  if (extMismatch.value) parts.push(tr("扩展名 .{0} 与识别类型不符，疑似修改扩展名伪装", { 0: ext.value }));
  return parts.join(' · ');
});

const {
  decodingSettled,
  decodedText,
  encodingOverride,
  needsDecodedText,
  autoEncodingLabel,
  encodingWarn,
  encodingTitle,
  ensureDecodedText,
  highlightedTextHtml,
  textChunks,
  resetTextPreview,
} = useTextPreview({
  imageId: () => props.imageId,
  nodeId: () => props.node.id,
  nodeKind: () => props.node.kind,
  activeTab,
  category,
  bytes,
  searchQuery,
  previewCap: PREVIEW_CAP,
});
const encodingOptions = computed<UiSelectOption<string>[]>(() => [
  { value: '', label: autoEncodingLabel.value },
  ...CHARSETS.map(charset => ({ value: charset.decoder, label: charset.label })),
]);

const totalPages = computed(() => Math.max(1, Math.ceil(bytes.value.length / pageSize.value)));
const {
  currentHexStart,
  hexHeaderBytes,
  hexMatch,
  selectedCharsets,
  charsetMenuOpen,
  selectedCharsetList,
  toggleCharset,
  hexGridCols,
  hexRows,
  selMin,
  selMax,
  hasSelection,
  selCount,
  inSel,
  onHexMouseDown,
  clearHexSelection,
  prepareHexContextMenu,
  copySelectionHex,
  copySelectionText,
  copySelectionOffset,
} = useHexPreview({
  bytes,
  page,
  pageSize,
  searchQuery,
});

// 十六进制视图的右键菜单：右键处的字节不在选区内时先选中它，菜单作用于选区。
const { menu: hexMenu, openFromEvent: openHexMenu } = useContextMenu<{ charset: CharsetOption | null }>();
function onHexContextMenu(event: MouseEvent) {
  event.preventDefault();
  const target = prepareHexContextMenu(event);
  if (target) openHexMenu(event, target);
}
const hexOffset = (offset: number) => `0x${offset.toString(16).toUpperCase()}`;
const hexMenuHeading = computed(() => tr('已选 {0} 字节', { 0: selCount.value.toLocaleString() }));
const hexMenuSubheading = computed(() => (selCount.value > 1 ? `${hexOffset(selMin.value)} – ${hexOffset(selMax.value)}` : hexOffset(selMin.value)));
const hexMenuEntries = computed<UiContextMenuEntry[]>(() => {
  const here = hexMenu.target?.charset ?? null;
  const charsets = selectedCharsetList.value;
  return compactMenuEntries<UiContextMenuEntry>([
    { id: 'copy-hex', label: tr('复制十六进制'), icon: Binary, action: copySelectionHex },
    ...(here ? [{ id: 'copy-here', label: tr('复制文本（{0}）', { 0: here.label }), icon: Type, action: () => copySelectionText(here) }] : []),
    ...(charsets.length ? [{
      kind: 'submenu' as const, id: 'copy-text', label: tr('按编码复制文本'), icon: Languages,
      items: charsets.map(charset => ({ id: `copy-text:${charset.id}`, label: charset.label, action: () => copySelectionText(charset) })),
    }] : []),
    { id: 'copy-offset', label: tr('复制起始偏移'), icon: Hash, action: copySelectionOffset },
    { kind: 'separator' },
    { id: 'clear', label: tr('取消选择'), icon: X, action: clearHexSelection },
  ]);
});

const previewSearch = usePreviewSearch({
  searchQuery, activeTab, category, decodedText, renderedMarkdown, page, pageSize, currentHexStart, hexMatch,
});
const { currentMatch, matchCount, csvMatchCount, sqliteViewerRef, gotoMatch, onSearchKeydown, resetMatchNav } = previewSearch;

// ---- 查找 ---------------------------------------------------------------
// 只在有可查找文本的视图里提供；查找栏按查找开关 / Ctrl+F 打开，有查找词时一直显示。
const findBarId = `preview-find-${useId()}`;
const findOpen = ref(false);
const hasContent = computed(() => fileContent.value && props.node.size > 0);
const searchesSqlitePage = computed(() => activeTab.value === 'raw' && category.value === 'sqlite');
const searchable = computed(() => {
  if (!hasContent.value) return false;
  switch (activeTab.value) {
    case 'detail': return false;
    case 'hex': return !props.focusTarget;
    case 'text': return true;
    case 'raw':
      if (category.value === 'sqlite') return sqliteViewerRef.value?.matchCount != null;
      return category.value === 'markdown' || category.value === 'code' || category.value === 'structured' || category.value === 'csv';
    default: return false;
  }
});
const findVisible = computed(() => searchable.value && (findOpen.value || searchQuery.value !== ''));

function openFind() {
  if (!searchable.value) return;
  findOpen.value = true;
  void nextTick(() => findBar.value?.focus());
}

function closeFind() {
  const root = findBar.value?.$el as HTMLElement | undefined;
  const hadFocus = !!root && root.contains(document.activeElement);
  findOpen.value = false;
  searchQuery.value = '';
  if (hadFocus) void nextTick(() => viewBar.value?.focusFindToggle());
}

function toggleFind() {
  if (findVisible.value) closeFind();
  else openFind();
}

/** Ctrl+F opens the find bar of the preview that has focus. */
function onPreviewKeydown(event: KeyboardEvent) {
  if (!(event.ctrlKey || event.metaKey) || event.altKey || event.key.toLowerCase() !== 'f' || !searchable.value) return;
  event.preventDefault();
  event.stopPropagation();
  openFind();
}

// ---- 状态栏 -------------------------------------------------------------
/** 读取文件开头一段的视图（文本、十六进制与文本类原类型视图）。 */
const usesPrefix = computed(() => activeTab.value === 'text'
  || (activeTab.value === 'hex' && !props.focusTarget)
  || (activeTab.value === 'raw' && (category.value === 'markdown' || category.value === 'code' || category.value === 'structured')));
const prefixTruncated = computed(() => fileContent.value && !loading.value && !error.value && bytes.value.length > 0 && truncated.value && usesPrefix.value);
const hexPaging = computed(() => activeTab.value === 'hex' && !props.focusTarget && hasContent.value && !loading.value && !error.value);
const imageSize = computed(() => {
  const natural = recognitionView.value.imageNatural;
  return activeTab.value === 'raw' && category.value === 'image' && natural ? `${natural.w} × ${natural.h} px` : '';
});

const { mediaUrl, mediaLoading, mediaError, streamUrl, streamLoading, streamError,
  loadMedia, loadStreamMedia, resetMedia, onMediaError } = useMediaPreview({
  imageId: () => props.imageId, nodeId: () => props.node.id, nodeSize: () => props.node.size,
  category: () => category.value, mime: () => effectiveMime.value, extension: () => ext.value,
  isBlobMedia: () => isBlobMedia.value, isStreamMedia: () => isStreamMedia.value,
});

// Lazy-load media the first time the user lands on the native view (covers
// switching tabs after the initial load, and category flips from the text
// sniff). Switching views also resets keyword navigation.
watch(activeTab, (t) => {
  if (t === 'raw' && !props.indexOnly) {
    loadMedia();
    void loadStreamMedia();
  }
});

// 进入文本类视图、或识别异步翻转 category 后，按需拉取后端解码文本。loadPreview 末尾亦
// 显式调用一次；二者经 ensureDecodedText 内的去重（nodeId 命中即返回），重复触发无害。
watch(needsDecodedText, (need) => {
  if (need && !props.indexOnly) void ensureDecodedText();
});

watch(
  () => [props.imageId, props.node.id, props.indexOnly],
  () => loadPreview(),
  { immediate: true },
);
watch(() => props.focusTarget, target => {
  if (target && !props.indexOnly) activeTab.value = 'hex';
}, { immediate: true });

const { getReadingState } = usePreviewReadingState({
  imageId: () => props.imageId, nodeId: () => props.node.id,
  initial: () => props.indexOnly ? (props.readingState?.tab === 'detail' ? props.readingState : undefined) : props.readingState && props.focusTarget ? { ...props.readingState, tab: 'hex' } : props.readingState,
  body: previewBody, tab: activeTab, searchQuery, encoding: encodingOverride,
  page, pageSize, currentMatch, matchCount, totalPages,
  ready: () => !loading.value && (!error.value || props.indexOnly === true) &&
    (!needsDecodedText.value || props.node.size === 0 || decodingSettled.value) &&
    (activeTab.value !== 'raw' || category.value !== 'code' || !!previewSearch.codeViewerRef.value),
  codeView: () => previewSearch.codeViewerRef.value,
  restoreMatch: previewSearch.restoreMatch,
});
defineExpose({ getReadingState });
</script>

<style scoped src="../styles/markdown-preview.css"></style>
<style scoped>
.file-preview {
  position: relative;
  display: flex;
  flex: 1;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  height: 100%;
  container: file-preview / inline-size;
  background: var(--surface-secondary);
  color: var(--text-primary);
  font-family: var(--font-ui), sans-serif;
}

.mono {
  font-family: var(--font-mono);
  font-size: 12px;
}

/* 内容区：文档类视图（文本、Markdown、十六进制）随内容增高、由这里滚动；自管布局的
   查看器（viewer-fill）撑满并自己滚动。点进内容后它取得焦点，Ctrl+F 就能打开查找。 */
.preview-body {
  display: flex;
  flex: 1;
  flex-direction: column;
  min-height: 0;
  overflow: auto;
  background: var(--surface-secondary);
  font-size: 12px;
  outline: none;
}

.viewer-fill {
  flex: 1;
  min-height: 0;
}

.document {
  flex-shrink: 0;
  padding: 12px 16px;
}

/* Keyword-match highlight for text / hex / markdown views (CodeMirror uses
   its own .cm-kw-match). No horizontal padding — it would break the hex
   column alignment. The focused match gets the accent color. */
.preview-body :deep(mark) {
  background: var(--sev-warn-bg-hover);
  color: inherit;
  border-radius: 2px;
}

.preview-body :deep(mark.current) {
  background: var(--primary-solid);
  color: var(--accent-contrast);
}

.text-view {
  margin: 0;
  white-space: pre-wrap;
  word-break: break-all;
  color: var(--text-primary);
  line-height: 1.6;
}

/* 分块文本容器：每块用 content-visibility 让浏览器跳过视口外块的布局/绘制，切到文本视图与
   滚动都不再卡。contain-intrinsic-size 给视口外块一个占位高度（auto 记住实际渲染高度），
   滚动条与定位大致准确。white-space / word-break 由 .text-view 继承到各块。 */
.text-chunk {
  content-visibility: auto;
  contain-intrinsic-size: auto 700px;
}

.hex-view {
  line-height: 1.6;
  /* 撑满预览区宽度，让文本列的 1fr 按列数均分、铺满不留空；列多到放不下时各列
     退到 18ch 下限并溢出，由 .preview-body 横向滚动，所有行随之一同平移。 */
  width: 100%;
  min-width: max-content;
  /* 永久禁用原生文本选择：十六进制视图有自己的字节级拖选，原生选择会与之叠加
     （还会选中偏移列）。须常开——只在拖动时设会因 class 异步生效而来不及。 */
  user-select: none;
  -webkit-user-select: none;
}

/* 表头吸顶：负外边距铺满内边距，滚动时只露出列名一行。 */
.hex-header-row {
  position: sticky;
  top: -12px;
  z-index: 1;
  width: auto;
  margin: -12px -16px 4px;
  padding: 12px 16px 4px;
  border-bottom: 1px solid var(--border-color);
  background: var(--surface-secondary);
  color: var(--text-secondary);
}

.hex-row {
  display: grid;
  /* 列数随所选字符集变化，由 .hex-view 的内联 --hex-grid-cols 提供（自定义
     属性可继承）；兜底为「偏移 + 字节 + 四列」。 */
  grid-template-columns: var(--hex-grid-cols, 56px minmax(360px, max-content) repeat(4, minmax(19ch, 1fr)));
  gap: 12px;
  width: 100%;
  color: var(--text-primary);
}

.hex-offset {
  color: var(--text-secondary);
}

.hex-bytes {
  white-space: pre;
  /* 偏移与字节区之间的分隔线，与各字符集列的分隔线一致，形成整齐栅格。 */
  border-left: 1px solid var(--border-color);
  padding-left: 12px;
}

/* 多编码文本列：ANSI/ASCII · UTF-8 · UTF-16(LE) · UTF-16 BE，逐行 16 字节
   解码、定宽对齐；相邻列以细分隔线区分，超出列宽按列裁切。 */
.hex-text-col {
  color: var(--text-secondary);
  white-space: pre;
  overflow: hidden;
  border-left: 1px solid var(--border-color);
  padding-left: 12px;
}

/* 单个字节 / 字符单元：默认无底色，按命中（搜索）/ 当前命中 / 选区着色。多态可
   叠加，靠源码顺序定优先级：选区 > 命中，当前命中最高。 */
.hb,
.hc {
  border-radius: 2px;
}

.hb.m,
.hc.m {
  background: var(--sev-warn-bg-hover);
}

.hb.sel,
.hc.sel {
  /* 半透明强调色铺底——拖选区在字节列与各字符集列同步出现（允许的半透明叠层）。 */
  background: color-mix(in srgb, var(--primary-color) 26%, transparent);
}

.hb.cur,
.hc.cur {
  background: var(--primary-solid);
  color: var(--accent-contrast);
}
</style>
