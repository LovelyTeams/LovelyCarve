<template>
<div
      class="table-wrap"
      ref="container"
      tabindex="0"
      :class="{ busy: sortBusyKey !== null }"
      @mousedown="emit('marqueeStart', $event)"
      @keydown="emit('keydown', $event)"
    >
  <table class="data-table grid">
  <colgroup>
    <col class="col-check" />
    <col v-for="c in visibleColumns" :key="c.key" :class="`col-${c.key}`" />
  </colgroup>
  <thead>
    <tr class="head-row">
      <th class="cell-check">
        <input
          type="checkbox"
          :aria-label="tr('全选文件列表条目')"
          :checked="headerChecked"
          :indeterminate="headerIndeterminate"
          @change="emit('toggleAll')"
        />
      </th>
      <th
        v-for="c in visibleColumns"
        :key="c.key"
        class="head-cell"
        :class="`head-${c.key}`"
        :aria-sort="sortKey === c.key ? (sortDir === 'asc' ? 'ascending' : 'descending') : 'none'"
      >
        <button
          v-if="c.filterMode !== 'none'"
          class="head-funnel"
          :aria-label="tr('过滤{0}', { 0: tr(c.label) })"
          :class="{ active: isColumnFiltered(c.key) }"
          :disabled="deepsearch && !FLAT_SERVER_FILTER_COLUMNS.has(c.key)"
          :title="deepsearch && !FLAT_SERVER_FILTER_COLUMNS.has(c.key)
            ? tr('「{0}」的数据来自独立存档，文件全显的分页查询无法按它过滤', { 0: tr(c.label) })
            : isColumnFiltered(c.key) ? tr('过滤「{0}」：{1}', { 0: tr(c.label), 1: filterSummaryOf(c.key) }) : tr('过滤「{0}」', { 0: tr(c.label) })"
          @click.stop="emit('filter', $event, c)"
        >
          <Filter :size="12" />
        </button>
        <button class="head-label" :aria-label="tr('按{0}排序', { 0: tr(c.label) })" @click="emit('sort', c.key)">{{ tr(c.label) }}</button>
        <Loader2 v-if="sortBusyKey === c.key" :size="12" class="head-sort spin" />
        <ArrowUp v-else-if="sortKey === c.key && sortDir === 'asc'" :size="12" class="head-sort" />
        <ArrowDown v-else-if="sortKey === c.key && sortDir === 'desc'" :size="12" class="head-sort" />
      </th>
    </tr>
  </thead>
  <tbody>
    <tr v-if="loading">
      <td class="state-row" :colspan="visibleColumns.length + 1">
        <template v-if="deepsearch">
          <Loader2 :size="12" class="spin" />
          <span>{{ tr("正在查询全显结果…") }}</span>
        </template>
        <template v-else>{{ tr("加载中…") }}</template>
      </td>
    </tr>
    <tr v-else-if="rowCount === 0">
      <td class="state-row" :colspan="visibleColumns.length + 1">
        <!-- 有条目但被过滤光 ≠ 空目录：取证列表绝不能把「被过滤」说成「空」。 -->
        {{ loadedCount > 0 ? tr("无匹配条目（已过滤）") : deepsearch ? tr("无匹配文件") : tr("空目录") }}
      </td>
    </tr>
    <template v-else>
      <tr v-if="paddingTop > 0" class="vspacer" aria-hidden="true">
        <td :colspan="visibleColumns.length + 1" :style="{ height: paddingTop + 'px' }"></td>
      </tr>
      <tr
        v-for="vi in visibleItems"
        :key="vi.item.rowKey ?? vi.item.id"
        class="body-row disk-file-row"
        :class="{
          deleted: vi.item.deleted,
          'has-snapshot': vi.item.has_snapshot && vi.item.id.startsWith('refs:'),
          'snapshot-row': vi.item.id.startsWith('refs-snap:'),
        }"
        @click="emit('rowClick', vi.item)"
        @dblclick="emit('rowDoubleClick', vi.item)"
        @mousedown="emit('rowMouseDown', $event)"
        @contextmenu.prevent.stop="emit('contextMenu', $event, vi.item)"
        :data-selected="selectedKey === rowKeyOf(vi.item) || null"
      >
        <td class="cell-check">
          <input
            type="checkbox"
            :aria-label="tr('勾选 {0}', { 0: vi.item.name })"
            :checked="checkedIds.has(vi.item.id)"
            @click.stop="emit('checkClick', $event, vi.item.id)"
            @dblclick.stop
          />
        </td>
        <td v-for="c in visibleColumns" :key="c.key" :class="`cell cell-${c.key}`">
          <template v-if="c.key === 'index'">
            <span class="mono">{{ vi.index + 1 }}</span>
          </template>
          <template v-else-if="c.key === 'name'">
            <FileNodeIcon
              :id="vi.item.id"
              :kind="vi.item.kind"
              :name="vi.item.name"
              :size="14"
              class="kind-icon"
            />
            <span
              class="name-link"
              :title="fullPathOf(vi.item)"
              :class="{ 'is-dir': vi.item.kind === 'dir' }"
              @click="emit('nameClick', $event, vi.item)"
            >{{ vi.item.name }}</span>
            <span v-if="vi.item.deleted" class="deleted-label">{{ tr("已删除") }}</span>
            <span
              v-if="vi.item.link_target"
              class="link-target mono"
              :title="tr('符号链接指向：{0}', { 0: vi.item.link_target })"
            >→ {{ vi.item.link_target }}</span>
            <span
              v-if="vi.item.has_snapshot && vi.item.id.startsWith('ntfs:')"
              class="tag-pill mono snapshot-pill"
              :title="tr('该文件含 NTFS 命名数据流（ADS / Alternate Data Stream），可能隐藏额外内容；下方独立行可查看或导出')"
            >{{ tr("数据流") }}</span>
            <span
              v-else-if="vi.item.has_snapshot"
              class="tag-pill mono snapshot-pill"
              :title="tr('该文件含 ReFS 文件快照流（refsutil streamsnapshot），保存了替换前的原始内容；下方独立行可查看或导出')"
            >{{ tr("快照") }}</span>
            <span
              v-if="vi.item.kind === 'dir' && dirCounts[vi.item.id] != null && dirCounts[vi.item.id] > 0"
              class="dir-count mono"
            >{{ dirCounts[vi.item.id] }}</span>
          </template>
          <template v-else-if="c.key === 'ext'">
            <span class="mono">{{ extOf(vi.item) || '—' }}</span>
          </template>
          <template v-else-if="c.key === 'kind'">
            <span>{{ kindLabel(vi.item, tr) }}</span>
          </template>
          <template v-else-if="c.key === 'size'">
            <span class="mono">{{ vi.item.kind === 'dir' ? '—' : formatBytes(vi.item.size) }}</span>
          </template>
          <template v-else-if="c.key === 'created'">
            <span class="mono">{{ formatTimestamp(vi.item.timestamps?.created || vi.item.timestamps?.birth) }}</span>
          </template>
          <template v-else-if="c.key === 'accessed'">
            <span class="mono">{{ formatTimestamp(vi.item.timestamps?.accessed) }}</span>
          </template>
          <template v-else-if="c.key === 'modified'">
            <span class="mono">{{ formatTimestamp(vi.item.timestamps?.modified) }}</span>
          </template>
          <template v-else-if="c.key === 'inode'">
            <span class="mono">{{ vi.item.inode ?? '—' }}</span>
          </template>
          <template v-else-if="c.key === 'path'">
            <span class="mono path-cell" :title="fullPathOf(vi.item)">{{ fullPathOf(vi.item) }}</span>
          </template>
          <template v-else-if="c.key === 'link'">
            <span class="mono" :title="vi.item.link_target ?? undefined">{{ vi.item.link_target ?? '' }}</span>
          </template>
          <template v-else-if="c.key === 'md5'">
            <span class="mono">{{ digestCache.get(vi.item.id)?.md5 ?? '' }}</span>
          </template>
          <template v-else-if="c.key === 'sha1'">
            <span class="mono">{{ digestCache.get(vi.item.id)?.sha1 ?? '' }}</span>
          </template>
          <template v-else-if="c.key === 'sha256'">
            <span class="mono">{{ digestCache.get(vi.item.id)?.sha256 ?? '' }}</span>
          </template>
          <template v-else-if="c.key === 'sm3'">
            <span class="mono">{{ digestCache.get(vi.item.id)?.sm3 ?? '' }}</span>
          </template>
          <template v-else-if="c.key === 'tag'">
            <span
              v-if="tagOfNode(vi.item)"
              class="tag-pill mono"
              :class="`tag-${TAG_STYLE[tagOfNode(vi.item)!]}`"
            >{{ tr(tagOfNode(vi.item) ?? '') }}</span>
          </template>
          <template v-else-if="c.key === 'ftype'">
            <span
              v-if="ftypeCache.get(vi.item.id)"
              class="mono"
              :title="ftypeTitle(ftypeCache.get(vi.item.id)!, tr)"
            >{{ ftypeCellText(ftypeCache.get(vi.item.id)!) }}</span>
          </template>
          <template v-else-if="c.key === 'deleted'">
            <span>{{ vi.item.deleted ? tr("是") : tr("否") }}</span>
          </template>
          <template v-else>
            <span class="mono">—</span>
          </template>
        </td>
      </tr>
      <tr v-if="paddingBottom > 0" class="vspacer" aria-hidden="true">
        <td :colspan="visibleColumns.length + 1" :style="{ height: paddingBottom + 'px' }"></td>
      </tr>
    </template>
  </tbody>
  </table>
  <div
    v-if="marqueeRect"
    class="marquee-box"
    :style="{
      left: marqueeRect.left + 'px',
      top: marqueeRect.top + 'px',
      width: marqueeRect.width + 'px',
      height: marqueeRect.height + 'px',
    }"
  ></div>
