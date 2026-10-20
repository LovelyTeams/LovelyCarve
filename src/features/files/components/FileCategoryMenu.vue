<template>
  <button
    ref="trigger"
    type="button"
    class="ui-btn category-trigger"
    :class="{ active: selected.size > 0 }"
    aria-haspopup="menu"
    :aria-expanded="open"
    :aria-controls="open ? menuId : undefined"
    :disabled="disabled"
    :title="selected.size ? tr('按文件类型筛选：{0}', { 0: selectedLabels }) : tr('按文件类型筛选')"
    @click="toggle"
    @keydown.down.prevent="openMenu"
  >
    <Shapes :size="14" aria-hidden="true" />
    <span class="trigger-label">{{ selected.size === 1 ? selectedLabels : tr("文件类型") }}</span>
    <span v-if="selected.size > 1" class="ui-count accent">{{ selected.size }}</span>
    <ChevronDown :size="14" class="trigger-caret" aria-hidden="true" />
  </button>
  <Teleport to="body">
    <div
      v-if="open"
      :id="menuId"
      ref="menu"
      class="ui-menu category-menu"
      popover="manual"
      role="menu"
      :aria-label="tr('文件类型')"
      @keydown="onMenuKeydown"
    >
      <button class="ui-menu-item category-item" role="menuitemradio" :aria-checked="selected.size === 0" @click="emit('clear')">
        <span class="item-check radio" :class="{ on: selected.size === 0 }" aria-hidden="true"></span>
        <Files :size="14" class="item-icon" aria-hidden="true" />
        <span class="item-label">{{ tr("全部类型") }}</span>
        <span v-if="countsComplete" class="item-count mono">{{ total.toLocaleString() }}</span>
      </button>
      <div class="ui-menu-sep" role="separator"></div>
      <button
        v-for="category in shownCategories"
        :key="category.key"
        class="ui-menu-item category-item"
        role="menuitemcheckbox"
        :aria-checked="selected.has(category.key)"
        @click="emit('toggle', category.key)"
      >
        <span class="item-check" :class="{ on: selected.has(category.key) }" aria-hidden="true"><Check v-if="selected.has(category.key)" :size="11" /></span>
        <component :is="CATEGORY_ICON[category.key]" :size="14" class="item-icon" :class="`cat-${category.key}`" aria-hidden="true" />
        <span class="item-label">{{ tr(category.label) }}</span>
        <span v-if="countsComplete" class="item-count mono">{{ (counts[category.key] ?? 0).toLocaleString() }}</span>
      </button>
      <p v-if="!countsComplete" class="menu-note">{{ tr("目录按需分页加载，暂不统计各类数量") }}</p>
      <p v-else-if="shownCategories.length === 0" class="menu-note">{{ tr("当前列表没有文件") }}</p>
    </div>
  </Teleport>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

// 文件类型筛选：按扩展名归类（fileCategory 的 IconCategory），多选取并集；目录与
// 符号链接不受影响。计数完整时只列出有文件的类别（已选的类别清零后仍保留以便取消），
// 按需分页的目录列出全部类别、不显示数量。
import { computed, nextTick, onBeforeUnmount, onDeactivated, ref, useId, watch } from 'vue';
import {
  Check,
  ChevronDown,
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
  Shapes,
} from 'lucide-vue-next';
import { CATEGORY_META, type IconCategory } from '../../../utils/fileCategory';
import { placeDropdown } from '../../../utils/dropdownPlacement';
import { joinList } from '../../../i18n/translate';

const props = defineProps<{
  /** 当前列表每个类别的文件数（key 缺省视为 0）。 */
  counts: Record<string, number>;
  /** False while the host has only a page: keep every category selectable. */
  countsComplete: boolean;
  /** 文件总数（目录/符号链接不计入分类）。 */
  total: number;
  /** 已选类别；空集 = 不按类别过滤。 */
  selected: Set<IconCategory>;
  disabled?: boolean;
}>();

const emit = defineEmits<{ toggle: [key: IconCategory]; clear: [] }>();

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

const shownCategories = computed(() =>
  CATEGORY_META.filter(c => !props.countsComplete || (props.counts[c.key] ?? 0) > 0 || props.selected.has(c.key)),
);
const selectedLabels = computed(() => joinList(CATEGORY_META.filter(c => props.selected.has(c.key)).map(c => tr(c.label)), tr));

const trigger = ref<HTMLButtonElement | null>(null);
const menu = ref<HTMLDivElement | null>(null);
const open = ref(false);
const menuId = useId();

watch(() => props.disabled, disabled => { if (disabled) dismiss(); });
onDeactivated(dismiss);
onBeforeUnmount(dismiss);

function toggle() {
  if (open.value) close();
  else void openMenu();
}

async function openMenu() {
  if (open.value || props.disabled) return;
  open.value = true;
  await nextTick();
  const element = menu.value;
  if (!open.value || !element || !trigger.value) return;
  // The top layer keeps the menu above the table and outside clipping containers.
  if (typeof element.showPopover === 'function') element.showPopover();
  place();
  (element.querySelector<HTMLElement>('[aria-checked="true"]') ?? element.querySelector<HTMLElement>('.category-item'))?.focus();
  document.addEventListener('pointerdown', onOutsidePointer, true);
  document.addEventListener('scroll', onScroll, { capture: true, passive: true });
  window.addEventListener('resize', dismiss);
  window.addEventListener('blur', dismiss);
}

