<template>
  <section
    class="disk-file-detail"
    :class="{ standalone }"
    :style="standalone ? undefined : { height: `${panelHeight}px` }"
  >
    <div
      v-if="!standalone"
      class="resize-handle"
      :title="tr('拖动调整面板高度')"
      @pointerdown="onResizeStart"
    ></div>
    <div class="preview-pane">
      <FilePreviewToolbar :standalone="standalone" :node-kind="node.kind" :focus-target="!!focusTarget" :index-only="indexOnly"
        v-model:active-tab="activeTab" v-model:search-query="searchQuery" v-model:encoding-override="encodingOverride"
        v-model:charset-menu-open="charsetMenuOpen" :selected-charsets="selectedCharsets" :selected-charset-count="selectedCharsetList.length"
        :needs-decoded-text="needsDecodedText" :encoding-title="encodingTitle" :encoding-warn="encodingWarn" :encoding-options="encodingOptions"
        :match-count="matchCount" :current-match="currentMatch" :is-media-category="isMediaCategory" :category="category"
        :ext-mismatch="extMismatch" :identifying="identifying" :ident-btn-title="identBtnTitle" :effective-ident="effectiveIdent"
        :qr-scanning="qrScanning" :ocr-scanning="ocrScanning" @close="emit('close')" @external="openExternal"
        @toggle-charset="toggleCharset" @search-keydown="onSearchKeydown" @goto-match="gotoMatch" @identify="identifyType"
        @qr-scan="runQrScan" @ocr-scan="runOcrScan"
      />

      <div ref="previewBody" class="preview-body" v-content-motion="node.id + ':' + activeTab" :class="{ metadata: activeTab === 'detail', flush: (activeTab === 'hex' && !!focusTarget) || (activeTab === 'raw' && (category === 'structured' || category === 'csv' || category === 'code' || category === 'binary' || category === 'gguf' || category === 'sqlite' || category === 'realm' || category === 'rich' || category === 'zip' || isMediaCategory)) }">
        <FileMetadataPanel v-if="activeTab === 'detail'" :node="node" :full-path="fullPath" :extension="ext"
          :ident="effectiveIdent" :digest="digest" :ext-mismatch="extMismatch" />
        <template v-else>
          <div v-if="indexOnly" class="state-row mono">{{ tr("原镜像不可用，仅能查看索引中的文件属性。") }}</div>
          <FileRangePreview v-else-if="activeTab === 'hex' && focusTarget" :image-id="imageId" :node-id="node.id" :size="node.size" :target="focusTarget" />
          <div v-else-if="loading" class="state-row mono">{{ tr("加载中…") }}</div>
          <div v-else-if="error" class="state-row mono error">{{ error }}</div>
          <div v-else-if="node.kind === 'dir'" class="state-row mono">{{ tr("目录没有内容可预览，详见「详细信息」标签页。") }}</div>

          <!-- 原类型视图：按文件原生类型渲染 -->
          <template v-else-if="activeTab === 'raw'">
            <div v-if="category === 'markdown'" :ref="previewSearch.mdEl" class="md-body"></div>

            <StructuredViewer
              v-else-if="category === 'structured'"
              :key="node.id"
              class="structured-view"
              :text="decodedText"
              :format="structuredFormat"
              :query="searchQuery"
            />

            <CsvViewer
              v-else-if="category === 'csv'"
              :ref="previewSearch.csvViewerRef"
              :key="node.id"
              class="csv-view"
              :image-id="imageId"
              :node-id="node.id"
              :delimiter="csvDelimiter"
              :query="searchQuery"
              @matches="csvMatchCount = $event"
            />

            <CodeViewer
              v-else-if="category === 'code'"
              :ref="previewSearch.codeViewerRef"
              class="code-view"
              :code="decodedText"
              :filename="node.name"
              :dark="dark"
              :query="searchQuery"
            />

            <BinaryViewer
              v-else-if="category === 'binary'"
              :key="node.id"
              class="binary-view"
              :image-id="imageId"
              :node-id="node.id"
            />

            <GgufViewer
              v-else-if="category === 'gguf'"
              :key="node.id"
              class="gguf-view"
              :image-id="imageId"
              :node-id="node.id"
            />

            <SqliteViewer
              v-else-if="category === 'sqlite'"
              :key="node.id"
              class="sqlite-view"
              :image-id="imageId"
              :node-id="node.id"
              :parent-id="node.parent_id"
            />

            <RealmViewer
              v-else-if="category === 'realm'"
              :key="node.id"
              class="realm-view"
              :image-id="imageId"
              :node-id="node.id"
            />

            <!-- 文档 / 办公 / 邮件 / 非 zip 压缩包 / CAD / 3D / 图表等「rich」类别统一交给
                 Flyfish File Viewer（RichPreviewViewer），库内按扩展名分派渲染器。 -->
            <RichPreviewViewer
              v-else-if="category === 'rich'"
              :key="node.id"
              class="rich-view"
              :image-id="imageId"
              :node="node"
            />

            <ZipViewer
              v-else-if="category === 'zip'"
              :key="node.id"
              class="zip-view"
              :image-id="imageId"
              :node-id="node.id"
            />

            <FileMediaPreview v-else-if="isMediaCategory" :node="node" :category="category" :is-stream-media="isStreamMedia"
              :media-too-large="mediaTooLarge" :media-url="mediaUrl" :media-loading="mediaLoading" :media-error="mediaError"
              :stream-url="streamUrl" :stream-loading="streamLoading" :stream-error="streamError"
              :recognition="recognitionView" :elements="recognitionElements" v-model:qr-expected="qrExpected" v-model:qr-active-index="qrActiveIndex"
              @media-error="onMediaError" @image-load="onImageLoad" @cancel-qr="cancelQrScan" @copy-qr="copyQrText" @copy-ocr="copyAllOcrText"
            />

            <div v-else class="state-row mono">{{ tr("该类型暂不支持原类型视图，请切换到文本 / 十六进制视图。") }}</div>
          </template>

          <template v-else-if="activeTab === 'text'">
            <div v-if="truncated" class="trunc-note mono">{{ tr("已截断：仅显示前") }} {{ formatBytes(bytes.length) }}{{ tr("（共") }} {{ formatBytes(node.size) }}）</div>
            <!-- 无关键字时按纯文本渲染，且切成块、用 content-visibility 让浏览器只布局可视块——
                 否则上百万字符（尤其二进制文件解码后几乎无换行）一次性整体布局会卡死（切到文本视图很卡）。
                 仅在搜索时才走单个 <pre v-html>（注入 <mark> 高亮，命中导航需整块在 DOM 里）。 -->
            <pre v-if="searchQuery" :ref="previewSearch.textEl" class="text-view mono" v-html="highlightedTextHtml"></pre>
            <div v-else :ref="previewSearch.textEl" class="text-view text-chunks mono">
              <div v-for="(chunk, ci) in textChunks" :key="ci" class="text-chunk">{{ chunk }}</div>
            </div>
          </template>

          <div
            v-else-if="activeTab === 'hex'"
            :ref="previewSearch.hexEl"
            class="hex-view mono"
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

        <FilePreviewFooter v-if="!focusTarget && !loading && !error && activeTab === 'hex' && (totalPages > 1 || hasSelection)"
          v-model:page="page" v-model:page-size="pageSize" :total-pages="totalPages" :has-selection="hasSelection"
          :sel-count="selCount" :sel-min="selMin" :sel-max="selMax" />
      </div>
  </section>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import FileMetadataPanel from './FileMetadataPanel.vue';
