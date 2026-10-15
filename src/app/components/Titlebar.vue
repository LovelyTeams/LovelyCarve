<template>
  <div data-tauri-drag-region class="titlebar">
    <!-- Left: brand mark + wordmark (drag region for window move) -->
    <div class="titlebar-brand">
      <BrandMark :size="20" />
      <div class="titlebar-title">
        LovelyCarve<span class="titlebar-dot">.</span>
      </div>
    </div>

    <!-- Tab strip — fills the center, takes whatever horizontal space
         is left between the brand and the window controls. -->
    <Tabbar
      :tabs="tabs"
      :active-tab-id="activeTabId"
      :is-switching="isSwitching"
      :is-busy="isBusy"
      @activate="(id: string) => $emit('activate-tab', id)"
      @close="(id: string) => $emit('close-tab', id)"
      @new-tab="$emit('new-tab')"
    />

    <slot name="workspace-actions" />

    <!-- Keep the entry stable; the tooltip carries the current case name. -->
    <button
      type="button"
      class="ui-btn sm titlebar-chip titlebar-case-btn"
      :class="{ 'has-case': !!caseName }"
      :title="caseName ? tr('案件管理 · {0}', { 0: caseName }) : tr('案件管理（当前无案件）')"
      :aria-label="tr('案件管理')"
      @click="$emit('open-case')"
    >
      <Briefcase :size="14" />
      <span class="titlebar-chip-label">{{ tr("案件管理") }}</span>
    </button>

    <button class="ui-btn sm titlebar-chip titlebar-settings-btn" type="button" :title="tr('设置中心')" :aria-label="tr('设置中心')" @click="$emit('open-settings')">
      <Settings :size="14" />
      <span class="titlebar-chip-label">{{ tr("设置中心") }}</span>
    </button>

    <!-- Right: platform window controls -->
    <div class="titlebar-controls">
      <button class="titlebar-button" type="button" :aria-label="tr('最小化')" :title="tr('最小化')" @click="minimize">
        <Minus :size="16" />
      </button>
      <button class="titlebar-button" type="button" :aria-label="tr('最大化或还原')" :title="tr('最大化/还原')" @click="toggleMaximize">
        <Square :size="14" />
      </button>
      <button class="titlebar-button close" type="button" :aria-label="tr('关闭窗口')" :title="tr('关闭')" @click="close">
        <X :size="16" />
      </button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../composables/useLocale';

import { getCurrentWindow } from '@tauri-apps/api/window';
import {
  Briefcase, Minus, Settings, Square, X,
} from 'lucide-vue-next';
import BrandMark from '../../components/display/BrandMark.vue';
import Tabbar from './Tabbar.vue';
import type { WorkspaceTab } from '../workspace/useTabManager';

withDefaults(defineProps<{
  tabs?: WorkspaceTab[];
  activeTabId?: string | null;
  isSwitching?: boolean;
  isBusy?: boolean;
  caseName?: string | null;
}>(), {
  tabs: () => [],
  activeTabId: null,
  isSwitching: false,
  isBusy: false,
  caseName: null,
});

defineEmits<{
  'open-settings': [];
  'activate-tab': [tabId: string];
  'close-tab': [tabId: string];
  'new-tab': [];
  'open-case': [];
}>();

const appWindow = getCurrentWindow();
const minimize = () => appWindow.minimize();
const toggleMaximize = () => appWindow.toggleMaximize();
const close = () => appWindow.close();
</script>
