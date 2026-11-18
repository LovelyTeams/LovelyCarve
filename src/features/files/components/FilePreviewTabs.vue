<template>
  <div class="preview-tab-bar">
  <div ref="strip" class="preview-tabs ui-scroll-fade-x" role="group" :aria-label="tr('打开的文件')" @wheel="onWheel" @scroll.passive="updateOverflow">
    <div
      v-for="tab in tabs"
      :key="tab.id"
      class="preview-tab"
      :class="{ active: tab.id === activeId, transient: !tab.pinned, deleted: tab.node.deleted }"
      :data-tab-id="tab.id"
      @auxclick.middle.prevent="emit('close', tab.id)"
      @contextmenu="tabMenu.openFromEvent($event, tab)"
    >
      <button
        type="button"
        class="preview-tab-activate"
        :aria-current="tab.id === activeId ? 'true' : undefined"
        :title="titleOf(tab)"
        @click="emit('activate', tab.id)"
        @dblclick="emit('pin', tab.id)"
        @keydown="onTabKeydown($event, tab)"
      >
        <FileNodeIcon :id="tab.node.id" :kind="tab.node.kind" :name="tab.node.name" :size="14" />
        <span class="preview-tab-label">{{ tab.node.name }}</span>
      </button>
      <button
        type="button"
        class="ui-icon-btn sm preview-tab-close"
        :title="tr('关闭 {0}', { 0: tab.node.name })"
        :aria-label="tr('关闭 {0}', { 0: tab.node.name })"
        @click="emit('close', tab.id)"
      ><X :size="12" /></button>
    </div>
  </div>
  <button v-if="overflowing" type="button" class="ui-icon-btn sm preview-tab-list" :aria-expanded="listMenu.menu.open"
    :title="tr('全部打开的文件（{0}）', { 0: tabs.length })" :aria-label="tr('全部打开的文件（{0}）', { 0: tabs.length })"
    @click="listMenu.openBelow($event.currentTarget as Element, null)"><ChevronDown :size="14" /></button>
  </div>
  <UiContextMenu
    v-model:open="tabMenu.menu.open"
    :x="tabMenu.menu.x"
    :y="tabMenu.menu.y"
    :entries="menuEntries"
    :label="tr('标签页操作')"
    :heading="tabMenu.menu.target?.node.name"
    :subheading="tabMenu.menu.target?.fullPath"
  />
  <UiContextMenu
    v-model:open="listMenu.menu.open"
    :x="listMenu.menu.x"
    :y="listMenu.menu.y"
    :entries="listEntries"
    :label="tr('打开的文件')"
    :heading="tr('打开的文件')"
  />
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { ChevronDown, CircleX, ClipboardCopy, FolderSearch, ListX, Pin, X } from 'lucide-vue-next';
import FileNodeIcon from '../../../components/display/FileNodeIcon.vue';
import UiContextMenu, { type UiContextMenuEntry } from '../../../components/ui/UiContextMenu.vue';
import { isContextMenuKey, useContextMenu } from '../../../composables/useContextMenu';
import { compactMenuEntries } from '../../../utils/menuPlacement';
import type { FilePreviewTab } from '../model/previewTabs';

/** The files open in the preview. Clicking switches without touching the list;
 *  the preview slot (italic) is replaced by the next clicked file until it is
 *  kept open with a double click. */
const props = defineProps<{ tabs: FilePreviewTab[]; activeId: string | null }>();
const emit = defineEmits<{
  activate: [id: string];
  close: [id: string];
  pin: [id: string];
  closeOthers: [id: string];
  closeAll: [];
  reveal: [id: string];
  copyPath: [id: string];
}>();

const strip = ref<HTMLElement | null>(null);
const tabMenu = useContextMenu<FilePreviewTab>();

function titleOf(tab: FilePreviewTab) {
  const lines = [tab.fullPath];
  if (tab.node.deleted) lines.push(tr('已删除'));
  if (!tab.pinned) lines.push(tr('预览：单击其他文件会替换此标签页，双击保持打开'));
  return lines.join('\n');
}

const menuEntries = computed<UiContextMenuEntry[]>(() => {
  const tab = tabMenu.menu.target;
  if (!tab) return [];
  return compactMenuEntries<UiContextMenuEntry>([
    ...(!tab.pinned && tab.node.kind !== 'dir'
      ? [{ id: 'pin', label: tr('保持打开'), icon: Pin, action: () => emit('pin', tab.id) }] : []),
    { id: 'reveal', label: tr('在文件列表中定位'), icon: FolderSearch, title: tr('列表转到该文件所在目录并选中它'), action: () => emit('reveal', tab.id) },
    { id: 'copy-path', label: tr('复制完整路径'), icon: ClipboardCopy, action: () => emit('copyPath', tab.id) },
    { kind: 'separator' },
    { id: 'close', label: tr('关闭'), icon: X, action: () => emit('close', tab.id) },
    ...(props.tabs.length > 1 ? [
      { id: 'close-others', label: tr('关闭其他标签页'), icon: ListX, action: () => emit('closeOthers', tab.id) },
      { id: 'close-all', label: tr('关闭全部标签页'), icon: CircleX, action: () => emit('closeAll') },
    ] : []),
  ]);
});

