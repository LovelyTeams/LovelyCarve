<template>
<div class="table-shell">
<div
      class="table-wrap"
      ref="container"
      tabindex="0"
      :class="{ busy: sortBusyKey !== null, stale }"
      :aria-busy="loading"
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
        :class="[`head-${c.key}`, { numeric: NUMERIC_COLUMNS.has(c.key), sorted: sortKey === c.key }]"
        :aria-sort="sortKey === c.key ? (sortDir === 'asc' ? 'ascending' : 'descending') : 'none'"
      >
        <div class="head-inner">
          <button class="head-sort" type="button" :aria-label="tr('按{0}排序', { 0: tr(c.label) })" @click="emit('sort', c.key)">
            <span class="head-label">{{ tr(c.label) }}</span>
            <Loader2 v-if="sortBusyKey === c.key" :size="12" class="sort-icon spin" aria-hidden="true" />
            <component :is="sortIcon(c.key)" v-else :size="12" class="sort-icon" aria-hidden="true" />
          </button>
          <button
            v-if="c.filterMode !== 'none'"
            class="head-funnel"
            type="button"
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
        </div>
      </th>
    </tr>
  </thead>
  <tbody v-if="!state">
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
          <div v-else-if="c.key === 'name'" class="name-cell">
            <FileNodeIcon
              :id="vi.item.id"
              :kind="vi.item.kind"
              :name="vi.item.name"
              :size="15"
              class="kind-icon"
            />
            <span
              class="name-link"
              :title="fullPathOf(vi.item)"
              :class="{ 'is-dir': vi.item.kind === 'dir' }"
              @click="emit('nameClick', $event, vi.item)"
            >{{ vi.item.name }}</span>
            <span v-if="deepsearch && !pathColumnVisible && parentPathOf(vi.item)" class="name-parent" :title="fullPathOf(vi.item)">{{ parentPathOf(vi.item) }}</span>
            <span
              v-if="vi.item.link_target"
              class="link-target mono"
              :title="tr('符号链接指向：{0}', { 0: vi.item.link_target })"
            >→ {{ vi.item.link_target }}</span>
            <span v-if="vi.item.deleted" class="ui-badge sm danger name-badge">{{ tr("已删除") }}</span>
            <span
              v-if="vi.item.has_snapshot && vi.item.id.startsWith('ntfs:')"
              class="ui-badge sm name-badge snapshot-pill"
              :title="tr('该文件含 NTFS 命名数据流（ADS / Alternate Data Stream），可能隐藏额外内容；下方独立行可查看或导出')"
            >{{ tr("数据流") }}</span>
            <span
              v-else-if="vi.item.has_snapshot"
              class="ui-badge sm name-badge snapshot-pill"
              :title="tr('该文件含 ReFS 文件快照流（refsutil streamsnapshot），保存了替换前的原始内容；下方独立行可查看或导出')"
            >{{ tr("快照") }}</span>
            <span
              v-if="!tagColumnVisible && tagOfNode(vi.item)"
              class="tag-pill mono name-badge"
              :class="`tag-${TAG_STYLE[tagOfNode(vi.item)!]}`"
            >{{ tr(tagOfNode(vi.item) ?? '') }}</span>
            <span
              v-if="vi.item.kind === 'dir' && dirCounts[vi.item.id] != null && dirCounts[vi.item.id] > 0"
              class="ui-count dir-count"
              :title="tr('{0} 项', { 0: dirCounts[vi.item.id].toLocaleString() })"
            >{{ dirCounts[vi.item.id].toLocaleString() }}</span>
          </div>
          <template v-else-if="c.key === 'ext'">
            <span class="mono" :class="{ muted: !extOf(vi.item) }">{{ extOf(vi.item) || '—' }}</span>
          </template>
          <template v-else-if="c.key === 'kind'">
            <span>{{ kindLabel(vi.item, tr) }}</span>
          </template>
          <template v-else-if="c.key === 'size'">
            <span v-if="vi.item.kind === 'dir'" class="muted">—</span>
            <span v-else class="mono">{{ formatBytes(vi.item.size) }}</span>
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
            <span class="mono" :class="{ muted: vi.item.inode == null }">{{ vi.item.inode ?? '—' }}</span>
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
            <span class="mono muted">—</span>
          </template>
        </td>
      </tr>
      <tr v-if="paddingBottom > 0" class="vspacer" aria-hidden="true">
        <td :colspan="visibleColumns.length + 1" :style="{ height: paddingBottom + 'px' }"></td>
      </tr>
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
  <div v-if="loading && !state" class="load-bar" aria-hidden="true"><span></span></div>
  <!-- 状态盖在表头下方的可视区域中央：横向滚动的宽表格里也不会跑出视口。 -->
  <div v-if="state" class="table-state">
    <div v-if="state === 'loading'" class="ui-empty" role="status">
      <span class="ui-empty-icon accent"><Loader2 :size="22" class="spin" /></span>
      <p class="ui-empty-desc">{{ deepsearch ? tr("正在查询所有子目录中的文件…") : tr("正在加载目录…") }}</p>
    </div>
    <div v-else-if="state === 'error'" class="ui-empty error" role="alert">
      <span class="ui-empty-icon"><CircleAlert :size="24" /></span>
      <p class="ui-empty-title">{{ tr("目录读取失败") }}</p>
      <p class="ui-empty-desc mono">{{ error }}</p>
      <div class="ui-empty-actions"><button class="ui-btn" type="button" @click="emit('retry')"><RefreshCw :size="14" />{{ tr("重试") }}</button></div>
    </div>
    <!-- 有条件时绝不说成「空目录」：取证列表必须区分「被筛掉」与「本来就空」。 -->
    <div v-else-if="state === 'filtered'" class="ui-empty" role="status">
      <span class="ui-empty-icon"><FilterX :size="24" /></span>
      <p class="ui-empty-title">{{ tr("没有符合条件的条目") }}</p>
      <p class="ui-empty-desc">{{ deepsearch ? tr("当前目录及所有子目录中没有文件符合关键词或筛选条件。") : tr("当前目录中的条目都被筛选条件隐藏了。") }}</p>
      <div class="ui-empty-actions"><button class="ui-btn" type="button" @click="emit('clearFilters')">{{ tr("清除筛选") }}</button></div>
    </div>
    <div v-else class="ui-empty" role="status">
      <span class="ui-empty-icon"><FolderOpen :size="24" /></span>
      <p class="ui-empty-title">{{ deepsearch ? tr("子目录中没有文件") : tr("空目录") }}</p>
      <p class="ui-empty-desc">{{ deepsearch ? tr("当前目录及所有子目录中没有任何文件。") : tr("此目录中没有任何条目。") }}</p>
    </div>
  </div>
