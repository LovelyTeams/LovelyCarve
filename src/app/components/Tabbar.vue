<template>
  <nav class="tabbar" :aria-label="tr('证据标签页')" data-tauri-drag-region>
    <div ref="scrollRegion" class="tabbar-scroll">
      <div
        v-for="tab in tabs"
        :key="tab.id"
        class="tab"
        :data-tab-id="tab.id"
        :class="{
          active: tab.id === activeTabId,
          'mode-image': tab.mode === 'image',
          'mode-evidence': tab.mode === 'evidence',
          pending: tab.pendingLoad,
          disabled: isBusy && tab.id !== activeTabId,
        }"
        @auxclick.middle.prevent="closeTab(tab.id)"
      >
        <button
          type="button"
          class="tab-activate"
          :aria-current="tab.id === activeTabId ? 'page' : undefined"
          :title="tab.pendingLoad ? tr('{0}（等待当前分析完成）', { 0: tab.filePath }) : tab.filePath"
          :disabled="isBusy && tab.id !== activeTabId"
          @click="$emit('activate', tab.id)"
        >
          <Loader2 v-if="isSwitching && tab.id === activeTabId" :size="14" class="tab-icon spin" />
          <Clock v-else-if="tab.pendingLoad" :size="14" class="tab-icon" />
          <component v-else :is="tab.mode === 'image' ? HardDrive : ClipboardList" :size="14" class="tab-icon" />
          <span class="tab-label">{{ tab.fileName }}</span>
        </button>
        <button
          type="button"
          class="ui-icon-btn sm tab-close"
          :title="tr('关闭 {0}', { 0: tab.fileName })"
          :aria-label="tr('关闭 {0}', { 0: tab.fileName })"
          :disabled="isBusy && tab.id !== activeTabId"
          @click="closeTab(tab.id)"
          @auxclick.middle.stop.prevent="closeTab(tab.id)"
        >
          <X :size="14" />
        </button>
      </div>
    </div>
    <button ref="newTabButton" type="button" class="ui-icon-btn tab-new" :title="tr('新标签（Ctrl+T）')" :aria-label="tr('新标签')" @click="$emit('new-tab')">
      <Plus :size="16" />
    </button>
  </nav>
</template>

<script setup lang="ts">
import { tr } from '../../composables/useLocale';

import { onBeforeUnmount, onBeforeUpdate, onMounted, onUpdated, ref, watch } from 'vue';
import { ClipboardList, Clock, HardDrive, Loader2, Plus, X } from 'lucide-vue-next';
import type { WorkspaceTab } from '../workspace/useTabManager';

const props = withDefaults(defineProps<{
  tabs: WorkspaceTab[];
  activeTabId: string | null;
  isSwitching?: boolean;
  isBusy?: boolean;
}>(), {
  isSwitching: false,
  isBusy: false,
});

const emit = defineEmits<{
  activate: [tabId: string];
  close: [tabId: string];
  'new-tab': [];
}>();

const scrollRegion = ref<HTMLElement | null>(null);
const newTabButton = ref<HTMLButtonElement | null>(null);
let resizeObserver: ResizeObserver | undefined;
let focusedBeforeUpdate: HTMLElement | null = null;

function activeTabElement() {
  return Array.from(scrollRegion.value?.children ?? []).find(
    element => (element as HTMLElement).dataset.tabId === props.activeTabId,
  ) as HTMLElement | undefined;
}

function revealActiveTab() {
  const region = scrollRegion.value;
  const activeTab = activeTabElement();
  if (!region || !activeTab) return;
  const viewport = region.getBoundingClientRect();
  if (!viewport.width) return;
  const tabBounds = activeTab.getBoundingClientRect();
  // Keep the keyboard focus ring inside the scroll viewport.
  if (tabBounds.left < viewport.left + 4) {
    region.scrollLeft = Math.max(0, region.scrollLeft + tabBounds.left - viewport.left - 4);
  } else if (tabBounds.right > viewport.right - 4) {
    region.scrollLeft += tabBounds.right - viewport.right + 4;
  }
}

function closeTab(tabId: string) {
  if (props.isBusy && tabId !== props.activeTabId) return;
  emit('close', tabId);
}

// Closing evidence can await confirmation or backend cleanup before removal.
onBeforeUpdate(() => {
  focusedBeforeUpdate = scrollRegion.value?.contains(document.activeElement)
    ? document.activeElement as HTMLElement : null;
});
onUpdated(() => {
  if (focusedBeforeUpdate && !focusedBeforeUpdate.isConnected && document.activeElement === document.body) {
    (activeTabElement()?.querySelector<HTMLButtonElement>('.tab-activate') ?? newTabButton.value)?.focus();
  }
  focusedBeforeUpdate = null;
});

watch(() => [props.activeTabId, props.tabs.map(tab => [tab.id, tab.fileName])], revealActiveTab, { flush: 'post' });
onMounted(() => {
  revealActiveTab();
  if (typeof ResizeObserver !== 'undefined' && scrollRegion.value) {
    resizeObserver = new ResizeObserver(revealActiveTab);
    resizeObserver.observe(scrollRegion.value);
  }
});
onBeforeUnmount(() => resizeObserver?.disconnect());
</script>
