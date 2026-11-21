<template>
  <!-- 一行视图栏：左侧切换视图，右侧是当前视图的工具（查看器经 PreviewChromeSlot 放进来的
       控件排在内置工具之后）与查找开关。面板较窄时工具另起一行、左对齐。 -->
  <div class="preview-viewbar">
    <div class="ui-seg view-switch" role="tablist" :aria-label="tr('预览视图')" @keydown="onSwitchKeydown">
      <button
        v-for="tab in PREVIEW_TABS"
        :key="tab.key"
        type="button"
        role="tab"
        class="ui-seg-item view-tab"
        :aria-selected="activeTab === tab.key"
        :tabindex="activeTab === tab.key ? 0 : -1"
        :disabled="tabDisabled(tab.key)"
        :title="tabTitle(tab.key)"
        :data-view="tab.key"
        @click="activeTab = tab.key"
      >{{ tr(tab.label) }}</button>
    </div>

    <div ref="toolsEl" class="viewbar-tools">
      <!-- 字符集列：仅十六进制视图，勾选要显示的字符集列（跨会话记忆）。 -->
      <div v-if="activeTab === 'hex' && fileContent && !focusTarget" class="charset-picker">
        <button
          type="button"
          class="ui-btn sm ghost"
          :aria-expanded="charsetMenuOpen"
          :title="tr('选择十六进制视图显示的字符集列')"
          @click="charsetMenuOpen = !charsetMenuOpen"
        >
          <Languages :size="13" />
          <span>{{ tr("字符集") }}</span>
          <span class="ui-count">{{ selectedCharsets.length }}</span>
          <ChevronDown :size="12" />
        </button>
        <template v-if="charsetMenuOpen">
          <div class="charset-backdrop" @click="charsetMenuOpen = false"></div>
          <div class="ui-menu charset-menu" role="group" :aria-label="tr('字符集')" @keydown.esc.stop="charsetMenuOpen = false">
            <label v-for="charset in CHARSETS" :key="charset.id" class="ui-check charset-item">
              <input type="checkbox" :checked="selectedCharsets.includes(charset.id)" @change="emit('toggleCharset', charset.id)" />
              <span class="mono">{{ charset.label }}</span>
            </label>
          </div>
        </template>
      </div>

      <!-- 图片的原类型视图：扫描二维码（WeChat 检测器 + 多种变换）与识别文字（PP-OCRv6）。 -->
      <template v-if="activeTab === 'raw' && category === 'image' && fileContent">
        <button type="button" class="ui-btn sm" :disabled="qrScanning"
          :title="qrScanning ? tr('正在扫描二维码…') : tr('扫描图中的二维码（WeChat 检测器 + 多种图像变换）')" @click="emit('qrScan')">
          <Loader2 v-if="qrScanning" :size="13" class="spin" />
          <QrCode v-else :size="13" />
          {{ qrScanning ? tr("扫描中…") : tr("扫描二维码") }}
        </button>
        <button type="button" class="ui-btn sm" :disabled="ocrScanning"
          :title="ocrScanning ? tr('正在识别文字…') : tr('识别图中的文字（PP-OCRv6 OCR）')" @click="emit('ocrScan')">
          <Loader2 v-if="ocrScanning" :size="13" class="spin" />
          <ScanText v-else :size="13" />
          {{ ocrScanning ? tr("识别中…") : tr("识别文字") }}
        </button>
      </template>
    </div>

    <button
      v-if="searchable"
      ref="findToggle"
      type="button"
      class="ui-icon-btn find-toggle"
      :aria-pressed="findVisible"
      :aria-controls="findBarId"
      :title="tr('查找 (Ctrl+F)')"
      :aria-label="tr('查找')"
      @click="emit('toggleFind')"
    ><Search :size="16" /></button>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { ref } from 'vue';
import { ChevronDown, Languages, Loader2, QrCode, ScanText, Search } from 'lucide-vue-next';
import { PREVIEW_TABS, type DetailTab, type PreviewCategory } from '../model/preview';
import { CHARSETS } from '../model/charsets';

const props = defineProps<{
  indexOnly?: boolean;
  /** A file with content: the other views stay disabled for directories and offline indexes. */
  fileContent: boolean;
  focusTarget: boolean;
  category: PreviewCategory;
  searchable: boolean;
  findVisible: boolean;
  findBarId: string;
  selectedCharsets: string[];
  qrScanning: boolean;
  ocrScanning: boolean;
}>();
const activeTab = defineModel<DetailTab>('activeTab', { required: true });
const charsetMenuOpen = defineModel<boolean>('charsetMenuOpen', { required: true });
const emit = defineEmits<{ toggleFind: []; toggleCharset: [id: string]; qrScan: []; ocrScan: [] }>();

const toolsEl = ref<HTMLElement | null>(null);
const findToggle = ref<HTMLButtonElement | null>(null);
defineExpose({ tools: toolsEl, focusFindToggle: () => findToggle.value?.focus() });

function tabDisabled(tab: DetailTab) {
  return tab !== 'detail' && !props.fileContent;
}

function tabTitle(tab: DetailTab) {
  if (!tabDisabled(tab)) return undefined;
  return props.indexOnly ? tr('正文预览需恢复源镜像') : tr('只有文件有内容可预览');
}

/** ←/→, Home and End move between the views that are available. */
function onSwitchKeydown(event: KeyboardEvent) {
  const order = PREVIEW_TABS.map(tab => tab.key).filter(tab => !tabDisabled(tab));
  const index = order.indexOf(activeTab.value);
  const next = event.key === 'ArrowLeft' ? order[index - 1] : event.key === 'ArrowRight' ? order[index + 1]
    : event.key === 'Home' ? order[0] : event.key === 'End' ? order[order.length - 1] : undefined;
  if (!next) return;
  event.preventDefault();
  activeTab.value = next;
  const switcher = event.currentTarget as HTMLElement;
  requestAnimationFrame(() => switcher.querySelector<HTMLButtonElement>(`[data-view="${next}"]`)?.focus());
}
</script>

<style scoped>
.preview-viewbar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
  min-height: 44px;
  padding: 8px 12px;
  border-bottom: 1px solid var(--border-color);
  background: var(--surface-secondary);
}

.view-switch {
  flex-shrink: 0;
}

.view-tab {
  padding: 0 10px;
}

.viewbar-tools {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
  min-width: 0;
  margin-left: auto;
}

/* 没有工具时不占位；查找开关在有工具时排在它们之后。 */
.viewbar-tools:not(:has(*)) {
  display: none;
}

.viewbar-tools:not(:has(*)) + .find-toggle {
  margin-left: auto;
}

/* 窄面板：视图切换与查找开关留在第一行，工具另起一行并左对齐。 */
@container file-preview (max-width: 480px) {
  .viewbar-tools {
    order: 3;
    flex-basis: 100%;
    margin-left: 0;
  }

  .find-toggle {
    order: 2;
    margin-left: auto;
  }

  .charset-menu {
    right: auto;
    left: 0;
  }
}

.charset-picker {
  position: relative;
}

.charset-backdrop {
  position: fixed;
  inset: 0;
  z-index: 40;
}

.charset-menu {
  position: absolute;
  top: calc(100% + 4px);
  right: 0;
  z-index: 41;
  min-width: 180px;
  max-height: 320px;
  overflow-y: auto;
}

.charset-item {
  display: flex;
  min-height: 30px;
  padding: 0 10px;
  border-radius: var(--radius-md);
}

.charset-item:hover {
  background: var(--hover-bg);
}

.charset-item .mono {
  font-size: 11px;
}
</style>