</div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, onMounted, onBeforeUnmount, useTemplateRef } from 'vue';
import { ArrowDown, ArrowUp, ArrowUpDown, CircleAlert, Filter, FilterX, FolderOpen, Loader2, RefreshCw } from 'lucide-vue-next';
import FileNodeIcon from '../../../components/display/FileNodeIcon.vue';
import { extOf, kindLabel, formatBytes, rowKeyOf, type ColumnDef, type DiskFsNode } from '../model/model';
import { FLAT_SERVER_FILTER_COLUMNS } from '../model/flatQuery';
import { ftypeCellText, ftypeTitle } from '../model/derivedData';
import { TAG_STYLE, type FileTag } from '../model/tags';
import { formatTimestamp } from '../../../utils/time';
import type { FsTypeIdent, StoredDigest } from '../../../lib/ipc/fs/derived';

const props = defineProps<{
  visibleColumns: ColumnDef[];
  visibleItems: { item: DiskFsNode; index: number }[];
  rowCount: number;
  loading: boolean;
  /** A keyword, column condition, category or hidden empty folders narrows the list. */
  filtered: boolean;
  error: string | null;
  deepsearch: boolean;
  currentDirectoryPath: string;
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
  clearFilters: [];
  retry: [];
}>();
const container = useTemplateRef<HTMLElement>('container');
onMounted(() => emit('containerChange', container.value));
onBeforeUnmount(() => emit('containerChange', null));

const NUMERIC_COLUMNS = new Set(['index', 'size']);

/** Reloading a folder keeps its previous rows, dimmed, until the new page
 *  arrives; recursive queries and empty lists show a state instead. */
