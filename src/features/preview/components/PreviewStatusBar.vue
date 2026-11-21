<template>
  <!-- 状态栏：左侧是文件本身的事实（真实类型、大小、离线状态）与查看器的统计，右侧是
       当前视图的阅读设置（文本编码）与翻页。查看器经 PreviewChromeSlot 填入两端。 -->
  <footer class="ui-footbar preview-statusbar">
    <div ref="startEl" class="status-group status-start">
      <span v-if="indexOnly" class="ui-badge sm info" :title="tr('属性来自已有索引，正文预览需恢复源镜像')">{{ tr("离线索引") }}</span>
      <button
        v-else
        type="button"
        class="status-chip type-chip"
        :class="{ identified: !!ident, mismatch: extMismatch }"
        :disabled="identifying"
        :title="identTitle"
        @click="emit('identify')"
      >
        <Loader2 v-if="identifying" :size="12" class="spin" />
        <AlertTriangle v-else-if="extMismatch" :size="12" />
        <ScanSearch v-else :size="12" />
        <span v-if="ident" class="mono">{{ ident.label }}</span>
        <span v-else>{{ identifying ? tr("识别中…") : tr("识别真实类型") }}</span>
      </button>
      <span v-if="extMismatch && ident" class="ui-badge sm warn" :title="tr('扩展名 .{0} 与识别类型不符，疑似修改扩展名伪装', { 0: extension })">{{ tr("扩展名不符") }}</span>
      <span class="ui-footbar-stat" :title="tr('文件大小 {0} 字节', { 0: size.toLocaleString() })"><strong>{{ formatBytes(size) }}</strong></span>
      <span v-if="truncated" class="ui-footbar-stat truncated" :title="tr('文本与十六进制视图只读取文件开头的 {0}', { 0: formatBytes(previewBytes) })">
        <AlertTriangle :size="12" />{{ tr("仅读取前 {0}", { 0: formatBytes(previewBytes) }) }}
      </span>
      <slot name="start" />
    </div>

    <span class="ui-footbar-spacer" />

    <div ref="endEl" class="status-group status-end">
      <!-- 文本编码：文本 / 代码 / Markdown / 结构化视图回显自动检测的编码，可就地纠正。 -->
      <label v-if="encodingVisible" class="encoding" :title="encodingTitle">
        <AlertTriangle v-if="encodingWarn" :size="12" class="encoding-warn" />
        <span class="sr-only">{{ tr("文本编码") }}</span>
        <UiSelect v-model="encoding" class="sm mono encoding-select" :options="encodingOptions" :aria-label="tr('文本编码')" />
      </label>

      <!-- 十六进制：选区读数 + 分页。 -->
      <template v-if="hexPaging">
        <span v-if="hasSelection" class="ui-footbar-stat sel-info">
          {{ tr("已选") }} <strong>{{ selCount.toLocaleString() }}</strong> {{ tr("字节") }}
          <span class="mono">0x{{ selMin.toString(16).toUpperCase() }}–0x{{ selMax.toString(16).toUpperCase() }}</span>
        </span>
        <div v-if="totalPages > 1" class="hex-pager">
          <UiSelect v-model="pageSize" class="sm mono" :options="PAGE_SIZE_OPTIONS" :aria-label="tr('分页大小')" />
          <button type="button" class="ui-icon-btn sm" :title="tr('上一页')" :aria-label="tr('上一页')" :disabled="page <= 1" @click="page -= 1"><ChevronLeft :size="14" /></button>
          <span class="page-of mono">
            <input v-model.number="page" class="page-input" type="number" min="1" :max="totalPages" :aria-label="tr('当前页')" />
            <span class="page-total">/ {{ totalPages }}</span>
          </span>
          <button type="button" class="ui-icon-btn sm" :title="tr('下一页')" :aria-label="tr('下一页')" :disabled="page >= totalPages" @click="page += 1"><ChevronRight :size="14" /></button>
        </div>
      </template>
    </div>
  </footer>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { ref } from 'vue';
import { AlertTriangle, ChevronLeft, ChevronRight, Loader2, ScanSearch } from 'lucide-vue-next';
import UiSelect, { type UiSelectOption } from '../../../components/ui/UiSelect.vue';
import type { FsTypeIdent } from '../../../lib/ipc/fs/derived';
import { PREVIEW_PAGE_SIZES as PAGE_SIZE_OPTIONS, formatPreviewBytes as formatBytes } from '../model/preview';

defineProps<{
  indexOnly?: boolean;
  size: number;
  extension: string;
  ident: FsTypeIdent | null;
  identifying: boolean;
  identTitle: string;
  extMismatch: boolean;
  /** The text and hex views read only this prefix of a larger file. */
  truncated: boolean;
  previewBytes: number;
  encodingVisible: boolean;
  encodingWarn: boolean;
  encodingTitle: string;
  encodingOptions: UiSelectOption<string>[];
  hexPaging: boolean;
  totalPages: number;
  hasSelection: boolean;
  selCount: number;
  selMin: number;
  selMax: number;
}>();
const encoding = defineModel<string>('encoding', { required: true });
const page = defineModel<number>('page', { required: true });
const pageSize = defineModel<number>('pageSize', { required: true });
const emit = defineEmits<{ identify: [] }>();

const startEl = ref<HTMLElement | null>(null);
const endEl = ref<HTMLElement | null>(null);
defineExpose({ start: startEl, end: endEl });
</script>

<style scoped>
.preview-statusbar {
  gap: 4px 12px;
  padding: 4px 8px 4px 12px;
}

.status-group {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 4px 12px;
  min-width: 0;
}

.status-end {
  gap: 4px 8px;
}

/* 真实类型：未识别时是一个轻量的文字按钮，识别后显示 Magika 标签；扩展名不符时转为警告色。 */
.status-chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 22px;
  margin-left: -6px;
  padding: 0 6px;
  border: 0;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-secondary);
  font: inherit;
  font-size: 11px;
  cursor: pointer;
  transition: background-color var(--transition-fast), color var(--transition-fast);
}

.status-chip:hover:not(:disabled) {
  background: var(--hover-bg);
  color: var(--text-primary);
}

.status-chip:focus-visible {
  outline: 2px solid var(--primary-color);
  outline-offset: -2px;
}

.status-chip:disabled {
  cursor: progress;
}

.status-chip.identified {
  color: var(--text-primary);
}

.status-chip.identified .mono {
  font-weight: 600;
}

.status-chip.mismatch,
.status-chip.mismatch:hover:not(:disabled) {
  color: var(--sev-warn-fg);
}

.truncated {
  align-items: center;
  color: var(--sev-warn-fg);
}

.encoding {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}

.encoding-warn {
  flex-shrink: 0;
  color: var(--sev-warn-fg);
}

.encoding-select {
  height: 24px;
  border-color: transparent;
  background-color: transparent;
  font-size: 11px;
}

.encoding-select:hover {
  background-color: var(--hover-bg);
}

.sel-info .mono {
  margin-left: 4px;
}

.hex-pager {
  display: flex;
  align-items: center;
  gap: 4px;
}

.hex-pager :deep(.ui-select) {
  height: 24px;
  font-size: 11px;
}

.page-of {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 11px;
}

.page-input {
  width: 48px;
  height: 22px;
  padding: 0 6px;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-sm);
  background: var(--surface-secondary);
  color: var(--text-primary);
  font: inherit;
  text-align: right;
  outline: none;
}

.page-input:focus-visible {
  border-color: var(--primary-color);
  box-shadow: 0 0 0 3px var(--primary-glow);
}

.page-total {
  color: var(--text-secondary);
}
</style>