import { vContentMotion } from '../../../composables/uiMotionDirectives';
import { ref, computed, watch, defineAsyncComponent } from 'vue';
import type { FsTypeIdent } from '../../../lib/ipc/fs/derived';
import type { FsNode } from '../../../lib/ipc/fs/types';
import { useMediaPreview } from '../composables/useMediaPreview';
import { usePreviewSession } from '../composables/usePreviewSession';
import { usePreviewReadingState } from '../composables/usePreviewReadingState';
import type { FilePreviewReadingState } from '../../../lib/models/filePreviewReadingState';
import { PREVIEW_CAP, MEDIA_CAP, formatPreviewBytes as formatBytes, type DetailTab } from '../model/preview';
import { openFilePreviewWindow } from '../../../lib/ipc/windows';
import { renderMarkdownPreview } from '../composables/useMarkdown';
import { usePreviewSearch } from '../composables/usePreviewSearch';
import { useTheme } from '../../../composables/useTheme';
import { notifyError } from '../../../composables/useToast';
import { useImageRecognition } from '../composables/useImageRecognition';
import { usePreviewCategory } from '../composables/usePreviewCategory';
import { useResizablePanel } from '../composables/useResizablePanel';
import { useTextPreview } from '../composables/useTextPreview';
import { useHexPreview } from '../composables/useHexPreview';
import { CHARSETS } from '../model/charsets';
import FilePreviewToolbar from './FilePreviewToolbar.vue';
import FileMediaPreview from './FileMediaPreview.vue';
import FileRangePreview from './viewers/FileRangePreview.vue';
import type { UiSelectOption } from '../../../components/ui/UiSelect.vue';
import FilePreviewFooter from './FilePreviewFooter.vue';
// 重型原类型视图懒加载：CodeMirror 语言包、Flyfish File Viewer（rich）等只在打开对应
// 文件时拉取，不拖累文件列表/详情面板的常用路径。
const CodeViewer = defineAsyncComponent(() => import('./viewers/CodeViewer.vue'));
const StructuredViewer = defineAsyncComponent(() => import('./viewers/StructuredViewer.vue'));
const CsvViewer = defineAsyncComponent(() => import('./viewers/CsvViewer.vue'));
const BinaryViewer = defineAsyncComponent(() => import('./viewers/BinaryViewer.vue'));
const GgufViewer = defineAsyncComponent(() => import('./viewers/GgufViewer.vue'));
const SqliteViewer = defineAsyncComponent(() => import('./viewers/SqliteViewer.vue'));
const RealmViewer = defineAsyncComponent(() => import('./viewers/RealmViewer.vue'));
const ZipViewer = defineAsyncComponent(() => import('./viewers/ZipViewer.vue'));
const RichPreviewViewer = defineAsyncComponent(() => import('./viewers/RichPreviewViewer.vue'));

