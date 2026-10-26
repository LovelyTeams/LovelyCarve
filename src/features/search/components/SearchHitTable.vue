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
                    @contextmenu="onHitContextMenu($event, virtualItem.item)"
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
              <UiContextMenu
                v-model:open="hitMenu.open"
                :x="hitMenu.x"
                :y="hitMenu.y"
                :entries="hitMenuEntries"
                :label="tr('命中操作')"
                :heading="hitMenu.target?.file_name"
                :subheading="hitMenu.target ? `${hexOffset(hitMenu.target.file_offset)} · ${hitMenu.target.encoding}` : undefined"
              />
            </div>

</template>
<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, nextTick, onScopeDispose, ref, watch } from 'vue';
import { ArrowUpRight, ClipboardCopy, Copy, Hash, Loader2, SearchX, TextQuote } from 'lucide-vue-next';
import FileNodeIcon from '../../../components/display/FileNodeIcon.vue';
import UiContextMenu, { type UiContextMenuEntry } from '../../../components/ui/UiContextMenu.vue';
import { copyText } from '../../../composables/clipboard';
import { useContextMenu } from '../../../composables/useContextMenu';
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

// 右键：选中该命中（右侧显示上下文），菜单作用于它。
const { menu: hitMenu, openFromEvent } = useContextMenu<GlobalSearchHit>();
function onHitContextMenu(event: MouseEvent, hit: GlobalSearchHit) {
  emit('select', hit.hit_seq);
  openFromEvent(event, hit);
}
const hexOffset = (offset: number) => `0x${offset.toString(16).toUpperCase()}`;
const hitMenuEntries = computed<UiContextMenuEntry[]>(() => {
  const hit = hitMenu.target;
  if (!hit) return [];
  return [
    { id: 'open', label: tr('定位到文件'), icon: ArrowUpRight, hint: 'Enter', disabled: props.locating, title: tr('定位文件并查看命中字节'), action: () => emit('open', hit) },
    { kind: 'separator' },
    { id: 'copy-name', label: tr('复制名称'), icon: Copy, action: () => void copyText(hit.file_name, tr('{0} 个名称', { 0: 1 })) },
    { id: 'copy-path', label: tr('复制完整路径'), icon: ClipboardCopy, disabled: !hit.file_path, action: () => void copyText(hit.file_path, tr('{0} 条完整路径', { 0: 1 })) },
    { id: 'copy-context', label: tr('复制命中内容'), icon: TextQuote, action: () => void copyText(hit.context_full, tr('命中内容')) },
    { id: 'copy-offset', label: tr('复制偏移'), icon: Hash, action: () => void copyText(hexOffset(hit.file_offset), tr('命中偏移')) },
  ];
});
</script>
<style scoped src="../../../styles/data-table.css"></style>
<style scoped src="../../../styles/analysis-workspace.css"></style>