</div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { onMounted, onBeforeUnmount, useTemplateRef } from 'vue';
import { Filter, Loader2, ArrowUp, ArrowDown } from 'lucide-vue-next';
import FileNodeIcon from '../../../components/display/FileNodeIcon.vue';
import { extOf, kindLabel, formatBytes, rowKeyOf, type ColumnDef, type DiskFsNode } from '../model/model';
import { FLAT_SERVER_FILTER_COLUMNS } from '../model/flatQuery';
import { ftypeCellText, ftypeTitle } from '../model/derivedData';
import { TAG_STYLE, type FileTag } from '../model/tags';
import { formatTimestamp } from '../../../utils/time';
import type { FsTypeIdent, StoredDigest } from '../../../lib/ipc/fs/derived';

defineProps<{
  visibleColumns: ColumnDef[];
  visibleItems: { item: DiskFsNode; index: number }[];
  rowCount: number;
  loadedCount: number;
  loading: boolean;
  deepsearch: boolean;
  sortBusyKey: string | null;
  sortKey: string | null;
  sortDir: 'asc' | 'desc';
  headerChecked: boolean;
  headerIndeterminate: boolean;
  paddingTop: number;
  paddingBottom: number;
  selectedKey: string | null;
  checkedIds: Set<string>;
  dirCounts: Record<string, number>;
  digestCache: Map<string, StoredDigest>;
  ftypeCache: Map<string, FsTypeIdent>;
  marqueeRect: { left: number; top: number; width: number; height: number } | null;
  fullPathOf: (node: DiskFsNode) => string;
  tagOfNode: (node: DiskFsNode) => FileTag | undefined;
  isColumnFiltered: (key: string) => boolean;
  filterSummaryOf: (key: string) => string;
}>();
const emit = defineEmits<{
  containerChange: [container: HTMLElement | null];
  marqueeStart: [event: MouseEvent];
  keydown: [event: KeyboardEvent];
  toggleAll: [];
  sort: [key: string];
  filter: [event: MouseEvent, column: ColumnDef];
  rowClick: [node: DiskFsNode];
  rowDoubleClick: [node: DiskFsNode];
  rowMouseDown: [event: MouseEvent];
  contextMenu: [event: MouseEvent, node: DiskFsNode];
  nameClick: [event: MouseEvent, node: DiskFsNode];
  checkClick: [event: MouseEvent, id: string];
}>();
const container = useTemplateRef<HTMLElement>('container');
onMounted(() => emit('containerChange', container.value));
onBeforeUnmount(() => emit('containerChange', null));
</script>