const props = defineProps<{
  indexOnly?: boolean;
  imageId: string;
  node: FsNode;
  fullPath: string;
  /** 已计算的内容哈希（来自文件列表的 digestCache）；按算法可缺省——只算过
   *  MD5 就只显示 MD5 行。整组缺省时哈希行不渲染——不显示「—」以免被解读
   *  成「该文件无哈希」。 */
  digest?: { md5?: string | null; sha1?: string | null; sha256?: string | null; sm3?: string | null } | null;
  /** Magika 按内容识别的真实类型（来自列表 ftypeCache）。驱动「原类型视图」
   *  按真实类型而非扩展名渲染——可看穿改后缀伪装；缺省时退回扩展名判定。
   *  详情面板也可就地识别填充（见 identifyType）。 */
  ident?: FsTypeIdent | null;
  /** Rendered inside its own pop-out window — fill the viewport and hide the
   *  "open externally" button (can't pop out further). */
  standalone?: boolean;
  focusTarget?: { offset: number; length: number };
  readingState?: FilePreviewReadingState;
}>();

const emit = defineEmits<{
  (e: 'close'): void;
  /** 详情面板就地识别成功后上报，父组件据此回灌列表「识别类型」列缓存。 */
  (e: 'identified', payload: { nodeId: string; ident: FsTypeIdent }): void;
}>();

const activeTab = ref<DetailTab>('detail');
const searchQuery = ref('');
const page = ref(1);
const pageSize = ref(2048);
const previewBody = ref<HTMLElement | null>(null);