const stale = computed(() => props.loading && props.rowCount > 0 && !props.deepsearch);
const state = computed<'loading' | 'error' | 'filtered' | 'empty' | null>(() => {
  if (props.error && !props.loading) return 'error';
  if (props.rowCount > 0 && !(props.loading && props.deepsearch)) return null;
  if (props.loading) return 'loading';
  return props.filtered ? 'filtered' : 'empty';
});
const pathColumnVisible = computed(() => props.visibleColumns.some(column => column.key === 'path'));
const tagColumnVisible = computed(() => props.visibleColumns.some(column => column.key === 'tag'));

function sortIcon(key: string) {
  if (props.sortKey !== key) return ArrowUpDown;
  return props.sortDir === 'asc' ? ArrowUp : ArrowDown;
}

/** Folder of a recursive-listing row relative to the browsed folder ('' when directly inside). */
function parentPathOf(node: DiskFsNode): string {
  const fullPath = props.fullPathOf(node);
  const cut = fullPath.lastIndexOf('/');
  const parent = cut > 0 ? fullPath.slice(0, cut) : '';
  const base = props.currentDirectoryPath === '/' ? '' : props.currentDirectoryPath;
  const relative = parent.startsWith(`${base}/`) ? parent.slice(base.length + 1) : parent === base ? '' : parent;
  return relative ? `${relative}/` : '';
}
</script>

<style scoped src="../../../styles/data-table.css"></style>
<style scoped src="../styles/file-tags.css"></style>
<style scoped>
.table-shell {
  position: relative;
  flex: 1;
  display: flex;
  flex-direction: column;
  min-height: 0;
}

.grid { table-layout: fixed; }

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

/* 重新加载目录：旧行留在原处淡化，等新一页到达再整体替换，避免整表闪白。 */
.table-wrap.stale tbody {
  opacity: 0.5;
  pointer-events: none;
  transition: opacity var(--transition-normal);
}

.load-bar {
  position: absolute;
  top: 32px;
  left: 0;
  right: 0;
  z-index: 3;
  height: 2px;
  overflow: hidden;
  pointer-events: none;
}

.load-bar > span {
  position: absolute;
  top: 0;
  bottom: 0;
  width: 30%;
  background: var(--primary-color);
  animation: load-bar-slide 1.1s ease-in-out infinite;
}

@keyframes load-bar-slide {
  from { left: -30%; }
  to { left: 100%; }
}

:root[data-motion='reduced'] .load-bar > span {
  left: 0;
  width: 100%;
  opacity: 0.45;
  animation: none;
}

.table-state {
  position: absolute;
  inset: 34px 0 0;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 16px;
  overflow: auto;
  pointer-events: none;
}

.table-state .ui-empty {
  max-width: 420px;
  pointer-events: auto;
  overflow-wrap: anywhere;
}

.table-state .ui-empty.error .ui-empty-icon {
  background: var(--sev-critical-bg);
  color: var(--sev-critical-fg);
}

