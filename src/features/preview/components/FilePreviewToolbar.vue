<template>
  <!-- 页签与窗口动作占一行，预览工具另起一行，避免窄面板中换行后仍整体右对齐。 -->
  <div class="preview-bar">
    <div class="ui-tabs tabs" role="tablist">
      <button
        v-for="t in tabs"
        :key="t.key"
        class="ui-tab detail-tab"
        role="tab"
        :aria-selected="activeTab === t.key"
        :disabled="indexOnly && t.key !== 'detail'"
        :title="indexOnly && t.key !== 'detail' ? tr('正文预览需恢复源镜像') : undefined"
        :class="{ active: activeTab === t.key }"
        @click="activeTab = t.key"
      >{{ tr(t.label) }}</button>
    </div>
    <span v-if="indexOnly" class="ui-badge sm info" :title="tr('属性来自已有索引，正文预览需恢复源镜像')">{{ tr("离线索引") }}</span>
    <div v-if="!standalone" class="window-actions">
      <button class="ui-icon-btn sm" :title="tr('在独立窗口打开')" :aria-label="tr('在独立窗口打开')" @click="emit('external')">
        <ExternalLink :size="14" />
      </button>
      <button class="ui-icon-btn sm" :title="tr('关闭')" :aria-label="tr('关闭预览')" @click="emit('close')">
        <X :size="14" />
      </button>
    </div>
  </div>
  <div v-if="nodeKind === 'file' && !indexOnly" class="ui-toolbar preview-toolbar">
    <div class="preview-controls">
      <!-- 字符集选择器：仅十六进制页出现，勾选要显示的字符集列（跨会话记忆）。 -->
      <div v-if="activeTab === 'hex' && !focusTarget" class="charset-picker">
        <button
          class="ui-btn sm charset-btn"
          :class="{ active: charsetMenuOpen }"
          :title="tr('选择十六进制视图显示的字符集列')"
          @click="charsetMenuOpen = !charsetMenuOpen"
        >
          <Languages :size="13" />
          <span class="charset-btn-label">{{ tr("字符集") }} {{ selectedCharsetCount }}</span>
          <ChevronDown :size="12" />
        </button>
        <template v-if="charsetMenuOpen">
          <div class="charset-backdrop" @click="charsetMenuOpen = false"></div>
          <div class="ui-menu charset-menu">
            <label v-for="c in CHARSETS" :key="c.id" class="ui-check charset-item">
              <input
                type="checkbox"
                :checked="selectedCharsets.includes(c.id)"
                @change="emit('toggleCharset', c.id)"
              />
              <span class="charset-item-label">{{ c.label }}</span>
            </label>
          </div>
        </template>
      </div>
      <!-- 文本编码：文本/代码/Markdown/结构化视图展示自动检测出的编码，并允许就地
           切换（chardetng 检测可能误判，取证场景需可纠正）。十六进制视图有自己的
           字符集列，不在此列。 -->
      <div v-if="needsDecodedText" class="enc-picker" :title="encodingTitle">
        <Languages :size="13" />
        <UiSelect v-model="encodingOverride" class="enc-select mono" :options="encodingOptions" :aria-label="tr('文本编码')" />
        <AlertTriangle v-if="encodingWarn" :size="12" class="enc-warn" />
      </div>
      <!-- 关键字搜索：文本/十六进制及可搜索的原类型子视图才有意义；
           图片/视频/音频等媒体原类型视图无可搜索内容，隐藏搜索框。 -->
      <div v-if="activeTab !== 'detail' && !(activeTab === 'hex' && focusTarget) && !(activeTab === 'raw' && isMediaCategory)" class="search-wrap">
        <div class="ui-search search-field">
          <Search :size="13" aria-hidden="true" />
          <input
            class="mono search-input"
            type="text"
            :placeholder="tr('请输入关键词')"
            :aria-label="tr('搜索预览内容')"
            v-model="searchQuery"
            @keydown="emit('searchKeydown', $event)"
          />
        </div>
        <span v-if="searchQuery" class="match-count mono">
          {{ matchCount ? `${currentMatch >= 0 ? currentMatch + 1 : 0}/${matchCount}` : tr("无匹配") }}
        </span>
        <button class="ui-icon-btn sm bordered" :title="tr('上一个 (Shift+Enter)')" :aria-label="tr('上一个匹配')" :disabled="matchCount === 0" @click="emit('gotoMatch', -1)">
          <ChevronUp :size="14" />
        </button>
        <button class="ui-icon-btn sm bordered" :title="tr('下一个 (Enter)')" :aria-label="tr('下一个匹配')" :disabled="matchCount === 0" @click="emit('gotoMatch', 1)">
          <ChevronDown :size="14" />
        </button>
        <button class="ui-icon-btn sm bordered" :title="tr('清除')" :aria-label="tr('清除搜索')" :disabled="!searchQuery" @click="searchQuery = ''">
          <X :size="14" />
        </button>
      </div>
    </div>
    <div class="ui-toolbar-actions preview-actions">
      <button
        v-if="nodeKind === 'file'"
        class="ui-btn sm ident-btn"
        :class="{ mismatch: extMismatch }"
        :disabled="identifying"
        :title="identBtnTitle"
        @click="emit('identify')"
      >
        <Loader2 v-if="identifying" :size="13" class="spin" />
        <AlertTriangle v-else-if="extMismatch" :size="13" />
        <ScanSearch v-else :size="13" />
        <span v-if="effectiveIdent" class="ident-label mono">{{ effectiveIdent.label }}</span>
        <span v-else class="ident-label">{{ tr("识别真实类型") }}</span>
      </button>
      <!-- 扫描二维码：仅图片的原类型视图出现。WeChat 检测器 + 暴力变换，
           识别结果在图上叠框并列于图下方。 -->
      <button
        v-if="activeTab === 'raw' && category === 'image'"
        class="ui-btn sm ident-btn qr-scan-btn"
        :disabled="qrScanning"
        :title="qrScanning ? tr('正在扫描二维码…') : tr('扫描图中的二维码（WeChat 检测器 + 多种图像变换）')"
        @click="emit('qrScan')"
      >
        <Loader2 v-if="qrScanning" :size="13" class="spin" />
        <QrCode v-else :size="13" />
        <span class="ident-label">{{ qrScanning ? tr("扫描中…") : tr("扫描二维码") }}</span>
      </button>
      <!-- 识别文字（OCR）：仅图片的原类型视图出现。PP-OCRv6 检测+识别，
           识别结果在图上叠框并列于图下方。 -->
      <button
        v-if="activeTab === 'raw' && category === 'image'"
        class="ui-btn sm ident-btn ocr-scan-btn"
        :disabled="ocrScanning"
        :title="ocrScanning ? tr('正在识别文字…') : tr('识别图中的文字（PP-OCRv6 OCR）')"
        @click="emit('ocrScan')"
      >
        <Loader2 v-if="ocrScanning" :size="13" class="spin" />
        <ScanText v-else :size="13" />
        <span class="ident-label">{{ ocrScanning ? tr("识别中…") : tr("识别文字") }}</span>
      </button>
    </div>
  </div>


