<template>
  <div ref="tableWrapRef" class="table-wrap">
    <table class="data-table grid">
      <colgroup>
        <col class="col-check" /><col class="col-name" />
        <template v-if="signatureColumns"><col class="col-offset" /><col class="col-size" /><col class="col-confidence" /><col class="col-type" /></template>
        <template v-else><col class="col-size" /><col class="col-time" /><col class="col-type" /><col class="col-status" /></template>
      </colgroup>
      <thead><tr>
        <th class="cell-check"><input type="checkbox" :checked="allChecked" :indeterminate="someChecked" :disabled="busy" :aria-label="tr('选择当前列表')" @change="emit('toggle-all')" /></th>
        <th>{{ tr("名称") }}</th>
        <template v-if="signatureColumns"><th>{{ tr("起始偏移") }}</th><th class="numeric">{{ tr("大小") }}</th><th>{{ tr("置信度") }}</th><th>{{ tr("类型") }}</th></template>
        <template v-else><th class="numeric">{{ tr("大小") }}</th><th>{{ tr("修改时间") }}</th><th>{{ tr("类型") }}</th><th>{{ tr("状态") }}</th></template>
      </tr></thead>
      <tbody>
        <tr v-if="paddingTop > 0" class="virtual-spacer" aria-hidden="true"><td :colspan="6" :style="{ height: `${paddingTop}px` }"></td></tr>
        <tr v-for="{ item } in visibleItems" :key="item.key" class="body-row" :class="{ deleted: item.node?.deleted }" tabindex="0"
          :aria-selected="item.key === selectedKey" @click="onRowClick($event, item)" @dblclick="item.kind !== 'file' && emit('open', item)"
          @keydown.enter="item.kind === 'file' ? emit('select', item) : emit('open', item)" @contextmenu="onRowContextMenu($event, item)">
          <td class="cell-check" @click.stop @dblclick.stop><input type="checkbox" :checked="isChecked(item)" :indeterminate="isPartial(item)" :disabled="busy || !recoverable(item)" :aria-label="tr('选择 {0}', { 0: item.name })" @change="emit('toggle', item)" /></td>
          <td class="cell-name">
            <span class="name-content" :title="item.path">
              <ScanSearch v-if="item.kind === 'signatures'" :size="14" class="kind-icon signature-icon" />
              <FileNodeIcon v-else :id="item.kind === 'volume' ? 'part:0' : item.key" :kind="item.kind === 'file' ? 'file' : 'dir'" :name="item.name" :size="14" class="kind-icon" />
              <span class="name-text" :class="{ 'is-dir': item.kind !== 'file' }">{{ item.name }}</span>
              <span v-if="item.kind !== 'file' && item.fileCount" class="dir-count mono">{{ item.fileCount.toLocaleString() }}</span>
              <AlertTriangle v-if="item.artifact?.read_fault" :size="13" class="read-warning" :aria-label="tr('包含补零数据')" />
            </span>
          </td>
          <template v-if="signatureColumns">
            <td class="mono offset-cell">{{ item.artifact ? formatOffset(item.artifact.offset) : '—' }}</td>
            <td class="mono numeric">{{ item.kind === 'file' ? formatBytes(item.bytes) : '—' }}</td>
            <td><span v-if="item.artifact" class="confidence" :class="confidenceTone(item.artifact.confidence)" :title="tr('置信度 {0}%', { 0: item.artifact.confidence })"><span class="ui-progress confidence-bar"><span :style="{ width: `${Math.min(100, Math.max(0, item.artifact.confidence))}%` }"></span></span><span class="mono">{{ item.artifact.confidence }}%</span></span><span v-else class="mono">—</span></td>
            <td>{{ typeLabel(item) }}</td>
          </template>
          <template v-else>
            <td class="mono numeric">{{ item.kind === 'volume' ? formatBytes(volumeBytes(item)) : item.kind === 'file' ? formatBytes(item.bytes) : '—' }}</td>
            <td class="mono">{{ formatTimestamp(item.node?.timestamps.modified) }}</td>
            <td>{{ typeLabel(item) }}</td>
            <td class="cell-status"><span v-for="badge in statusBadges(item)" :key="badge.label" class="ui-badge sm" :class="badge.tone">{{ badge.label }}</span></td>
          </template>
        </tr>
        <tr v-if="paddingBottom > 0" class="virtual-spacer" aria-hidden="true"><td :colspan="6" :style="{ height: `${paddingBottom}px` }"></td></tr>
        <tr v-if="!rows.length" class="empty-row"><td colspan="6"><Loader2 v-if="loading" :size="17" class="spin" /><SearchX v-else :size="18" /><span>{{ emptyText }}</span><button v-if="filtered && !loading" class="ui-btn sm" type="button" @click="emit('clear-filters')">{{ tr("清除筛选") }}</button></td></tr>
      </tbody>
    </table>
    <UiContextMenu
      v-model:open="rowMenu.open"
      :x="rowMenu.x"
      :y="rowMenu.y"
      :entries="rowMenuEntries"
      :label="tr('恢复条目操作')"
      :heading="rowMenu.target?.name"
      :subheading="rowMenu.target ? typeLabel(rowMenu.target) : undefined"
    />
  </div>