<style scoped src="../../../styles/data-table.css"></style>
<style scoped src="../styles/file-tags.css"></style>
<style scoped>
.grid { table-layout: fixed; }
.head-cell { user-select: none; }
.head-label {
  padding: 0; border: 0; background: transparent;
  color: inherit; font: inherit; cursor: pointer;
  border-radius: var(--radius-sm);
}
.head-label:hover { color: var(--primary-color); }
.head-label:focus-visible { outline: 2px solid var(--primary-color); outline-offset: 2px; }

.table-wrap {
  flex: 1;
  overflow: auto;
  background: var(--surface-secondary);
  min-height: 120px;
  /* 框选矩形（.marquee-box）的定位上下文：绝对定位子元素按内容坐标随滚动走 */
  position: relative;
  outline: none;
}

/* tabindex=0 的容器：键盘聚焦时给一圈内嵌提示，鼠标点击聚焦不显示。 */
.table-wrap:focus-visible {
  outline: 1px solid var(--primary-color);
  outline-offset: -1px;
}

.table-wrap.busy {
  cursor: progress;
}

.marquee-box {
  position: absolute;
  /* 盖在行上，但压不过 sticky 表头（z-index: 2） */
  z-index: 1;
  pointer-events: none;
  border: 1px solid var(--primary-color);
  background: color-mix(in srgb, var(--primary-color) 14%, transparent);
  border-radius: 2px;
}