</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { X, Search, ExternalLink, ChevronUp, ChevronDown, ScanSearch, AlertTriangle, Loader2, Languages, QrCode, ScanText } from 'lucide-vue-next';
import UiSelect, { type UiSelectOption } from '../../../components/ui/UiSelect.vue';
import type { FsTypeIdent } from '../../../lib/ipc/fs/derived';
import { PREVIEW_TABS as tabs, type DetailTab, type PreviewCategory } from '../model/preview';
import { CHARSETS } from '../model/charsets';

defineProps<{
  indexOnly?: boolean;
  standalone?: boolean; nodeKind: string; focusTarget: boolean; needsDecodedText: boolean;
  selectedCharsets: string[]; selectedCharsetCount: number;
  encodingTitle: string; encodingWarn: boolean; encodingOptions: UiSelectOption<string>[];
  matchCount: number; currentMatch: number; isMediaCategory: boolean; category: PreviewCategory;
  extMismatch: boolean; identifying: boolean; identBtnTitle: string; effectiveIdent: FsTypeIdent | null;
  qrScanning: boolean; ocrScanning: boolean;
}>();
const activeTab = defineModel<DetailTab>('activeTab', { required: true });
const searchQuery = defineModel<string>('searchQuery', { required: true });
const encodingOverride = defineModel<string>('encodingOverride', { required: true });
const charsetMenuOpen = defineModel<boolean>('charsetMenuOpen', { required: true });
const emit = defineEmits<{
  close: []; external: []; identify: []; qrScan: []; ocrScan: [];
  toggleCharset: [id: string]; searchKeydown: [event: KeyboardEvent]; gotoMatch: [delta: number];
}>();
</script>