</template>
<script setup lang="ts">
import { tr } from '../../../composables/useLocale';
import { computed, ref, watch } from 'vue';
import { AlertTriangle, ClipboardCopy, Copy, FolderOpen, Hash, Loader2, PanelRightOpen, ScanSearch, SearchX, Square, SquareCheck } from 'lucide-vue-next';
import FileNodeIcon from '../../../components/display/FileNodeIcon.vue';
import UiContextMenu, { type UiContextMenuEntry } from '../../../components/ui/UiContextMenu.vue';
import { copyText } from '../../../composables/clipboard';
import { useContextMenu } from '../../../composables/useContextMenu';
import { useVirtualList } from '../../../composables/useVirtualList';
import { compactMenuEntries } from '../../../utils/menuPlacement';
import { formatBytes } from '../../../utils/format';
import { formatTimestamp } from '../../../utils/time';
import { confidenceTone, formatOffset } from '../model/carveView';
import { isRecoverable, type RecoveryEntry } from '../model/recoveryTree';
const props = defineProps<{
  rows: RecoveryEntry[]; signatureColumns: boolean; selectedKey: string | null; contextKey: string;
  isChecked: (item: RecoveryEntry) => boolean; isPartial: (item: RecoveryEntry) => boolean;
  allChecked: boolean; someChecked: boolean; busy: boolean; loading: boolean; filtered: boolean; emptyText: string;
  formatLabel: (formatId: string) => string; sectorSize: number;
}>();
const emit = defineEmits<{ select: [item: RecoveryEntry]; open: [item: RecoveryEntry]; toggle: [item: RecoveryEntry]; 'toggle-all': []; 'clear-filters': [] }>();
const tableWrapRef = ref<HTMLElement | null>(null);
const { visibleItems, paddingTop, paddingBottom, scrollToTop } = useVirtualList({
  items: computed(() => props.rows), itemHeight: 36, containerRef: tableWrapRef, overscan: 10,
});
watch(() => props.contextKey, scrollToTop);

function recoverable(item: RecoveryEntry) {
  return item.kind === 'file' ? isRecoverable(item) : item.fileCount > 0;
}

/** 单击选中；Ctrl+单击勾选或取消，与文件列表一致；双击或 Enter 打开文件夹。 */
function onRowClick(event: MouseEvent, item: RecoveryEntry) {
  if ((event.ctrlKey || event.metaKey) && !props.busy && recoverable(item)) emit('toggle', item);
  else emit('select', item);
}

