<template>
  <aside class="sidebar-nav" :class="{ collapsed, 'hover-preview': collapsed && hoverPreview }" :aria-label="tr('工作区导航')" @pointerleave="closeHover" @focusout="onFocusOut" @keydown.esc="dismissHover">
    <div class="category-rail">
      <nav class="category-list" :aria-label="tr('工作区大类')">
        <button
          v-for="group in groups"
          :key="group.groupId"
          class="ui-btn ghost category-button"
          :class="{ active: selectedGroup?.groupId === group.groupId, previewing: hoverPreview && displayedGroup?.groupId === group.groupId }"
          :aria-pressed="selectedGroup?.groupId === group.groupId"
          :aria-controls="pagesId"
          :aria-expanded="(!collapsed || hoverPreview) && displayedGroup?.groupId === group.groupId"
          :title="group.groupLabel"
          @click="selectGroup(group)"
          @pointerenter="previewGroup(group)"
          @focus="previewGroup(group)"
        >
          <component :is="getIcon(group.groupIcon)" :size="20" aria-hidden="true" />
          <span>{{ group.groupLabel }}</span>
        </button>
      </nav>
      <div class="rail-actions">
        <button
          class="ui-icon-btn rail-action"
          :title="collapsed ? tr('展开子类栏') : tr('收起子类栏')"
          :aria-label="collapsed ? tr('展开子类栏') : tr('收起子类栏')"
          :aria-expanded="!collapsed"
          :aria-controls="pagesId"
          @click="collapsed = !collapsed"
        >
          <component :is="collapsed ? PanelLeftOpen : PanelLeftClose" :size="18" />
        </button>
      </div>
    </div>

    <section v-show="!collapsed || hoverPreview" :id="pagesId" class="page-sidebar" :aria-label="displayedGroup?.groupLabel">
      <nav v-if="!showContext" class="page-list" :aria-label="tr('{0}子页面', { 0: displayedGroup?.groupLabel ?? '' })">
        <button
          v-for="item in displayedGroup?.items ?? []"
          :key="item.id"
          class="ui-btn ghost page-button"
          :class="{ active: activeId === item.id }"
          :aria-current="activeId === item.id ? 'page' : undefined"
          :title="item.label"
          @click="selectItem(item.id)"
        >
          <component :is="getIcon(item.icon)" :size="16" aria-hidden="true" />
          <span class="page-label">{{ item.label }}</span>
          <span v-if="item.badge" class="ui-badge sm" :class="item.badgeType">{{ item.badge.toLocaleString() }}</span>
        </button>
      </nav>
      <div v-if="showContext" class="sidebar-context"><slot name="context" :select="selectItem" /></div>
    </section>
  </aside>
</template>

<script setup lang="ts">
import { tr } from '../../composables/useLocale';

import { computed, ref, useId, useSlots, watch, type Component } from 'vue';
import {
  BarChart3, Database, FileQuestion, Files, FileSearch, Globe, HardDrive,
  Info, Package, PanelLeftClose, PanelLeftOpen, Phone, ScanSearch, Search, Send, Share2, Wifi, Wrench,
} from 'lucide-vue-next';
import type { NavGroup } from '../workspace/types';

const props = withDefaults(defineProps<{
  groups?: NavGroup[];
  activeId?: string;
  contextGroupId?: string;
}>(), { groups: () => [], activeId: '', contextGroupId: 'analysis' });
const emit = defineEmits<{ select: [id: string] }>();
const slots = useSlots();
const pagesId = useId();
const collapsedKey = 'lovelycarve-sidebar-collapsed';
const collapsed = ref((() => {
  try { return localStorage.getItem(collapsedKey) === 'true'; }
  catch { return false; }
})());
watch(collapsed, value => {
  try { localStorage.setItem(collapsedKey, String(value)); }
  catch { /* Navigation remains usable when storage is unavailable. */ }
});

// Page state remains shell-owned, so task links, file location and evidence-tab
// restoration select the same category as direct sidebar clicks.
const selectedGroup = computed(() => props.groups.find(group => group.items.some(item => item.id === props.activeId)) ?? props.groups[0]);
const hoveredGroupId = ref<string | null>(null);
const hoveredGroup = computed(() => props.groups.find(group => group.groupId === hoveredGroupId.value));
const hoverPreview = computed(() => !!hoveredGroup.value);
const displayedGroup = computed(() => hoverPreview.value ? hoveredGroup.value : selectedGroup.value);
const showContext = computed(() => !!slots.context && displayedGroup.value?.groupId === props.contextGroupId);
function previewGroup(group: NavGroup) { hoveredGroupId.value = group.groupId; }
function closeHover() { hoveredGroupId.value = null; }
function onFocusOut(event: FocusEvent) {
  const sidebar = event.currentTarget as HTMLElement;
  if (!(event.relatedTarget instanceof Node) || !sidebar.contains(event.relatedTarget)) closeHover();
}
function dismissHover(event: KeyboardEvent) {
  if (!hoverPreview.value) return;
  event.preventDefault();
  event.stopPropagation();
  (event.currentTarget as HTMLElement).querySelector<HTMLButtonElement>('.category-button.previewing')?.focus();
  closeHover();
}
watch(collapsed, closeHover);
watch(() => props.activeId, closeHover);
const lastPages = new Map<string, string>();
watch(() => [props.groups, props.activeId] as const, () => {
  const group = selectedGroup.value;
  if (group?.items.some(item => item.id === props.activeId)) lastPages.set(group.groupId, props.activeId);
}, { immediate: true });

function selectGroup(group: NavGroup) {
  collapsed.value = false;
  const remembered = lastPages.get(group.groupId);
  const item = group.items.find(item => item.id === remembered) ?? group.items[0];
  if (item) emit('select', item.id);
}
function selectItem(id: string) {
  emit('select', id);
  closeHover();
}

const iconMap: Record<string, Component> = {
  BarChart3, Database, Files, FileSearch, Globe, HardDrive, Info, Package,
  Phone, ScanSearch, Search, Send, Share2, Wifi, Wrench,
};
function getIcon(name: string) { return iconMap[name] ?? FileQuestion; }
</script>

<style scoped src="../../styles/components/sidebar-nav.css"></style>
