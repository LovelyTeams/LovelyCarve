<template>
            <div ref="tableWrapRef" class="hits-table-wrap">
              <table class="data-table hits-table carve-table">
                <colgroup><col class="check-column" /><col class="name-column" /><col class="offset-column" /><col class="size-column" /><col class="confidence-column" /><col /><col class="action-column" /></colgroup>
                <thead><tr><th class="check-cell"><input type="checkbox" :checked="allVisibleSelected" :indeterminate="!allVisibleSelected && visibleArtifacts.some(artifact => checkedIds.has(artifact.id))" :aria-label="tr('选择当前显示结果')" @change="emit('toggle-all')" /></th><th>{{ tr("文件") }}</th><th>{{ tr("起始偏移") }}</th><th class="numeric">{{ tr("大小") }}</th><th>{{ tr("置信度") }}</th><th>{{ tr("扫描区域") }}</th><th><span class="action-heading">{{ tr("恢复") }}</span></th></tr></thead>
                <tbody>
                  <tr v-if="paddingTop > 0" class="virtual-spacer" aria-hidden="true"><td colspan="7" :style="{ height: `${paddingTop}px` }"></td></tr>
                  <tr v-for="{ item: artifact } in visibleItems" :key="artifact.id" :class="{ selected: artifact.id === selectedId }" tabindex="0" :aria-selected="artifact.id === selectedId" @click="emit('select', artifact.id)" @keydown.enter="emit('select', artifact.id)">
                    <td class="check-cell" @click.stop><input type="checkbox" :checked="checkedIds.has(artifact.id)" :aria-label="tr('选择 {0}', { 0: artifact.name })" @change="emit('toggle-checked', artifact.id)" /></td>
                    <td class="file-name-cell"><span class="file-name-content" :title="artifact.name"><span class="category-icon" :style="{ '--category-tone': categoryTone(artifact.category) }"><component :is="categoryIcon(artifact.category)" :size="14" /></span><strong>{{ artifact.name }}</strong><AlertTriangle v-if="artifact.read_fault" :size="13" class="read-warning" :aria-label="tr('包含补零数据')" /></span></td>
                    <td class="mono offset-cell">{{ formatOffset(artifact.offset) }}</td><td class="mono numeric">{{ formatBytes(artifact.length) }}</td>
                    <td><span class="confidence" :class="confidenceTone(artifact.confidence)" :title="tr('置信度 {0}%', { 0: artifact.confidence })"><span class="ui-progress confidence-bar"><span :style="{ width: `${Math.min(100, Math.max(0, artifact.confidence))}%` }"></span></span><span class="mono">{{ artifact.confidence }}%</span></span></td>
                    <td class="path-cell" :title="artifact.region">{{ artifact.region }}</td>
                    <td><button class="ui-btn link locate-link" :disabled="recovering" @click.stop="emit('recover', artifact.id)" @keydown.enter.stop><Download :size="14" />{{ tr("恢复") }}</button></td>
                  </tr>
                  <tr v-if="paddingBottom > 0" class="virtual-spacer" aria-hidden="true"><td colspan="7" :style="{ height: `${paddingBottom}px` }"></td></tr>
                  <tr v-if="!visibleArtifacts.length && !readError" class="empty-row"><td colspan="7"><Loader2 v-if="syncingResults || (isActive && scan.status !== 'paused')" :size="17" class="spin" /><SearchX v-else :size="18" /><span>{{ syncingResults ? tr("正在读取雕刻结果…") : filterIncomplete ? tr("尚有结果未读取，请加载更多完成筛选") : hasFilters ? tr("没有符合当前过滤条件的记录") : isActive ? (scan.status === 'paused' ? tr("扫描已暂停") : tr("正在等待雕刻结果…")) : tr("本次扫描没有发现文件") }}</span><button v-if="hasFilters && !syncingResults" class="ui-btn sm" @click="emit('clear-filters')">{{ tr("清除筛选") }}</button></td></tr>
                </tbody>
              </table>
            </div>
</template>
<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, ref, watch } from 'vue';
import { useVirtualList } from '../../../composables/useVirtualList';
import { Loader2, SearchX, Download, AlertTriangle } from 'lucide-vue-next';
import type { CarveArtifact, CarveScanSnapshot } from '../../../lib/ipc/carve';
import { formatBytes } from '../../../utils/format';
import { categoryTone, confidenceTone, formatOffset } from '../model/carveView';
import { categoryIcon } from './carveIcons';
const props = defineProps<{ visibleArtifacts: CarveArtifact[]; allVisibleSelected: boolean; checkedIds: ReadonlySet<string>;
  selectedId: string | null; recovering: boolean; syncingResults: boolean; readError: string; filterIncomplete: boolean; contextKey: string; isActive: boolean; scan: CarveScanSnapshot; hasFilters: boolean }>();
const emit = defineEmits<{ select: [id: string]; recover: [id: string]; 'toggle-checked': [id: string]; 'toggle-all': []; 'clear-filters': [] }>();
const tableWrapRef = ref<HTMLElement | null>(null);
const { visibleItems, paddingTop, paddingBottom, scrollToTop } = useVirtualList({
  items: computed(() => props.visibleArtifacts), itemHeight: 36, containerRef: tableWrapRef, overscan: 10,
});
watch(() => props.contextKey, scrollToTop);
</script>
<style scoped src="../../../styles/data-table.css"></style>
<style scoped src="../../../styles/analysis-workspace.css"></style>
<style scoped src="../styles/carve-workspace.css"></style>
