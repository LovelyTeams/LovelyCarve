<template>
  <div class="file-category-bar">
    <button
      class="cat-chip all"
      :class="{ active: selected.size === 0 }"
      :title="tr('显示全部类别')"
      @click="$emit('clear')"
    >
      <Files :size="13" />
      <span class="cat-label">{{ tr("全部") }}</span>
      <span v-if="countsComplete !== false" class="cat-count mono">{{ total.toLocaleString() }}</span>
    </button>
    <button
      v-for="c in shownCategories"
      :key="c.key"
      class="cat-chip"
      :class="[`cat-${c.key}`, { active: selected.has(c.key) }]"
      :title="selected.has(c.key) ? tr('取消筛选「{0}」', { 0: tr(c.label) }) : tr('仅显示「{0}」（可多选）', { 0: tr(c.label) })"
      @click="$emit('toggle', c.key)"
    >
      <component :is="CATEGORY_ICON[c.key]" :size="13" class="cat-icon" />
      <span class="cat-label">{{ tr(c.label) }}</span>
      <span v-if="countsComplete !== false" class="cat-count mono">{{ (counts[c.key] ?? 0).toLocaleString() }}</span>
    </button>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

// 文件分类筛选条：把列表里已有的「按扩展名分类」（fileCategory 的 IconCategory）
// 横向铺成可多选的芯片，每个芯片带该类别在当前目录的命中数。选中即按类别收窄
// filteredEntries（多选取并集），不改后端、不读文件头——纯前端按扩展名归类。
// 图标/配色与 FileNodeIcon 的 cat-* 同源，三主题自动同步。
import { computed } from 'vue';
import {
  Database,
  File as FileIcon,
  FileArchive,
  FileAudio,
  FileCode,
  FileCog,
  FileImage,
  Files,
  FileSpreadsheet,
  FileText,
  FileVideo,
  Mail,
} from 'lucide-vue-next';
import { CATEGORY_META, type IconCategory } from '../../../utils/fileCategory';

const props = defineProps<{
  /** 当前目录每个类别的命中数（key 缺省视为 0）。 */
  counts: Record<string, number>;
  /** False while the host has only a page: keep every category selectable. */
  countsComplete?: boolean;
  /** 文件总数（目录/符号链接不计入分类，由调用方决定 total 口径）。 */
  total: number;
  /** 已选类别集合；空集 = 不按类别过滤（「全部」高亮）。 */
  selected: Set<IconCategory>;
}>();

defineEmits<{
  (e: 'toggle', key: IconCategory): void;
  (e: 'clear'): void;
}>();

const CATEGORY_ICON: Record<IconCategory, typeof FileIcon> = {
  image: FileImage,
  video: FileVideo,
  audio: FileAudio,
  pdf: FileText,
  document: FileText,
  spreadsheet: FileSpreadsheet,
  code: FileCode,
  archive: FileArchive,
  executable: FileCog,
  database: Database,
  email: Mail,
  other: FileIcon,
};

// 命中数为 0 的类别不占位（除非它已被选中——选中后清零仍要能取消）。
const shownCategories = computed(() =>
  CATEGORY_META.filter(c => props.countsComplete === false || (props.counts[c.key] ?? 0) > 0 || props.selected.has(c.key)),
);
</script>

<style scoped>
.file-category-bar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
  padding: 8px 12px;
  border-bottom: 1px solid var(--border-color);
  background: var(--surface-primary);
}

.cat-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 26px;
  padding: 0 9px;
  background: var(--surface-secondary);
  border: 1px solid var(--border-color);
  border-radius: 999px;
  color: var(--text-primary);
  font-family: var(--font-ui);
  font-size: 12px;
  cursor: pointer;
  transition: background var(--transition-fast), border-color var(--transition-fast), color var(--transition-fast);
}

.cat-chip:hover {
  border-color: color-mix(in srgb, var(--primary-color) 45%, var(--border-color));
}

.cat-chip.active {
  background: var(--sidebar-active-bg);
  border-color: color-mix(in srgb, var(--primary-color) 60%, var(--border-color));
  color: var(--primary-color);
}

.cat-chip:focus-visible {
  outline: 2px solid var(--primary-color);
  outline-offset: 2px;
}

.cat-label {
  white-space: nowrap;
}

.cat-count {
  font-size: 11px;
  color: var(--text-secondary);
}

.cat-chip.active .cat-count {
  color: var(--primary-color);
}

/* 类别图标配色：与 FileNodeIcon 的 cat-* 同一套 token，三主题自动同步。
   选中态整枚芯片转 accent，图标也跟随 accent 不再单独着色。 */
.cat-icon {
  flex-shrink: 0;
}
.cat-chip.cat-image .cat-icon { color: var(--sev-success-fg); }
.cat-chip.cat-video .cat-icon,
.cat-chip.cat-audio .cat-icon,
.cat-chip.cat-email .cat-icon { color: var(--sev-info-fg); }
.cat-chip.cat-archive .cat-icon,
.cat-chip.cat-database .cat-icon { color: var(--sev-warn-fg); }
.cat-chip.cat-executable .cat-icon { color: var(--sev-critical-fg); }
.cat-chip.cat-code .cat-icon { color: var(--accent-color); }
.cat-chip.cat-pdf .cat-icon,
.cat-chip.cat-document .cat-icon,
.cat-chip.cat-spreadsheet .cat-icon { color: var(--primary-color); }

.cat-chip.active .cat-icon {
  color: var(--primary-color);
}
</style>