// 标签页放不下时，末尾的列表按钮列出全部打开的文件，不必横向滚动去找。
const listMenu = useContextMenu<null>();
const overflowing = ref(false);
const listEntries = computed<UiContextMenuEntry[]>(() => props.tabs.map(tab => ({
  id: `tab:${tab.id}`, label: tab.node.name, title: tab.fullPath, hint: tab.pinned ? undefined : tr('预览'),
  checked: tab.id === props.activeId, action: () => emit('activate', tab.id),
})));
function updateOverflow() {
  const element = strip.value;
  overflowing.value = !!element && element.scrollWidth > element.clientWidth + 1;
}

function tabButtons() {
  return [...strip.value?.querySelectorAll<HTMLButtonElement>('.preview-tab-activate') ?? []];
}

/** ←/→, Home and End move between tabs and show them; Delete closes one. */
function onTabKeydown(event: KeyboardEvent, tab: FilePreviewTab) {
  if (isContextMenuKey(event)) {
    event.preventDefault();
    tabMenu.openBelow(event.currentTarget as Element, tab);
    return;
  }
  if (event.key === 'Delete') {
    event.preventDefault();
    emit('close', tab.id);
    return;
  }
  const index = props.tabs.findIndex(item => item.id === tab.id);
  const target = event.key === 'ArrowLeft' ? index - 1 : event.key === 'ArrowRight' ? index + 1
    : event.key === 'Home' ? 0 : event.key === 'End' ? props.tabs.length - 1 : null;
  if (target === null) return;
  event.preventDefault();
  const next = props.tabs[Math.max(0, Math.min(props.tabs.length - 1, target))];
  if (!next) return;
  emit('activate', next.id);
  void nextTick(() => tabButtons()[props.tabs.findIndex(item => item.id === next.id)]?.focus());
}

/** A vertical wheel scrolls the strip sideways. */
function onWheel(event: WheelEvent) {
  const element = strip.value;
  if (!element || Math.abs(event.deltaY) <= Math.abs(event.deltaX) || element.scrollWidth <= element.clientWidth) return;
  event.preventDefault();
  element.scrollLeft += event.deltaY;
}

function revealActive() {
  const element = strip.value;
  const active = element?.querySelector<HTMLElement>('.preview-tab.active');
  if (!element || !active) return;
  const viewport = element.getBoundingClientRect();
  if (!viewport.width) return;
  const bounds = active.getBoundingClientRect();
  if (bounds.left < viewport.left + 4) element.scrollLeft = Math.max(0, element.scrollLeft + bounds.left - viewport.left - 4);
  else if (bounds.right > viewport.right - 4) element.scrollLeft += bounds.right - viewport.right + 4;
}
function refresh() {
  updateOverflow();
  revealActive();
}
watch(() => [props.activeId, props.tabs.map(tab => tab.node.name).join('\n')], refresh, { flush: 'post' });
let resizeObserver: ResizeObserver | undefined;
onMounted(() => {
  refresh();
  if (typeof ResizeObserver !== 'undefined' && strip.value) {
    resizeObserver = new ResizeObserver(refresh);
    resizeObserver.observe(strip.value);
  }
});
onBeforeUnmount(() => resizeObserver?.disconnect());
</script>

<style scoped>
/* 打开的文件是标题条上的胶囊：当前文件抬到内容底色并带细描边，其余悬停时显中性底色；
   预览位（单击替换的那一个）名称用斜体。放不下时先等比收窄，再横向滚动，末尾的列表按钮列出全部。 */
.preview-tab-bar {
  display: flex;
  flex: 1;
  align-items: center;
  gap: 2px;
  min-width: 0;
  height: 100%;
}

.preview-tabs {
  display: flex;
  flex: 0 1 auto;
  align-items: center;
  gap: 2px;
  min-width: 0;
  height: 100%;
  padding: 4px 2px;
  overflow-x: auto;
  overflow-y: hidden;
  scrollbar-width: none;
}

.preview-tabs::-webkit-scrollbar { display: none; }

.preview-tab-list { flex: none; }

.preview-tab {
  display: inline-flex;
  flex: 0 1 auto;
  align-items: center;
  height: 30px;
  min-width: 104px;
  max-width: 200px;
  padding-right: 2px;
  border-radius: var(--radius-md);
  color: var(--text-secondary);
  cursor: default;
  transition: color var(--transition-fast), background var(--transition-fast), box-shadow var(--transition-normal);
}

:root[data-motion='full'] .preview-tab {
  animation: ui-rise-in var(--motion-normal) var(--ease-out) backwards;
}

.preview-tab:where(:hover, :focus-within) {
  color: var(--text-primary);
  background: var(--hover-bg);
}

.preview-tab.active {
  color: var(--text-primary);
  background: var(--surface-secondary);
  box-shadow: var(--shadow-sm), 0 0 0 1px var(--border-color);
}

.preview-tab-activate {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  height: 100%;
  padding: 0 4px 0 8px;
  border: 0;
  border-radius: var(--radius-md) 0 0 var(--radius-md);
  background: transparent;
  color: inherit;
  font: inherit;
  font-size: 12px;
  cursor: pointer;
}

.preview-tab-activate:focus-visible {
  outline: 2px solid var(--primary-color);
  outline-offset: -2px;
}

.preview-tab-label {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.preview-tab.transient .preview-tab-label { font-style: italic; }
.preview-tab.deleted .preview-tab-label { color: var(--sev-critical-fg); text-decoration: line-through; }

.preview-tab-close {
  flex: none;
  opacity: 0;
  transition: opacity var(--transition-fast), background var(--transition-fast), color var(--transition-fast);
}

.preview-tab:where(:hover, :focus-within, .active) .preview-tab-close { opacity: 1; }
.preview-tab-close:focus-visible { outline-offset: -2px; }
</style>
