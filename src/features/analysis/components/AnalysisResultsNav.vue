<template>
  <section class="result-navigation" :aria-label="tr('分析结果目录')">
    <label class="ui-search directory-search"><Search :size="13" aria-hidden="true" /><input ref="searchInput" v-model="search" :aria-label="tr('搜索结果项')" :placeholder="tr('搜索结果项…')" @keydown.esc.stop.prevent="clearSearch" /><button v-if="search" class="ui-icon-btn sm" type="button" :title="tr('清除目录搜索')" :aria-label="tr('清除目录搜索')" @click="clearSearch"><X :size="12" aria-hidden="true" /></button></label>
    <div class="directory-scroll">
      <div v-if="treeLoading && !groups.length" class="directory-state" role="status"><Loader2 :size="16" class="spin" aria-hidden="true" />{{ tr("正在读取分析结果…") }}</div>
      <div v-else-if="treeError" class="directory-state" role="alert"><span>{{ tr("分析结果读取失败：") }}{{ treeError }}</span><button class="ui-btn link sm" @click="reload">{{ tr("重试") }}</button></div>
      <div v-else-if="!groups.length" class="directory-state" role="status">{{ search ? tr("未找到结果项") : tr("暂无分析结果") }}</div>
      <div v-for="group in groups" v-else :key="group.id" class="result-group">
        <button class="group-heading" :aria-expanded="search ? true : !collapsedGroups.has(group.id)" :title="group.category" @click="toggleGroup(group.id)"><ChevronDown :size="13" :class="{ closed: !search && collapsedGroups.has(group.id) }" /><span>{{ group.label }}</span><small v-if="group.category">{{ group.category }}</small></button>
        <UiTransition><div v-if="search || !collapsedGroups.has(group.id)">
          <button v-for="node in group.results" :key="node.nodeId" class="result-item" :class="{ selected: selectedNode?.nodeId === node.nodeId }" :aria-current="selectedNode?.nodeId === node.nodeId ? 'true' : undefined" :title="node.label" @click="openResult(node)"><Table2 :size="13" aria-hidden="true" /><span>{{ node.label }}</span><small class="ui-count" :class="{ accent: selectedNode?.nodeId === node.nodeId }">{{ node.childCount.toLocaleString() }}</small></button>
        </div></UiTransition>
      </div>
    </div>
  </section>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import UiTransition from '../../../components/ui/UiTransition.vue';
import { computed, ref, watch } from 'vue';
import { ChevronDown, Loader2, Search, Table2, X } from 'lucide-vue-next';
import { useAnalysisResults } from '../composables/useAnalysisResults';
import { buildAnalysisGroups } from '../model/analysisResultView';
import type { AnalysisResultNode } from '../../../lib/ipc/analysisResults';
const props = defineProps<{ imageId: string }>();
const emit = defineEmits<{ select: [] }>();
const { tree, treeLoading, treeError, selectedNode, selectNode, reload, setImageId, ensureFresh } = useAnalysisResults();
function openResult(node: AnalysisResultNode) { selectNode(node); emit('select'); }
const search = ref('');
const searchInput = ref<HTMLInputElement | null>(null);
const collapsedGroups = ref(new Set<string>());
const groups = computed(() => buildAnalysisGroups(tree.value, props.imageId, search.value));
function clearSearch() { search.value = ''; searchInput.value?.focus(); }
function toggleGroup(id: string) {
  if (collapsedGroups.value.has(id)) collapsedGroups.value.delete(id);
  else collapsedGroups.value.add(id);
}
watch(() => props.imageId, imageId => {
  search.value = '';
  collapsedGroups.value.clear();
  setImageId(imageId);
  ensureFresh();
}, { immediate: true });
</script>

<style scoped>
.result-navigation { display: flex; flex-direction: column; flex: 1; min-width: 0; min-height: 0; color: var(--text-primary); }
.directory-search { flex-shrink: 0; margin: 12px 12px 8px; padding-right: 4px; }
.directory-scroll { flex: 1; min-height: 0; overflow-y: auto; padding: 0 8px 12px; }
.group-heading, .result-item { display: flex; align-items: center; width: 100%; gap: 8px; border: 0; border-radius: var(--radius-md); background: transparent; cursor: pointer; text-align: left; transition: background var(--transition-fast), color var(--transition-fast); }
.group-heading { padding: 8px 6px 6px; font-size: 12px; font-weight: 600; color: var(--text-secondary); }
.group-heading span, .result-item span { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.group-heading small { flex-shrink: 0; font-size: 11px; font-weight: 400; }
.group-heading svg { flex-shrink: 0; transition: transform var(--transition-fast); }
.group-heading svg.closed { transform: rotate(-90deg); }
.result-item { height: 32px; padding: 0 8px 0 24px; font-size: 12px; color: var(--text-primary); }
.result-item svg { flex-shrink: 0; color: var(--text-secondary); }
.result-item .ui-count { flex-shrink: 0; }
.result-item:not(.selected) .ui-count { color: var(--text-primary); }
.group-heading:hover, .result-item:hover { background: var(--sidebar-hover-bg); }
.group-heading:hover { color: var(--text-primary); }
.result-item.selected { background: var(--sidebar-active-bg); color: var(--primary-color); font-weight: 500; }
.result-item.selected svg { color: var(--primary-color); }
.group-heading:focus-visible, .result-item:focus-visible { outline: 2px solid var(--primary-color); outline-offset: -2px; }
.directory-state { padding: 20px 10px; display: flex; flex-direction: column; align-items: center; gap: 8px; font-size: 12px; color: var(--text-secondary); overflow-wrap: anywhere; }
</style>
