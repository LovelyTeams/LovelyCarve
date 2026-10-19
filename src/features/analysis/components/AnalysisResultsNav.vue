<template>
  <section class="result-navigation" :aria-label="tr('分析结果目录')">
    <label class="ui-search directory-search"><Search :size="13" aria-hidden="true" /><input ref="searchInput" v-model="search" :aria-label="tr('搜索结果项')" :placeholder="tr('搜索结果项…')" @keydown.esc.stop.prevent="clearSearch" /><button v-if="search" class="ui-icon-btn sm" type="button" :title="tr('清除目录搜索')" :aria-label="tr('清除目录搜索')" @click="clearSearch"><X :size="12" aria-hidden="true" /></button></label>
    <div ref="scroller" class="directory-scroll">
      <div v-if="treeLoading && !groups.length" class="directory-state" role="status"><Loader2 :size="16" class="spin" aria-hidden="true" />{{ tr("正在读取分析结果…") }}</div>
      <div v-else-if="treeError" class="directory-state" role="alert"><span>{{ tr("分析结果读取失败：{0}", { 0: treeError }) }}</span><button class="ui-btn link sm" @click="reload">{{ tr("重试") }}</button></div>
      <div v-else-if="!groups.length" class="directory-state" role="status">{{ search ? tr("未找到结果项") : tr("暂无分析结果") }}</div>
      <!-- 分类做小节标题；每次运行一组：折叠按钮与运行标题（名称 + 目标，点开运行信息），下面是记录类型及条数。 -->
      <section v-for="section in sections" v-else :key="section.category" class="result-section">
        <h3 class="section-label">{{ section.category }}</h3>
        <div v-for="group in section.groups" :key="group.id" class="result-group">
          <div class="run-row" :class="{ selected: isCurrent(group.run) }">
            <button class="run-toggle" type="button" :aria-expanded="isExpanded(group)" :title="isExpanded(group) ? tr('收起') : tr('展开')"
              :aria-label="isExpanded(group) ? tr('收起 {0}', { 0: runName(group) }) : tr('展开 {0}', { 0: runName(group) })" @click="toggleGroup(group.id)">
              <ChevronRight :size="14" class="chevron" :class="{ open: isExpanded(group) }" aria-hidden="true" />
            </button>
            <button class="run-open" type="button" :aria-current="isCurrent(group.run) ? 'true' : undefined" :title="runTitle(group)" @click="group.run ? openResult(group.run) : toggleGroup(group.id)">
              <span class="run-text"><span class="run-name">{{ group.label }}</span><span class="run-target">{{ group.target || tr("整个检材") }}</span></span>
              <span v-if="group.legacy" class="ui-badge sm">{{ tr("旧版") }}</span>
              <span v-if="group.warningCount" class="ui-badge sm warn run-warnings" :title="tr('{0} 条告警', { 0: group.warningCount.toLocaleString() })"><TriangleAlert :size="11" aria-hidden="true" />{{ group.warningCount.toLocaleString() }}</span>
            </button>
          </div>
          <UiTransition>
            <ul v-if="isExpanded(group)" class="result-list">
              <li v-for="node in group.results" :key="node.nodeId">
                <button class="result-item" :class="{ selected: isCurrent(node), empty: node.childCount === 0 }" :aria-current="isCurrent(node) ? 'true' : undefined" :title="nodeLabel(node)" @click="openResult(node)"><span class="result-name">{{ nodeLabel(node) }}</span><span class="result-count">{{ node.childCount.toLocaleString() }}</span></button>
              </li>
              <li v-if="!group.results.length" class="group-empty">{{ tr("没有记录，查看运行信息了解告警。") }}</li>
            </ul>
          </UiTransition>
        </div>
      </section>
    </div>
  </section>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import UiTransition from '../../../components/ui/UiTransition.vue';
import { computed, nextTick, ref, watch } from 'vue';
import { ChevronRight, Loader2, Search, TriangleAlert, X } from 'lucide-vue-next';
import { useAnalysisResults } from '../composables/useAnalysisResults';
import { useAnalysisLabels } from '../composables/useAnalysisLabels';
import { buildAnalysisGroups, groupOfNode, sectionAnalysisGroups, type AnalysisResultGroup } from '../model/analysisResultView';
import type { AnalysisResultNode } from '../../../lib/ipc/analysisResults';
const props = defineProps<{ imageId: string }>();
const emit = defineEmits<{ select: [] }>();
const { tree, treeLoading, treeError, selectedNode, selectNode, reload, setImageId, ensureFresh } = useAnalysisResults();
const { nodeLabels, nodeLabel } = useAnalysisLabels();
function openResult(node: AnalysisResultNode) { selectNode(node); emit('select'); }
const search = ref('');
const searchInput = ref<HTMLInputElement | null>(null);
const scroller = ref<HTMLElement | null>(null);
const collapsedGroups = ref(new Set<string>());
const groups = computed(() => buildAnalysisGroups(tree.value, props.imageId, search.value, nodeLabels));
const sections = computed(() => sectionAnalysisGroups(groups.value));
const isCurrent = (node: AnalysisResultNode | null) => !!node && selectedNode.value?.nodeId === node.nodeId;
const isExpanded = (group: AnalysisResultGroup) => !!search.value || !collapsedGroups.value.has(group.id);
const runName = (group: AnalysisResultGroup) => [group.label, group.target].filter(Boolean).join(' · ');
const runTitle = (group: AnalysisResultGroup) => [group.category, group.label, group.target || tr("整个检材")].join(' · ');
function clearSearch() { search.value = ''; searchInput.value?.focus(); }
function toggleGroup(id: string) {
  if (collapsedGroups.value.has(id)) collapsedGroups.value.delete(id);
  else collapsedGroups.value.add(id);
}
/** Selections made elsewhere (the run overview, the task center) open their
 *  group and scroll it into this column without moving the page. */