const { panelHeight, onResizeStart } = useResizablePanel({
  storageKey: 'lc.detailPanelHeight',
  defaultHeight: 360,
  minHeight: 180,
  maxViewportRatio: 0.7,
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

// 「识别真实类型」按钮的状态：识别中、是否检出伪装、按钮文案与提示。
const identBtnTitle = computed<string>(() => {
  const t = effectiveIdent.value;
  if (!t) return tr("Magika 按文件内容识别真实类型，与扩展名无关——可发现改后缀伪装");
  const parts = [`${t.label}（${t.mime_type}）`, t.description];
  parts.push(t.inferred ? tr("模型置信度 {0}%", { 0: (t.score * 100).toFixed(1) }) : tr("规则直判"));
  if (extMismatch.value) parts.push(tr("扩展名 .{0} 与识别类型不符，疑似改后缀伪装", { 0: ext.value }));
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
  onHexContextMenu,
} = useHexPreview({
  bytes,
  page,
  pageSize,
  searchQuery,
});

const previewSearch = usePreviewSearch({
  searchQuery, activeTab, category, decodedText, renderedMarkdown, page, pageSize, currentHexStart, hexMatch,
});
const { currentMatch, matchCount, csvMatchCount, gotoMatch, onSearchKeydown, resetMatchNav } = previewSearch;

const { mediaUrl, mediaLoading, mediaError, streamUrl, streamLoading, streamError,
  loadMedia, loadStreamMedia, resetMedia, onMediaError } = useMediaPreview({
  imageId: () => props.imageId, nodeId: () => props.node.id, nodeSize: () => props.node.size,
  category: () => category.value, mime: () => effectiveMime.value, extension: () => ext.value,
  isBlobMedia: () => isBlobMedia.value, isStreamMedia: () => isStreamMedia.value,
});

// Pop the preview out into its own OS window (shares this process's loaded
// image, so the standalone window re-fetches bytes through the fs commands).
async function openExternal() {
  try {
    await openFilePreviewWindow({
      indexOnly: props.indexOnly,
      imageId: props.imageId,
      nodeId: props.node.id,
      fullPath: props.fullPath,
      title: props.node.name,
      node: props.node,
      ident: effectiveIdent.value,
      digest: props.digest,
      focusTarget: props.focusTarget,
      readingState: getReadingState(),
    });
  } catch (e) {
    notifyError(tr("打开独立窗口失败"), String(e));
  }
}

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

<style scoped>
.disk-file-detail {
  position: relative;
  display: flex;
  flex-direction: column;
  border-top: 1px solid var(--border-color);
  background: var(--surface-secondary);
  min-height: 180px;
  flex-shrink: 0;
  color: var(--text-primary);
  font-family: var(--font-ui), sans-serif;
}

/* 顶缘拖拽条：高度由 JS 钳制（180px ~ 70vh），存 localStorage 跨会话记忆。 */
.resize-handle {
  position: absolute;
  top: -3px;
  left: 0;
  right: 0;
  height: 7px;
  cursor: row-resize;
  z-index: 3;
  touch-action: none;
}

.resize-handle:hover {
  background: var(--primary-glow);
}

/* In its own window: fill the available space, no embedded-panel framing. */
.disk-file-detail.standalone {
  height: 100%;
  min-height: 0;
  max-height: none;
  flex: 1;
  border-top: none;
}

.mono {
  font-family: var(--font-mono);
  font-size: 12px;
}

.preview-pane {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
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

.preview-body {
  flex: 1;
  overflow: auto;
  padding: 8px 16px;
  background: var(--surface-secondary);
  font-size: 12px;
}

.preview-body.metadata { padding: 0; }

.state-row {
  padding: 24px;
  text-align: center;
  color: var(--text-secondary);
}

.state-row.error {
  color: var(--sev-critical-fg);
}

/* Code (CodeMirror) and media manage their own layout — drop the padding
   and let them fill the pane. */
.preview-body.flush {
  padding: 0;
  overflow: hidden;
  display: flex;
  flex-direction: column;
}

.code-view {
  flex: 1;
  min-height: 0;
}

/* JSON/YAML/XML 结构节点图视图同理（自管平移/缩放，flush 容器内占满）。 */
.structured-view {
  flex: 1;
  min-height: 0;
}

/* CSV/TSV 表格视图同理（自管滚动，flush 容器内占满）。 */
.csv-view {
  flex: 1;
  min-height: 0;
}

/* 二进制结构视图（ELF / Mach-O / PE / ar）自己滚动（与 code-view 同理，flush 容器内占满）。 */
.binary-view {
  flex: 1;
  min-height: 0;
}

/* GGUF 模型结构视图同理。 */
.gguf-view {
  flex: 1;
  min-height: 0;
}

/* SQLite / Realm 表浏览视图同理。 */
.sqlite-view,
.realm-view {
  flex: 1;
  min-height: 0;
}

/* ZIP 内部结构 + 成员预览视图同理（自管两栏布局与滚动，flush 容器内占满）。 */
.zip-view {
  flex: 1;
  min-height: 0;
}

/* rich 视图（Flyfish File Viewer）同理（自管滚动，flush 容器内占满）。 */
.rich-view {
  flex: 1;
  min-height: 0;
}

/* Rendered Markdown (原类型视图). Output comes from useMarkdown's
   safe-by-construction renderer — same path as the AI panel. */
.md-body {
  color: var(--text-primary);
  font-size: 13px;
  line-height: 1.7;
  word-break: break-word;
}
.md-body :deep(p) { margin: 0 0 8px; }
.md-body :deep(p:last-child) { margin-bottom: 0; }
.md-body :deep(.md-heading) {
  margin: 14px 0 8px;
  line-height: 1.35;
  font-weight: 700;
  color: var(--text-primary);
}
.md-body :deep(.md-h1) { font-size: 18px; border-bottom: 1px solid var(--border-color); padding-bottom: 4px; }
.md-body :deep(.md-h2) { font-size: 16px; border-bottom: 1px solid var(--border-color); padding-bottom: 3px; }
.md-body :deep(.md-h3) { font-size: 15px; }
.md-body :deep(.md-h4) { font-size: 14px; }
.md-body :deep(.md-h5),
.md-body :deep(.md-h6) { font-size: 12px; color: var(--text-secondary); }
.md-body :deep(.md-list) { margin: 6px 0; padding-left: 20px; }
.md-body :deep(.md-list li) { margin: 2px 0; }
.md-body :deep(.md-inline-code) {
  background: var(--surface-tertiary);
  font-family: var(--font-mono);
  font-size: 12px;
  padding: 1px 5px;
  border-radius: var(--radius-sm);
  border: 1px solid var(--border-color);
}
.md-body :deep(.md-code-block) {
  margin: 8px 0;
  padding: 8px 10px;
  border: 1px solid var(--border-color);
  background: var(--bg-secondary);
  border-radius: var(--radius-sm);
  overflow: auto;
  font-family: var(--font-mono);
  font-size: 12px;
  line-height: 1.5;
}
.md-body :deep(.md-code-block code) { font-family: var(--font-mono); }
.md-body :deep(.md-quote) {
  margin: 8px 0;
  padding: 4px 12px;
  border-left: 3px solid var(--border-color);
  color: var(--text-secondary);
}
.md-body :deep(.md-hr) {
  border: none;
  border-top: 1px solid var(--border-color);
  margin: 12px 0;
}
.md-body :deep(.md-table) {
  border-collapse: collapse;
  margin: 8px 0;
  font-size: 12px;
}
.md-body :deep(.md-table th),
.md-body :deep(.md-table td) {
  border: 1px solid var(--border-color);
  padding: 4px 8px;
  text-align: left;
}
.md-body :deep(.md-table th) {
  background: var(--surface-tertiary);
  font-weight: 600;
}
.md-body :deep(a) { color: var(--primary-color); text-decoration: none; }
.md-body :deep(a:hover) { text-decoration: underline; }

.text-view {
  margin: 0;
  white-space: pre-wrap;
  word-break: break-all;
  color: var(--text-primary);
}

/* 分块文本容器：每块用 content-visibility 让浏览器跳过视口外块的布局/绘制，切到文本视图与
   滚动都不再卡。contain-intrinsic-size 给视口外块一个占位高度（auto 记住实际渲染高度），
   滚动条与定位大致准确。white-space / word-break 由 .text-view 继承到各块。 */
.text-chunk {
  content-visibility: auto;
  contain-intrinsic-size: auto 700px;
}

.hex-view {
  line-height: 1.55;
  /* 撑满预览区宽度，让文本列的 1fr 按列数均分、铺满不留空；列多到放不下时各列
     退到 18ch 下限并溢出，由 .preview-body 横向滚动，所有行随之一同平移。 */
  width: 100%;
  /* 永久禁用原生文本选择：十六进制视图有自己的字节级拖选，原生选择会与之叠加
     （还会选中偏移列）。须常开——只在拖动时设会因 class 异步生效而来不及。 */
  user-select: none;
  -webkit-user-select: none;
}

.hex-header-row {
  color: var(--text-secondary);
  margin-bottom: 4px;
  border-bottom: 1px solid var(--border-color);
  padding-bottom: 2px;
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