const { menu: rowMenu, openFromEvent } = useContextMenu<RecoveryEntry>();
function onRowContextMenu(event: MouseEvent, item: RecoveryEntry) {
  emit('select', item);
  openFromEvent(event, item);
}
const rowMenuEntries = computed<UiContextMenuEntry[]>(() => {
  const item = rowMenu.target;
  if (!item) return [];
  const checked = props.isChecked(item);
  const artifact = item.artifact;
  return compactMenuEntries<UiContextMenuEntry>([
    item.kind === 'file'
      ? { id: 'detail', label: tr('查看详情'), icon: PanelRightOpen, action: () => emit('select', item) }
      : { id: 'open', label: tr('打开'), icon: FolderOpen, hint: 'Enter', action: () => emit('open', item) },
    {
      id: 'check', label: checked ? tr('取消勾选此项') : tr('勾选此项'), icon: checked ? Square : SquareCheck, hint: tr('Ctrl+单击'),
      disabled: props.busy || !recoverable(item), action: () => emit('toggle', item),
    },
    { kind: 'separator' },
    { id: 'copy-name', label: tr('复制名称'), icon: Copy, action: () => void copyText(item.name, tr('{0} 个名称', { 0: 1 })) },
    ...(item.path && item.path !== '/' ? [{ id: 'copy-path', label: tr('复制原路径'), icon: ClipboardCopy, action: () => void copyText(item.path, tr('原路径')) }] : []),
    ...(artifact ? [{ id: 'copy-offset', label: tr('复制起始偏移'), icon: Hash, action: () => void copyText(formatOffset(artifact.offset), tr('起始偏移')) }] : []),
  ]);
});
function volumeBytes(item: RecoveryEntry) {
  return (item.volume?.partition.length_lba ?? 0) * props.sectorSize;
}
function typeLabel(item: RecoveryEntry): string {
  switch (item.kind) {
    case 'volume': return item.volume?.partition.kind.toUpperCase() ?? tr("卷");
    case 'signatures': return tr("签名雕刻");
    case 'format': case 'dir': return tr("文件夹");
    case 'file': {
      if (item.artifact) return props.formatLabel(item.artifact.format_id);
      const dot = item.name.lastIndexOf('.');
      return dot > 0 && dot < item.name.length - 1 ? item.name.slice(dot + 1).toUpperCase() : tr("文件");
    }
  }
}
function statusBadges(item: RecoveryEntry): { label: string; tone: string }[] {
  const badges: { label: string; tone: string }[] = [];
  if (item.node?.deleted) badges.push({ label: tr("已删除"), tone: 'danger' });
  if (item.volume) {
    if (item.volume.reconstruction) badges.push({ label: tr("MFT 重建"), tone: 'warn' });
    else if (item.volume.lost) badges.push({ label: tr("丢失分区"), tone: 'warn' });
    else badges.push({ label: tr("已知分区"), tone: 'info' });
    if (item.volume.error) badges.push({ label: tr("读取失败"), tone: 'danger' });
  }
  if (item.artifact) badges.push({ label: tr("签名雕刻"), tone: 'info' });
  if (item.candidate?.source_node) badges.push({ label: tr("回收站还原"), tone: 'info' });
  if (item.candidate && !item.candidate.readable) badges.push({ label: tr("无可读映射"), tone: 'warn' });
  if (item.artifact?.read_fault) badges.push({ label: tr("读取异常"), tone: 'warn' });
  return badges;
}
</script>
<style scoped src="../../../styles/data-table.css"></style>
<style scoped src="../styles/carve-workspace.css"></style>
<style scoped>
.table-wrap { flex: 1; min-height: 0; overflow: auto; background: var(--surface-secondary); }
/* 固定布局：名称列占据其余宽度，窄窗时整表横向滚动，名称至少保留 220px。 */
.grid { table-layout: fixed; min-width: 760px; }
.col-check { width: 36px; }
.col-size { width: 96px; }
.col-time { width: 150px; }
.col-type { width: 88px; }
.col-status { width: 170px; }
.col-offset { width: 168px; }
.col-confidence { width: 112px; }
.cell-check { padding: 0 4px 0 12px; text-overflow: clip; }
.name-content { display: flex; align-items: center; min-width: 0; }
.kind-icon { flex-shrink: 0; margin-right: 8px; }
.signature-icon { color: var(--primary-color); }
.name-text { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
/* 名称与整行一样单击选中、双击打开（与文件列表一致）；文件夹名加字重。 */
.name-text.is-dir { font-weight: 500; }
/* 已删除条目：红字、划线名称与「已删除」标签；行底与选中反馈沿用普通表格规则。 */
.body-row.deleted td { color: var(--sev-critical-fg); }
.body-row.deleted .name-text { color: var(--sev-critical-fg); text-decoration: line-through; opacity: 0.75; }
.dir-count { flex-shrink: 0; min-width: 20px; height: 18px; margin-left: 6px; padding: 0 6px; border-radius: 999px; background: var(--chip-count-bg); color: var(--text-secondary); font-size: 11px; line-height: 18px; text-align: center; }
.read-warning { flex-shrink: 0; margin-left: 6px; color: var(--sev-warn-fg); }
.cell-status .ui-badge + .ui-badge { margin-left: 4px; }
.mono { font-family: var(--font-mono); font-size: 12px; }
.body-row { cursor: default; }
</style>