function revealSelection() {
  const container = scroller.value;
  const item = container?.querySelector<HTMLElement>('[aria-current="true"]');
  if (!container || !item) return;
  const top = item.offsetTop;
  const bottom = top + item.offsetHeight;
  if (top < container.scrollTop) container.scrollTop = Math.max(0, top - 36);
  else if (bottom > container.scrollTop + container.clientHeight) container.scrollTop = bottom - container.clientHeight + 8;
}
watch(() => selectedNode.value?.nodeId, async () => {
  const group = groupOfNode(groups.value, selectedNode.value);
  if (group) collapsedGroups.value.delete(group.id);
  await nextTick();
  revealSelection();
});
watch(() => props.imageId, imageId => {
  search.value = '';
  collapsedGroups.value.clear();
  setImageId(imageId);
  ensureFresh();
}, { immediate: true });
</script>

<style scoped>
.result-navigation { display: flex; flex-direction: column; flex: 1; min-width: 0; min-height: 0; color: var(--text-primary); }
.directory-search { flex-shrink: 0; margin: 12px 12px 4px; padding-right: 4px; }
.directory-scroll { position: relative; flex: 1; min-height: 0; overflow-y: auto; padding: 0 8px 16px; }
.result-section + .result-section { margin-top: 8px; }
.section-label { margin: 0; padding: 12px 8px 4px; color: var(--text-secondary); font-size: 11px; font-weight: 600; line-height: 16px; }

/* 运行标题：左侧是折叠按钮，右侧整块打开运行信息；两行分别是分析器名称与运行目标。 */
.run-row { display: flex; align-items: stretch; min-width: 0; border-radius: var(--radius-md); transition: background var(--transition-fast); }
.run-row:hover { background: var(--sidebar-hover-bg); }
.run-row.selected { background: var(--sidebar-active-bg); }
.run-toggle { display: grid; place-items: center; flex: none; width: 24px; padding: 0; border: 0; border-radius: var(--radius-md) 0 0 var(--radius-md); background: transparent; color: var(--text-secondary); cursor: pointer; }
.run-toggle:hover { color: var(--text-primary); }
.chevron { transition: transform var(--motion-normal) var(--ease-out); }
.chevron.open { transform: rotate(90deg); }
.run-open { display: flex; align-items: center; gap: 6px; flex: 1; min-width: 0; padding: 6px 8px 6px 2px; border: 0; border-radius: 0 var(--radius-md) var(--radius-md) 0; background: transparent; color: inherit; font: inherit; text-align: left; cursor: pointer; }
.run-text { display: flex; flex: 1; flex-direction: column; gap: 2px; min-width: 0; }
.run-name, .run-target { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.run-name { color: var(--text-primary); font-size: 12px; font-weight: 600; line-height: 16px; }
.run-target { color: var(--text-secondary); font-size: 11px; line-height: 14px; }
.run-row.selected .run-name { color: var(--primary-color); }
.run-open .ui-badge { flex-shrink: 0; }
.run-warnings { gap: 3px; font-family: var(--font-mono); }

/* 记录类型挂在一条中性树形缩进线上；条数是右对齐的等宽数字，0 条的类型整行退为次要色。 */
.result-list { margin: 2px 0 6px 12px; padding: 0 0 0 6px; border-left: 1px solid var(--border-color); list-style: none; }
.result-item { display: flex; align-items: center; gap: 8px; width: 100%; height: 30px; padding: 0 8px; border: 0; border-radius: var(--radius-md); background: transparent; color: var(--text-primary); font: inherit; font-size: 12px; text-align: left; cursor: pointer; transition: background var(--transition-fast), color var(--transition-fast); }
.result-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.result-count { flex-shrink: 0; color: var(--text-secondary); font-family: var(--font-mono); font-size: 11px; font-variant-numeric: tabular-nums; }
.result-item.empty { color: var(--text-secondary); }
.result-item:hover { background: var(--sidebar-hover-bg); }
.result-item.selected { background: var(--sidebar-active-bg); color: var(--primary-color); font-weight: 500; }
.result-item.selected .result-count { color: var(--primary-color); }
.run-toggle:focus-visible, .run-open:focus-visible, .result-item:focus-visible { outline: 2px solid var(--primary-color); outline-offset: -2px; }
.group-empty { padding: 4px 8px 6px; color: var(--text-secondary); font-size: 11px; line-height: 1.5; }
.directory-state { padding: 20px 10px; display: flex; flex-direction: column; align-items: center; gap: 8px; font-size: 12px; color: var(--text-secondary); overflow-wrap: anywhere; }
</style>