function close(restoreFocus = false) {
  if (!open.value) return;
  open.value = false;
  document.removeEventListener('pointerdown', onOutsidePointer, true);
  document.removeEventListener('scroll', onScroll, true);
  window.removeEventListener('resize', dismiss);
  window.removeEventListener('blur', dismiss);
  if (restoreFocus) trigger.value?.focus();
}

/** Closes without moving the focus (outside pointer, scroll, resize, blur). */
function dismiss() {
  close();
}

function place() {
  const element = menu.value;
  const anchor = trigger.value;
  if (!element || !anchor) return;
  element.style.maxHeight = 'none';
  const placement = placeDropdown(
    anchor.getBoundingClientRect(),
    { width: element.offsetWidth, height: element.offsetHeight },
    { width: window.innerWidth, height: window.innerHeight },
  );
  element.style.top = `${placement.top}px`;
  element.style.left = `${placement.left}px`;
  element.style.maxHeight = `${placement.maxHeight}px`;
  element.style.setProperty('--ui-pop-offset', placement.above ? '4px' : '-4px');
}

// 勾选会增删类别行（计数完整时），菜单高度变化后重新贴靠触发按钮。
watch(shownCategories, () => { if (open.value) void nextTick(place); });

function onOutsidePointer(event: PointerEvent) {
  const target = event.target;
  if (target instanceof Node && (trigger.value?.contains(target) || menu.value?.contains(target))) return;
  dismiss();
}

function onScroll(event: Event) {
  if (event.target instanceof Node && menu.value?.contains(event.target)) return;
  dismiss();
}

function onMenuKeydown(event: KeyboardEvent) {
  const items = [...(menu.value?.querySelectorAll<HTMLElement>('.category-item') ?? [])];
  const index = items.indexOf(document.activeElement as HTMLElement);
  const focusAt = (next: number) => items[(next + items.length) % items.length]?.focus();
  switch (event.key) {
    case 'ArrowDown': focusAt(index + 1); break;
    case 'ArrowUp': focusAt(index < 0 ? items.length - 1 : index - 1); break;
    case 'Home': focusAt(0); break;
    case 'End': focusAt(items.length - 1); break;
    case 'Escape':
      // Only the menu closes; the page keeps its preview and checked rows.
      event.stopPropagation();
      close(true);
      break;
    case 'Tab': close(); return;
    default: return;
  }
  event.preventDefault();
}
</script>

<style scoped>
.category-trigger {
  gap: 6px;
  padding: 0 8px 0 10px;
}

.trigger-label {
  max-width: 160px;
  overflow: hidden;
  text-overflow: ellipsis;
}

.trigger-caret {
  color: var(--text-secondary);
  transition: rotate var(--motion-fast) var(--ease-out);
}

.category-trigger[aria-expanded='true'] .trigger-caret {
  rotate: 180deg;
}

.category-trigger.active .trigger-caret {
  color: currentColor;
}

/* Normally in the top layer through the Popover API; the fixed layer and
   z-index only matter where that API is missing. */
.category-menu {
  position: fixed;
  inset: auto;
  z-index: 1000;
  margin: 0;
  min-width: 220px;
  max-width: calc(100vw - 16px);
  overflow-y: auto;
  overscroll-behavior: contain;
  font-family: var(--font-ui);
}

.category-item {
  gap: 10px;
}

.item-check {
  display: grid;
  place-items: center;
  flex-shrink: 0;
  width: 14px;
  height: 14px;
  border: 1px solid color-mix(in srgb, var(--text-secondary) 55%, var(--border-color));
  border-radius: var(--radius-sm);
  background: var(--surface-secondary);
  color: var(--accent-contrast);
}

.item-check.on {
  border-color: var(--primary-solid);
  background: var(--primary-solid);
}

.item-check.radio {
  border-radius: 50%;
}

.item-check.radio.on {
  background: var(--surface-secondary);
  box-shadow: inset 0 0 0 3px var(--primary-solid);
}

.item-icon {
  flex-shrink: 0;
}

/* 类别图标配色与 FileNodeIcon 的 cat-* 同一套 token。 */
.item-icon.cat-image { color: var(--sev-success-fg); }
.item-icon.cat-video,
.item-icon.cat-audio,
.item-icon.cat-email { color: var(--sev-info-fg); }
.item-icon.cat-archive,
.item-icon.cat-database { color: var(--sev-warn-fg); }
.item-icon.cat-executable { color: var(--sev-critical-fg); }
.item-icon.cat-code { color: var(--accent-color); }
.item-icon.cat-pdf,
.item-icon.cat-document,
.item-icon.cat-spreadsheet { color: var(--primary-color); }

.item-label {
  flex: 1;
  min-width: 0;
}

.item-count {
  color: var(--text-secondary);
  font-size: 11px;
  font-variant-numeric: tabular-nums;
}

.menu-note {
  max-width: 240px;
  margin: 4px 0 0;
  padding: 6px 10px;
  border-top: 1px solid var(--border-color);
  color: var(--text-secondary);
  font-size: 11px;
  line-height: 1.5;
  white-space: normal;
}
</style>
