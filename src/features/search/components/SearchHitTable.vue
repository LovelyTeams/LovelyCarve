<template>
            <div ref="tableWrapRef" class="hits-table-wrap">
              <table class="data-table hits-table">
                <colgroup>
                  <col class="name-column" /><col class="path-column" /><col class="context-column" />
                  <col class="action-column" />
                </colgroup>
                <thead><tr><th>{{ tr("文件") }}</th><th>{{ tr("路径 / 来源") }}</th><th>{{ tr("命中内容") }}</th><th><span class="action-heading">{{ tr("定位") }}</span></th></tr></thead>
                <tbody>
                  <tr v-if="paddingTop > 0" class="virtual-spacer" aria-hidden="true">
                    <td :colspan="columnCount" :style="{ height: `${paddingTop}px` }"></td>
                  </tr>
                  <tr
                    v-for="virtualItem in visibleItems"
                    :key="rowKeyOf(virtualItem.item)"
                    :class="{ selected: selectedHitSeq === virtualItem.item.hit_seq }"
                    :aria-selected="selectedHitSeq === virtualItem.item.hit_seq"
                    tabindex="0"
                    @click="emit('select', virtualItem.item.hit_seq)"
                    @dblclick="emit('open', virtualItem.item)"
                    @keydown.enter="emit('open', virtualItem.item)"
                  >
                    <td class="file-name-cell">
                      <span class="file-name-content" :title="virtualItem.item.file_name">
                        <FileNodeIcon :id="virtualItem.item.node_id" :kind="virtualItem.item.source_kind === 'file' ? 'file' : 'unallocated'" :name="virtualItem.item.file_name" :size="16" />
                        <strong>{{ virtualItem.item.file_name }}</strong>
                      </span>
                    </td>
                    <td class="path-cell" :title="virtualItem.item.file_path || sourceKindLabel(virtualItem.item.source_kind, tr)"><span v-if="virtualItem.item.source_kind !== 'file'" class="ui-badge sm source-badge">{{ sourceKindLabel(virtualItem.item.source_kind, tr) }}</span><span class="mono">{{ virtualItem.item.file_path || '—' }}</span></td>
                    <td class="context-cell">
                      <div class="context-preview mono" :title="virtualItem.item.context_full">
                        <template v-for="(segment, index) in highlightSearchPattern(buildSearchContextPreview(virtualItem.item.context_preview, virtualItem.item.pattern), virtualItem.item.pattern)" :key="index">
                          <mark v-if="segment.matched">{{ segment.text }}</mark><span v-else>{{ segment.text }}</span>
                        </template>
                      </div>
                    </td>
                    <td><button class="ui-btn link locate-link" :title="tr('定位文件并查看命中字节')" :disabled="locating" @click.stop="emit('open', virtualItem.item)" @keydown.enter.stop><ArrowUpRight :size="14" />{{ tr("定位") }}</button></td>
                  </tr>
                  <tr v-if="paddingBottom > 0" class="virtual-spacer" aria-hidden="true">
                    <td :colspan="columnCount" :style="{ height: `${paddingBottom}px` }"></td>
                  </tr>
                  <tr v-if="hits.length === 0 && !readError" class="empty-row">
                    <td :colspan="columnCount">
                      <Loader2 v-if="loadingHits || selectedRunning" :size="17" class="spin" />
                      <SearchX v-else :size="18" />
                      <span>{{
                        loadingHits || selectedRunning
                          ? tr("正在等待命中结果…")
                          : filterKeyword
                            ? tr("没有符合当前过滤条件的记录")
                            : tr("本次搜索没有命中")
                      }}</span>
                      <button v-if="filterKeyword && !loadingHits && !selectedRunning" class="ui-btn sm" @click="emit('clear-filter')">{{ tr("清除筛选") }}</button>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>

</template>
<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, nextTick, onScopeDispose, ref, watch } from 'vue';
import { ArrowUpRight, Loader2, SearchX } from 'lucide-vue-next';
import FileNodeIcon from '../../../components/display/FileNodeIcon.vue';
import { useVirtualList } from '../../../composables/useVirtualList';
import type { GlobalSearchHit } from '../../../lib/ipc/search';
import { buildSearchContextPreview, highlightSearchPattern, sourceKindLabel } from '../model/searchResults';
const props = defineProps<{
  hits: GlobalSearchHit[]; hitsTotal: number; loadingHits: boolean; selectedRunning: boolean;
  filterKeyword: string; readError: string; selectedHitSeq: number | null; locating?: boolean; contextKey: string;
}>();
const emit = defineEmits<{ select: [sequence: number]; open: [hit: GlobalSearchHit]; 'load-through': [index: number]; 'clear-filter': [] }>();
const tableWrapRef = ref<HTMLElement | null>(null);
const { visibleItems, paddingTop, paddingBottom, range } = useVirtualList({
  items: computed(() => props.hits), itemHeight: 36, containerRef: tableWrapRef,
  overscan: 10, virtualCount: computed(() => props.hitsTotal),
});
const columnCount = 4;
const rowKeyOf = (hit: GlobalSearchHit) => `hit:${hit.hit_seq}`;
watch(range, range => emit('load-through', range.end));
let generation = 0;
watch(() => props.contextKey, () => {
  const captured = ++generation;
  void nextTick(() => { if (captured === generation) tableWrapRef.value?.scrollTo({ top: 0 }); });
});
onScopeDispose(() => { generation++; });
</script>
<style scoped src="../../../styles/data-table.css"></style>
<style scoped src="../../../styles/analysis-workspace.css"></style>