<style scoped>
.mono { font-family: var(--font-mono); font-size: 12px; }
.preview-bar {
  display: flex;
  align-items: center;
  flex-shrink: 0;
  gap: 12px;
  min-height: 40px;
  padding: 0 16px;
  border-bottom: 1px solid var(--border-color);
  background: var(--surface-secondary);
}

.tabs {
  gap: 16px;
  flex: 1;
  min-width: 0;
  overflow-x: auto;
  overflow-y: hidden;
  border-bottom: 0;
}

.tabs .ui-tab {
  height: 40px;
  flex-shrink: 0;
  font-size: 12px;
}

.tabs .ui-tab::after {
  bottom: 0;
}

.window-actions {
  display: flex;
  align-items: center;
  flex-shrink: 0;
  gap: 4px;
}

.preview-toolbar {
  flex-shrink: 0;
  flex-wrap: wrap;
  gap: 8px 16px;
  padding: 8px 16px;
}

.preview-controls {
  display: flex;
  flex: 1 1 320px;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.preview-controls:empty {
  display: none;
}

.preview-actions {
  flex-wrap: wrap;
  max-width: 100%;
  gap: 8px;
}

/* 字符集选择器（十六进制页）：按钮 + 下拉勾选清单。下拉用全屏透明遮罩接管
   外部点击关闭，菜单本身贴按钮左下角弹出。 */
.charset-picker {
  position: relative;
  flex-shrink: 0;
}

.charset-btn-label {
  white-space: nowrap;
}

.charset-backdrop {
  position: fixed;
  inset: 0;
  z-index: 40;
}

.charset-menu {
  position: absolute;
  top: calc(100% + 4px);
  left: 0;
  z-index: 41;
  min-width: 170px;
  max-height: 320px;
  overflow-y: auto;
}

.charset-item {
  display: flex;
  min-height: 30px;
  padding: 0 8px;
  border-radius: var(--radius-sm);
}

.charset-item:hover {
  background: var(--hover-bg);
}

/* 文本编码选择器（文本/代码/Markdown/结构化视图）：Languages 图标 + UiSelect。
   第一项「自动 · <编码>」回显 chardetng 的检测结果，其余项为手动覆盖。had_errors
   时尾随一个告警三角。整体与字符集按钮同高、同色系。 */
.enc-picker {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 26px;
  padding: 0 0 0 8px;
  flex-shrink: 0;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  background: var(--surface-secondary);
  color: var(--text-secondary);
  transition: color var(--transition-fast), border-color var(--transition-fast);
}

.enc-picker:hover {
  color: var(--text-primary);
  border-color: color-mix(in srgb, var(--primary-color) 45%, var(--border-color));
}

/* 外层胶囊就是输入框外观：下拉框去掉自身的边框、底色和聚焦光圈，只保留箭头。 */
.enc-select {
  max-width: 160px;
  height: 22px;
  padding: 0 28px 0 4px;
  border: none;
  background-color: transparent;
  color: inherit;
  font-size: 11px;
}

.enc-select:focus {
  box-shadow: none;
}

.enc-picker .enc-select:focus-visible {
  outline: 1px solid var(--primary-color);
  border-radius: var(--radius-sm);
}

.enc-warn {
  color: var(--sev-warn-fg);
  flex-shrink: 0;
  margin-right: 8px;
}

.charset-item-label {
  font-family: var(--font-mono);
  font-size: 11px;
}

.search-wrap {
  display: flex;
  flex: 1 1 180px;
  align-items: center;
  gap: 4px;
  min-width: 0;
  max-width: 420px;
}

.search-field {
  flex: 1;
  min-width: 80px;
  height: 26px;
}

.search-input {
  width: 100%;
}

.match-count {
  font-size: 11px;
  color: var(--text-secondary);
  white-space: nowrap;
}

/* 「识别真实类型」按钮：图标 + 标签的自适应宽按钮，置于预览栏右侧（外链
   按钮前），内嵌与独立窗均显示。检出伪装时整体转 critical 色。 */
.ident-btn {
  color: var(--text-secondary);
}

.ident-btn.mismatch {
  color: var(--sev-critical-fg);
  border-color: var(--sev-critical-border);
  background: var(--sev-critical-bg);
}

.ident-label {
  white-space: nowrap;
}

.ident-btn .ident-label.mono {
  font-size: 11px;
}

</style>