.mono {
  font-size: 12px;
}

/* ── Table ── */
/* 固定布局：列宽由 colgroup 决定而不是可见行内容——虚拟滚动整批换行时
   列宽不再随长文件名/长路径左右抖动，浏览器也省去每批的布局推导。 */

.col-check { width: 34px; }
.col-index { width: 64px; }
/* 名称/全路径也给显式宽度：fixed 布局下 auto 列只分得「正剩余」，固定列
   填满容器时 auto 列会塌成 0px（哈希列自动亮出后必然发生）。全列定宽 +
   width:100% 时浏览器按比例分配富余，窄窗走容器的横向滚动。 */
.col-name { width: 240px; }
.col-path { width: 400px; }
.col-ext { width: 80px; }
.col-kind { width: 96px; }
.col-size { width: 100px; }
.col-created,
.col-accessed,
.col-modified { width: 156px; }
.col-link { width: 200px; }
.col-inode { width: 120px; }
.col-deleted { width: 72px; }
.col-md5 { width: 250px; }
.col-sha1 { width: 300px; }
.col-sha256 { width: 320px; }
.col-sm3 { width: 320px; }
.col-hashtype { width: 100px; }
.col-tag { width: 120px; }
.col-encryption { width: 100px; }

.cell-check {
  width: 34px;
  padding: 0 4px 0 12px;
  text-align: left;
  vertical-align: middle;
}

.head-funnel {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  margin-right: 4px;
  padding: 0;
  vertical-align: -4px;
  background: transparent;
  border: none;
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
  cursor: pointer;
  opacity: 0.7;
  transition:
    color var(--transition-fast),
    background var(--transition-fast),
    opacity var(--transition-fast);
}

.head-cell:hover .head-funnel,
.head-funnel:focus-visible {
  opacity: 1;
}

.head-funnel:hover:not(:disabled) {
  color: var(--primary-color);
  background: var(--hover-bg);
}

/* 全显下不能服务端过滤的列：漏斗留在原位但灰掉，避免「只过滤已加载页」的假象。 */
.head-funnel:disabled {
  opacity: 0.35;
  cursor: not-allowed;
}