.table-state .ui-empty.error .ui-empty-desc {
  color: var(--sev-critical-fg);
  font-size: 12px;
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

.muted {
  color: var(--text-secondary);
}

/* ── 表头：标签即排序按钮，未排序时只在悬停显示排序图标；漏斗靠列边缘、平时淡显 ── */
.head-cell {
  user-select: none;
}

.head-inner {
  display: flex;
  align-items: center;
  gap: 4px;
  min-width: 0;
}

.head-sort {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  flex: 1;
  min-width: 0;
  height: 24px;
  padding: 0;
  border: 0;
  background: transparent;
  color: inherit;
  font: inherit;
  text-align: left;
  cursor: pointer;
  border-radius: var(--radius-sm);
}

.head-label {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
}

.head-sort:hover {
  color: var(--text-primary);
}

.head-sort:focus-visible {
  outline: 2px solid var(--primary-color);
  outline-offset: 2px;
}

.sort-icon {
  flex-shrink: 0;
  color: var(--text-secondary);
  opacity: 0;
  transition: opacity var(--transition-fast);
}

.head-cell:hover .sort-icon,
.head-sort:focus-visible .sort-icon {
  opacity: 0.7;
}

.head-cell.sorted {
  color: var(--text-primary);
}

.head-cell.sorted .sort-icon,
.head-cell .sort-icon.spin {
  color: var(--primary-color);
  opacity: 1;
}

/* 数值列右对齐：标签贴右与数值对齐，漏斗换到左侧。 */
.head-cell.numeric .head-inner {
  flex-direction: row-reverse;
}

.head-cell.numeric .head-sort {
  flex-direction: row-reverse;
  justify-content: flex-start;
  text-align: right;
}

/* 漏斗平时隐藏，指针进入表头时整行淡入、所在列全亮；有条件的列常亮。 */
.head-funnel {
  display: inline-grid;
  place-items: center;
  flex-shrink: 0;
  width: 22px;
  height: 22px;
  padding: 0;
  background: transparent;
  border: none;
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
  cursor: pointer;
  opacity: 0;
  transition:
    color var(--transition-fast),
    background var(--transition-fast),
    opacity var(--transition-fast);
}

thead:hover .head-funnel {
  opacity: 0.55;
}

.head-cell:hover .head-funnel,
.head-funnel:focus-visible {
  opacity: 1;
}

.head-funnel:hover:not(:disabled) {
  color: var(--primary-color);
  background: var(--hover-bg);
}

.head-funnel:focus-visible {
  outline: 2px solid var(--primary-color);
  outline-offset: -2px;
}

/* 全显下不能服务端过滤的列：漏斗留在原位但灰掉，避免「只过滤已加载页」的假象。 */
thead:hover .head-funnel:disabled,
.head-cell:hover .head-funnel:disabled {
  opacity: 0.25;
  cursor: not-allowed;
}

/* 该列有生效的过滤条件：漏斗填充并染上主题色，一眼可辨。 */
.head-funnel.active,
thead:hover .head-funnel.active {
  color: var(--primary-color);
  background: var(--sidebar-active-bg);
  opacity: 1;
}

.head-funnel.active svg {
  fill: currentColor;
}

/* ── Table ── */
/* 固定布局：列宽由 colgroup 决定而不是可见行内容——虚拟滚动整批换行时
   列宽不再随长文件名/长路径左右抖动，浏览器也省去每批的布局推导。 */

.col-check { width: 34px; }
.col-index { width: 64px; }
/* 名称/全路径也给显式宽度：fixed 布局下 auto 列只分得「正剩余」，固定列
   填满容器时 auto 列会塌成 0px（哈希列自动亮出后必然发生）。全列定宽 +
   width:100% 时浏览器按比例分配富余，窄窗走容器的横向滚动。默认四列的宽度
   与目录树一起不超过预览栏留出的 880px（FilePreviewDock）。 */
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
  border-color: color-mix(in srgb, var(--sev-snapshot-border) 50%, transparent);
  color: var(--sev-snapshot-tag-fg);
  background: var(--sev-snapshot-tag-bg);
}

/* 已删除条目通过各列红字、划线名称与「已删除」标签标识；
   行底色、悬停与选中反馈沿用普通表格规则。 */
.body-row.deleted .cell {
  color: var(--sev-critical-fg);
}

.body-row.deleted .name-link {
  color: inherit;
  text-decoration: line-through;
  text-decoration-color: color-mix(in srgb, var(--sev-critical-fg) 70%, transparent);
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

/* 名称单元格：名称先省略，状态徽章与计数保持完整。 */
.name-cell {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

/* 图标颜色由 FileNodeIcon 自带（与目录树共用一份）；这里只管布局。 */
.kind-icon {
  flex-shrink: 0;
}

/* 目录名可点击进入：常态只加字重，悬停才出现下划线提示。 */
.name-link {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
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

/* 递归列出时，名称后淡显它相对当前目录所在的子目录。 */
.name-parent {
  flex-shrink: 1;
  min-width: 24px;
  overflow: hidden;
  text-overflow: ellipsis;
  color: var(--text-secondary);
  font-size: 12px;
}

.name-badge,
.dir-count {
  flex-shrink: 0;
}

.dir-count {
  margin-left: auto;
}

/* 符号链接指向，跟在名称后弱化显示；目标可能很长，截断并靠 title 看全 */
.link-target {
  flex-shrink: 1;
  min-width: 24px;
  overflow: hidden;
  text-overflow: ellipsis;
  color: var(--text-secondary);
  font-size: 11px;
}

.cell-path .path-cell {
  color: var(--text-secondary);
}
</style>