/* 该列有生效的过滤条件：漏斗填充并染上主题色，一眼可辨。 */
.head-funnel.active {
  color: var(--primary-color);
  opacity: 1;
}

.head-funnel.active svg {
  fill: currentColor;
}

.head-sort {
  margin-left: 6px;
  color: var(--primary-color);
  vertical-align: -1px;
}

/* ── 取证状态：快照底色与删除标识 ── */

/* ReFS 文件快照及其所属文件使用琥珀色行底。选中时保留行底，
   选中由 data-table.css 的描边标出；悬停时加深。
   NTFS 命名数据流（ADS）及其所属文件只用图标与「数据流」标签标识，沿用普通行底色。 */
.body-row.has-snapshot,
.body-row.snapshot-row,
.body-row.has-snapshot[data-selected],
.body-row.snapshot-row[data-selected] {
  background: var(--sev-snapshot-bg);
}

.body-row.has-snapshot:hover,
.body-row.snapshot-row:hover {
  background: var(--sev-snapshot-bg-hover);
}

.snapshot-pill {
  margin-left: 6px;
  color: var(--sev-snapshot-tag-fg);
  background: var(--sev-snapshot-tag-bg);
}

/* 已删除条目通过各列红字、划线名称与「已删除」标签标识；
   行底色、悬停与选中反馈沿用普通表格规则。 */
.body-row.deleted .cell {
  color: var(--sev-critical-fg);
}

.body-row.deleted .name-link {
  text-decoration: line-through;
  opacity: 0.75;
}

.deleted-label {
  margin-left: 8px;
  padding: 1px 5px;
  border-radius: var(--radius-sm);
  color: var(--sev-critical-fg);
  background: var(--sev-critical-bg);
  border: 1px solid color-mix(in srgb, var(--sev-critical-border) 60%, transparent);
  font-size: 11px;
}

/* 数值列右对齐 + 等宽数字，'3.52 MB' 与 '128 B' 可纵向比对数量级。 */
.cell-index,
.cell-size,
.head-index,
.head-size {
  text-align: right;
  font-variant-numeric: tabular-nums;
}

.cell-index {
  color: var(--text-secondary);
}

.cell-name {
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 图标颜色由 FileNodeIcon 自带（与目录树共用一份）；这里只管布局。 */
.kind-icon {
  display: inline-block;
  width: 18px;
  margin-right: 8px;
  vertical-align: -3px;
}

/* 目录名可点击进入：常态只加字重，悬停才出现下划线提示。 */
.name-link {
  color: var(--text-primary);
  font-weight: 500;
  cursor: pointer;
  text-decoration: underline;
  text-decoration-color: transparent;
  text-underline-offset: 3px;
  transition: color var(--transition-fast), text-decoration-color var(--transition-fast);
}

.name-link:hover {
  color: var(--primary-color);
  text-decoration-color: color-mix(in srgb, var(--primary-color) 55%, transparent);
}

.name-link:not(.is-dir) {
  cursor: default;
  font-weight: 400;
  text-decoration: none;
  color: var(--text-primary);
}

.dir-count {
  display: inline-block;
  min-width: 20px;
  height: 18px;
  margin-left: 6px;
  padding: 0 6px;
  border-radius: 999px;
  background: var(--chip-count-bg);
  color: var(--text-secondary);
  font-size: 11px;
  line-height: 18px;
  text-align: center;
  vertical-align: 1px;
}

/* 符号链接指向，跟在名称后弱化显示；目标可能很长，截断并靠 title 看全 */
.link-target {
  margin-left: 6px;
  color: var(--text-secondary);
  font-size: 11px;
  display: inline-block;
  max-width: 320px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  vertical-align: bottom;
}

.cell-path .path-cell {
  color: var(--text-secondary);
}

.state-row {
  padding: 40px;
  text-align: center;
  color: var(--text-secondary);
}

.state-row .spin {
  vertical-align: -2px;
  margin-right: 6px;
}

</style>
