<template>
<div
  ref="surface"
  class="treemap-surface"
  tabindex="0"
  role="group"
  :aria-roledescription="tr('树图')"
  :aria-label="tr('空间树图：{0}', { 0: directoryName })"
  @pointermove="onPointerMove"
  @pointerleave="onPointerLeave"
  @click="onClick"
  @dblclick="onDoubleClick"
  @contextmenu="onContextMenu"
  @mouseup="onMouseUp"
  @wheel="onWheel"
  @keydown="onKeydown"
>
  <canvas ref="canvas" class="treemap-canvas" aria-hidden="true"></canvas>
  <template v-if="layout.length">
    <div v-if="card" class="tile-card" role="tooltip" :style="card.style">
      <p class="card-name" :class="{ deleted: card.deleted }">{{ card.name }}</p>
      <p v-if="card.path" class="card-path">{{ card.path }}</p>
      <dl class="card-facts">
        <div><dt>{{ tr("大小") }}</dt><dd>{{ card.size }}<span class="card-share">{{ card.share }}</span></dd></div>
        <div v-if="card.files !== null"><dt>{{ tr("文件") }}</dt><dd>{{ card.files }}</dd></div>
        <div v-if="card.dirs !== null"><dt>{{ tr("子目录") }}</dt><dd>{{ card.dirs }}</dd></div>
        <div v-if="card.newest"><dt>{{ tr("最新修改") }}</dt><dd class="mono">{{ card.newest }}</dd></div>
      </dl>
      <span v-if="card.deleted" class="ui-badge sm danger">{{ tr("已删除") }}</span>
    </div>
    <span v-if="status === 'loading'" class="treemap-busy" :title="tr('正在汇总空间占用…')"><Loader2 :size="14" class="spin" /></span>
  </template>
  <div v-else-if="status === 'unsupported'" class="ui-empty treemap-state">
    <span class="ui-empty-icon"><LayoutGrid :size="26" /></span>
    <p class="ui-empty-title">{{ tr("树图仅支持 NTFS 卷") }}</p>
    <p class="ui-empty-desc">{{ tr("当前位置不是 NTFS 目录。进入 NTFS 分区即可查看各目录的空间占用。") }}</p>
    <div class="ui-empty-actions">
      <button class="ui-btn sm" type="button" @click="emit('showList')"><List :size="14" />{{ tr("切换到列表") }}</button>
    </div>
  </div>
  <div v-else-if="status === 'unindexed'" class="ui-empty treemap-state">
    <span class="ui-empty-icon"><Camera :size="26" /></span>
    <p class="ui-empty-title">{{ tr("卷索引尚未覆盖此目录") }}</p>
    <p class="ui-empty-desc">{{ tr("树图依据卷快照建立的索引汇总空间占用，快照构建完成后自动显示；已删除的目录不在索引中。") }}</p>
    <span v-if="snapshotLabel" class="ui-badge sm info"><Loader2 :size="11" class="spin" />{{ snapshotLabel }}</span>
  </div>
  <div v-else-if="status === 'error'" class="ui-empty treemap-state" role="alert">
    <span class="ui-empty-icon"><CircleAlert :size="26" /></span>
    <p class="ui-empty-title">{{ tr("无法汇总空间占用") }}</p>
    <p class="ui-empty-desc">{{ error }}</p>
    <div class="ui-empty-actions">
      <button class="ui-btn sm" type="button" @click="emit('retry')"><RefreshCw :size="14" />{{ tr("重试") }}</button>
    </div>
  </div>
  <div v-else-if="status === 'ready' && measured" class="ui-empty treemap-state">
    <span class="ui-empty-icon"><LayoutGrid :size="26" /></span>
    <p class="ui-empty-title">{{ tr("此目录没有可显示的内容") }}</p>
    <p class="ui-empty-desc">{{ tr("目录为空，或其中的文件大小都为 0。") }}</p>
  </div>
  <div v-else-if="status !== 'idle'" class="ui-empty treemap-state" role="status">
    <Loader2 :size="20" class="spin" />
    <p class="ui-empty-desc">{{ tr("正在汇总空间占用…") }}</p>
  </div>
  <p class="sr-only" aria-live="polite">{{ announcement }}</p>
  <UiContextMenu
    v-model:open="backgroundMenu.open"
    :x="backgroundMenu.x"
    :y="backgroundMenu.y"
    :entries="backgroundEntries"
    :label="tr('树图操作')"
    :heading="directoryName"
    :subheading="rootSizeText"
    :heading-icon="Folder"
  />
</div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue';
import { Camera, CircleAlert, ClipboardCopy, CornerLeftUp, Folder, LayoutGrid, List, ListMinus, ListPlus, Loader2, RefreshCw, ZoomOut } from 'lucide-vue-next';
import UiContextMenu, { type UiContextMenuEntry } from '../../../components/ui/UiContextMenu.vue';
import { isContextMenuKey, useContextMenu } from '../../../composables/useContextMenu';
import { useTheme } from '../../../composables/useTheme';
import { useUiMotion } from '../../../composables/useUiMotion';
import type { FsSizeTreeEntry, FsSizeTreeView } from '../../../lib/ipc/fs/sizeTree';
import type { SizeTreeStatus, TreemapMode } from '../composables/useSizeTree';
import { formatBytes, type DiskFsNode } from '../model/model';
import type { FileNavigationFrame } from '../model/navigation';
import {
  DEFAULT_TREEMAP_OPTIONS, TREEMAP_MAX_DEPTH, TREEMAP_MIN_DEPTH, hitTile, layoutTreemap, neighbourTile, tileBody, tileChain,
  type Rect, type TreemapDirection, type TreemapTile,
} from '../model/treemap';
import { ageBucketOf, formatUnixSeconds } from '../model/treemapAge';
import {
  IDENTITY_VIEW, KEY_ZOOM_FACTOR, WHEEL_ZOOM_FACTOR, clampView, layoutTransition, panView, projectRect, unprojectPoint,
  visibleBase, wheelLines, zoomStep, type LayoutTransition, type TreemapView, type ZoomStep,
} from '../model/treemapView';
import { paintTreemap, resolvePalette, type PaintTile, type TreemapPalette } from './treemapCanvas';

const props = defineProps<{
  view: FsSizeTreeView | null;
  status: SizeTreeStatus;
  error: string | null;
  mode: TreemapMode;
  depth: number;
  directoryName: string;
  /** 当前目录的完整路径，拼出各瓦片的路径。 */
  directoryPath: string;
  /** 当前目录是卷根：往回滚不再向上滚出卷。 */
  volumeRoot: boolean;
  selectedId: string | null;
  snapshotLabel: string | null;
}>();

const emit = defineEmits<{
  select: [node: DiskFsNode];
  open: [node: DiskFsNode];
  enter: [frames: FileNavigationFrame[]];
  up: [];
  depth: [delta: number];
  /** A tile's menu, at a viewport point. */
  contextmenu: [point: { x: number; y: number }, node: DiskFsNode];
  copyPath: [];
  showList: [];
  retry: [];
  /** 很可能马上要进入（或回到）的目录：先把它的数据取来。 */
  prefetch: [nodeId: string];
}>();

type Tile = TreemapTile<FsSizeTreeEntry>;

const { currentTheme } = useTheme();
const { motionEnabled } = useUiMotion();

const surface = ref<HTMLElement | null>(null);
const canvas = ref<HTMLCanvasElement | null>(null);
const area = ref({ width: 0, height: 0 });
const measured = computed(() => area.value.width >= 40 && area.value.height >= 40);
const hovered = ref(-1);
const pointer = ref({ x: 0, y: 0, inside: false });
/** 键盘或单击选中的瓦片下标；布局变化后按选中节点重新找。 */
const focused = ref(-1);
const announcement = ref('');
const palette = shallowRef<TreemapPalette | null>(null);
/** 视图变换与过渡逐帧变化，不进响应式：只触发重画。 */
let view: TreemapView = IDENTITY_VIEW;
let transition: LayoutTransition | null = null;
let context: CanvasRenderingContext2D | null = null;

const layout = computed<Tile[]>(() => {
  const data = props.view;
  if (!data || !measured.value) return [];
  const byFiles = props.mode === 'files';
  return layoutTreemap(data.root, { x: 0, y: 0, w: area.value.width, h: area.value.height }, {
    value: entry => (byFiles ? entry.files : entry.bytes),
    children: entry => entry.children,
    othersValue: entry => (entry.others ? (byFiles ? entry.others.files : entry.others.bytes) : 0),
    isDirectory: entry => entry.node?.kind === 'dir',
  }, { ...DEFAULT_TREEMAP_OPTIONS, maxDepth: props.depth });
});

function valueText(bytes: number, files: number): string {
  return props.mode === 'files' ? tr('{0} 个文件', { 0: files.toLocaleString() }) : formatBytes(bytes);
}

const paintTiles = computed<PaintTile[]>(() => {
  const colors = palette.value;
  if (!colors) return [];
  const reference = props.view?.newestWrite ?? null;
  return layout.value.map(tile => {
    const entry = tile.entry;
    const others = entry ? null : tile.owner.others;
    let fill: string;
    if (props.mode === 'age') {
      const bucket = ageBucketOf(entry?.newest ?? null, reference);
      fill = bucket === null ? colors.ageUnknown : colors.ageFills[bucket] ?? colors.ageUnknown;
    } else {
      fill = colors.depthFills[Math.min(tile.depth, colors.depthFills.length - 1)];
    }
    return {
      rect: tile.rect,
      header: tile.header,
      depth: tile.depth,
      fill,
      deleted: entry?.node?.deleted === true,
      others: !entry,
      label: entry ? entry.node?.name ?? props.directoryName : tr('其余 {0} 项', { 0: (others?.count ?? 0).toLocaleString() }),
      size: entry ? valueText(entry.bytes, entry.files) : valueText(others?.bytes ?? 0, others?.files ?? 0),
    };
  });
});

/** 选中的瓦片：单击 / 方向键选中的那块，否则按选中节点找到的第一块。 */
const selectedIndex = computed(() => {
  const id = props.selectedId;
  if (!id) return -1;
  if (layout.value[focused.value]?.entry?.node?.id === id) return focused.value;
  return layout.value.findIndex(tile => tile.entry?.node?.id === id);
});

// ── 绘制 ──

let frame = 0;
function requestPaint() {
  if (frame || typeof requestAnimationFrame === 'undefined') return;
  frame = requestAnimationFrame(paintNow);
}

function paintNow() {
  frame = 0;
  const element = canvas.value;
  const colors = palette.value;
  if (!element || !context || !colors) return;
  const { width, height } = area.value;
  const ratio = window.devicePixelRatio || 1;
  const pixelWidth = Math.max(1, Math.round(width * ratio));
  const pixelHeight = Math.max(1, Math.round(height * ratio));
  if (element.width !== pixelWidth) element.width = pixelWidth;
  if (element.height !== pixelHeight) element.height = pixelHeight;
  const running = paintTreemap(context, {
    tiles: paintTiles.value,
    view,
    transition,
    now: performance.now(),
    width,
    height,
    pixelRatio: ratio,
    hovered: hovered.value,
    selected: selectedIndex.value,
    palette: colors,
  });
  if (running) requestPaint();
  else transition = null;
}

function refreshPalette() {
  if (!context || !surface.value) return;
  palette.value = resolvePalette(surface.value, context);
}

let observer: ResizeObserver | null = null;
function measure() {
  const element = surface.value;
  if (!element) return;
  const width = Math.floor(element.clientWidth);
  const height = Math.floor(element.clientHeight);
  if (width !== area.value.width || height !== area.value.height) area.value = { width, height };
}

onMounted(() => {
  measure();
  if (typeof ResizeObserver !== 'undefined' && surface.value) {
    observer = new ResizeObserver(measure);
    observer.observe(surface.value);
  }
  try {
    context = canvas.value?.getContext('2d') ?? null;
  } catch {
    context = null;
  }
  refreshPalette();
  requestPaint();
});

onBeforeUnmount(() => {
  observer?.disconnect();
  if (frame && typeof cancelAnimationFrame !== 'undefined') cancelAnimationFrame(frame);
  frame = 0;
});

watch(currentTheme, refreshPalette);
watch(area, ({ width, height }) => {
  view = clampView(view, width, height);
});
watch([paintTiles, hovered, selectedIndex, area], requestPaint);

// ── 命中与选择 ──

function localPoint(event: MouseEvent): { x: number; y: number } {
  const bounds = surface.value?.getBoundingClientRect();
  return { x: event.clientX - (bounds?.left ?? 0), y: event.clientY - (bounds?.top ?? 0) };
}

/** 视口点下最深的瓦片。 */
function tileAt(x: number, y: number): number {
  const point = unprojectPoint(view, x, y);
  return hitTile(layout.value, point.x, point.y);
}

function hasContents(tile: Tile): boolean {
  return tile.entry?.node?.kind === 'dir' && tile.entry.bytes + tile.entry.files > 0;
}

/** 瓦片的节点，带上它的完整路径：列表的路径、导出与复制都按这条路径。 */
function nodeOf(index: number): DiskFsNode | null {
  const node = layout.value[index]?.entry?.node;
  if (!node) return null;
  const names = tileChain(layout.value, index).map(position => layout.value[position].entry?.node?.name ?? '');
  const base = props.directoryPath === '/' ? '' : props.directoryPath;
  return { ...node, path: `${base}/${names.join('/')}` };
}

const rootValue = computed(() => {
  const root = props.view?.root;
  if (!root) return 0;
  return props.mode === 'files' ? root.files : root.bytes;
});

function shareText(value: number): string {
  if (rootValue.value <= 0) return '';
  const percent = (value / rootValue.value) * 100;
  return `${percent >= 10 ? percent.toFixed(0) : percent.toFixed(1)}%`;
}

function select(index: number) {
  const node = nodeOf(index);
  if (!node) return;
  focused.value = index;
  emit('select', node);
  const entry = layout.value[index].entry;
  announcement.value = tr('{0}，{1}，占 {2}', {
    0: node.name,
    1: valueText(entry?.bytes ?? 0, entry?.files ?? 0),
    2: shareText(props.mode === 'files' ? entry?.files ?? 0 : entry?.bytes ?? 0) || '—',
  });
}

function relativePath(index: number): string {
  return tileChain(layout.value, index)
    .slice(0, -1)
    .map(position => layout.value[position].entry?.node?.name ?? '')
    .join('/');
}

const card = computed(() => {
  const tile = layout.value[hovered.value];
  if (!tile) return null;
  const entry = tile.entry;
  const others = entry ? null : tile.owner.others;
  const bytes = entry?.bytes ?? others?.bytes ?? 0;
  const files = entry?.files ?? others?.files ?? 0;
  const directory = entry?.node?.kind === 'dir';
  const { width, height } = area.value;
  const { x, y } = pointer.value;
  const shiftX = x > width * 0.6 ? 'calc(-100% - 14px)' : '14px';
  const shiftY = y > height * 0.6 ? 'calc(-100% - 12px)' : '16px';
  return {
    name: entry ? entry.node?.name ?? props.directoryName : tr('其余 {0} 项', { 0: (others?.count ?? 0).toLocaleString() }),
    path: relativePath(hovered.value),
    deleted: entry?.node?.deleted === true,
    size: formatBytes(bytes),
    share: shareText(props.mode === 'files' ? files : bytes),
    files: directory || !entry ? files.toLocaleString() : null,
    dirs: directory ? (entry?.dirs ?? 0).toLocaleString() : null,
    newest: entry && entry.newest !== null ? formatUnixSeconds(entry.newest) : '',
    style: `left:${x}px;top:${y}px;transform:translate(${shiftX},${shiftY})`,
  };
});

// ── 进出层级 ──

type PendingTransition =
  | { kind: 'enter'; targetId: string; from: Rect }
  | { kind: 'up'; childId: string | null; src: Rect };
let pending: PendingTransition | null = null;
let pendingSince = 0;
/** 一次进出层级在新视图到达前不接受下一次；迟迟不到的请求不会一直挡住手势。 */
const PENDING_TIMEOUT_MS = 3000;

function busy(): boolean {
  return pending !== null && performance.now() - pendingSince < PENDING_TIMEOUT_MS;
}

/** 换层级后指针下已是别的瓦片，直到指针再动之前不算悬停。 */
function forgetHover() {
  hovered.value = -1;
  pointer.value = { ...pointer.value, inside: false };
}

/** 进入一块目录瓦片：沿瓦片链把每一层目录都推入面包屑。`from` 是它的内容区此刻在屏幕上的位置，
 *  新的一层就从那里长到整个视口。 */
function enter(index: number, from?: Rect) {
  const tile = layout.value[index];
  const node = tile?.entry?.node;
  if (!tile || node?.kind !== 'dir' || busy()) return;
  const frames = tileChain(layout.value, index).map(position => {
    const entry = layout.value[position].entry?.node;
    return { id: entry?.id ?? '', name: entry?.name ?? '' };
  });
  if (frames.some(item => !item.id)) return;
  pending = { kind: 'enter', targetId: node.id, from: from ?? projectRect(view, tileBody(tile)) };
  pendingSince = performance.now();
  forgetHover();
  emit('enter', frames);
}

/** 回到上一级：正在看的区域缩回它在上一级里的瓦片。 */
function goUp() {
  if (busy()) return;
  pending = { kind: 'up', childId: props.view?.root.node?.id ?? null, src: visibleBase(view, area.value.width, area.value.height) };
  pendingSince = performance.now();
  forgetHover();
  emit('up');
}

/** Enter：选中的目录就进入它，选中的文件打开预览；什么都没选时进入最大的子目录。 */
function descend() {
  const index = selectedIndex.value;
  const node = nodeOf(index);
  if (node?.kind === 'dir') {
    enter(index);
    return;
  }
  if (node) {
    emit('open', node);
    return;
  }
  const largest = layout.value.findIndex(tile => tile.depth === 0 && tile.entry?.node);
  if (largest >= 0 && layout.value[largest].entry?.node?.kind === 'dir') enter(largest);
}

/** 滚轮正朝它放大的目录：放大到头就会进入，趁放大的这几格把它的数据取来。 */
let prefetched: string | null = null;
function prefetchTile(index: number) {
  const node = layout.value[index]?.entry?.node;
  if (node?.kind !== 'dir' || node.id === prefetched) return;
  prefetched = node.id;
  emit('prefetch', node.id);
}

function applyZoom(step: ZoomStep) {
  switch (step.kind) {
    case 'view':
      view = step.view;
      if (pointer.value.inside) hovered.value = tileAt(pointer.value.x, pointer.value.y);
      requestPaint();
      return;
    case 'enter':
      enter(step.target, step.from);
      return;
    case 'ascend':
      if (!props.volumeRoot) goUp();
      return;
    case 'none':
      return;
  }
}

/** 新视图到达：进入时新的一层从被放大的瓦片里长出来，返回时刚离开的目录缩回它的瓦片；
 *  同一目录换了层数或度量时保持当前的放大；其余跳转直接显示。 */
let shownPath: string | null = null;
watch(() => props.view, (next, previous) => {
  focused.value = -1;
  if (!next || next === previous) return;
  const arrived = pending;
  pending = null;
  prefetched = null;
  const { width, height } = area.value;
  const sameDirectory = arrived === null && shownPath === props.directoryPath;
  shownPath = props.directoryPath;
  if (sameDirectory) {
    view = clampView(view, width, height);
    requestPaint();
    return;
  }
  // 原始大小时往回滚就回上一级：上一级多半已在缓存（从那里进来的），否则现在就取。
  const parent = next.root.node?.parent_id;
  if (parent) emit('prefetch', parent);
  hovered.value = -1;
  view = IDENTITY_VIEW;
  transition = null;
  if (arrived && motionEnabled.value && width > 0 && height > 0) {
    const now = performance.now();
    if (arrived.kind === 'enter' && next.root.node?.id === arrived.targetId) {
      transition = layoutTransition(arrived.from, { x: 0, y: 0, w: width, h: height }, now);
    } else if (arrived.kind === 'up' && arrived.childId) {
      const index = layout.value.findIndex(tile => tile.entry?.node?.id === arrived.childId);
      const body = index >= 0 ? tileBody(layout.value[index]) : null;
      if (body && body.w > 1 && body.h > 1) transition = layoutTransition(arrived.src, body, now);
    }
  }
  requestPaint();
});

// 加载失败或换到无数据的状态时，未完成的进出作废，下一次手势照常响应。
watch(() => props.status, status => {
  if (status !== 'loading' && status !== 'ready') pending = null;
});

// ── 指针、滚轮与键盘 ──

function onPointerMove(event: PointerEvent) {
  const point = localPoint(event);
  pointer.value = { ...point, inside: true };
  const index = tileAt(point.x, point.y);
  if (index !== hovered.value) hovered.value = index;
}

function onPointerLeave() {
  forgetHover();
}

/** 单击只选中，与列表一致；双击、Enter 或滚轮才进入目录。 */
function onClick(event: MouseEvent) {
  if (event.detail > 1) return;
  const { x, y } = localPoint(event);
  const index = tileAt(x, y);
  if (index >= 0) select(index);
}

function onDoubleClick(event: MouseEvent) {
  const { x, y } = localPoint(event);
  const index = tileAt(x, y);
  const node = nodeOf(index);
  if (!node) return;
  if (node.kind === 'dir') enter(index);
  else emit('open', node);
}

// ── 右键菜单 ──
// 瓦片：选中它并打开与列表相同的行菜单；瓦片间的缝隙与合并的尾部：当前目录与视图的菜单。

const { menu: backgroundMenu, openAt: openBackgroundMenu } = useContextMenu<null>();
/** 打开背景菜单那一刻是否放大着：视图变换不进响应式。 */
const zoomedAtMenu = ref(false);

const rootSizeText = computed(() => {
  const root = props.view?.root;
  return root ? valueText(root.bytes, root.files) : undefined;
});

const backgroundEntries = computed<UiContextMenuEntry[]>(() => [
  { id: 'up', label: tr('返回上级'), icon: CornerLeftUp, hint: 'Backspace', action: goUp },
  { id: 'reset-zoom', label: tr('复原缩放'), icon: ZoomOut, hint: '0', disabled: !zoomedAtMenu.value, action: resetZoom },
  { kind: 'separator' },
  { id: 'deeper', label: tr('多显示一层'), icon: ListPlus, hint: ']', disabled: props.depth >= TREEMAP_MAX_DEPTH, action: () => emit('depth', 1) },
  { id: 'shallower', label: tr('少显示一层'), icon: ListMinus, hint: '[', disabled: props.depth <= TREEMAP_MIN_DEPTH, action: () => emit('depth', -1) },
  { kind: 'separator' },
  { id: 'copy-path', label: tr('复制当前目录路径'), icon: ClipboardCopy, action: () => emit('copyPath') },
  { id: 'list', label: tr('切换到列表'), icon: List, action: () => emit('showList') },
]);

function showBackgroundMenu(x: number, y: number) {
  zoomedAtMenu.value = view !== IDENTITY_VIEW;
  openBackgroundMenu(x, y, null);
}

function onContextMenu(event: MouseEvent) {
  event.preventDefault();
  if (!layout.value.length) return;
  const { x, y } = localPoint(event);
  const index = tileAt(x, y);
  const node = nodeOf(index);
  if (!node) {
    showBackgroundMenu(event.clientX, event.clientY);
    return;
  }
  select(index);
  emit('contextmenu', { x: event.clientX, y: event.clientY }, node);
}

/** ContextMenu 键 / Shift+F10：选中瓦片的菜单开在它的名称处，没有选中时开背景菜单。 */
function openMenuFromKeyboard() {
  const bounds = surface.value?.getBoundingClientRect();
  if (!bounds || !layout.value.length) return;
  const index = selectedIndex.value;
  const node = nodeOf(index);
  if (!node) {
    showBackgroundMenu(bounds.left + 24, bounds.top + 24);
    return;
  }
  const rect = projectRect(view, layout.value[index].rect);
  const { width, height } = area.value;
  const left = Math.min(Math.max(rect.x, 0), width);
  const top = Math.min(Math.max(rect.y + Math.min(rect.h, 24), 0), height);
  emit('contextmenu', { x: bounds.left + left + Math.min(rect.w / 2, 24), y: bounds.top + top }, node);
}

/** 鼠标侧键「后退」与 Backspace 一样返回上级（带缩回动画）。 */
function onMouseUp(event: MouseEvent) {
  if (event.button !== 3) return;
  event.preventDefault();
  event.stopPropagation();
  goUp();
}

function resetZoom() {
  view = IDENTITY_VIEW;
  requestPaint();
}

/** 菜单里的「打开」：按节点找到瓦片，像双击一样进入。 */
function enterNode(nodeId: string): boolean {
  const index = layout.value.findIndex(tile => tile.entry?.node?.id === nodeId);
  if (index < 0 || layout.value[index].entry?.node?.kind !== 'dir') return false;
  enter(index);
  return true;
}

defineExpose({ enterNode });

/** 滚轮：每次朝指针放大或缩小一档。放大停在指针下目录恰好填满视口的倍率，再滚就进入；
 *  在原始大小往回滚就回到上一级。按住 Shift 滚动在放大的视图里上下平移。 */
function onWheel(event: WheelEvent) {
  if (!layout.value.length) return;
  const delta = event.shiftKey && event.deltaY === 0 ? event.deltaX : event.deltaY;
  const lines = wheelLines(delta, event.deltaMode);
  if (Math.abs(lines) < 1e-6) return;
  event.preventDefault();
  const { width, height } = area.value;
  if (event.shiftKey) {
    view = panView(view, lines, width, height);
    requestPaint();
    return;
  }
  if (busy()) return;
  const { x, y } = localPoint(event);
  const factor = lines > 0 ? WHEEL_ZOOM_FACTOR : 1 / WHEEL_ZOOM_FACTOR;
  const step = zoomStep(layout.value, view, width, height, x, y, factor, true, hasContents);
  if (step.kind === 'view' && factor > 1 && step.target >= 0) prefetchTile(step.target);
  applyZoom(step);
}

const DIRECTIONS: Record<string, TreemapDirection> = { ArrowLeft: 'left', ArrowRight: 'right', ArrowUp: 'up', ArrowDown: 'down' };

function onKeydown(event: KeyboardEvent) {
  if (isContextMenuKey(event)) {
    event.preventDefault();
    openMenuFromKeyboard();
    return;
  }
  if (event.ctrlKey || event.metaKey) return;
  if (event.altKey) {
    if (event.key === 'ArrowLeft') {
      event.preventDefault();
      goUp();
    }
    return;
  }
  const direction = DIRECTIONS[event.key];
  if (direction) {
    event.preventDefault();
    move(direction);
    return;
  }
  const { width, height } = area.value;
  switch (event.key) {
    case 'Enter':
      event.preventDefault();
      descend();
      return;
    case 'Backspace':
      event.preventDefault();
      goUp();
      return;
    case '[':
      event.preventDefault();
      emit('depth', -1);
      return;
    case ']':
      event.preventDefault();
      emit('depth', 1);
      return;
    case '+':
    case '=':
      event.preventDefault();
      applyZoom(zoomStep(layout.value, view, width, height, width / 2, height / 2, KEY_ZOOM_FACTOR, false, hasContents));
      return;
    case '-':
      event.preventDefault();
      applyZoom(zoomStep(layout.value, view, width, height, width / 2, height / 2, 1 / KEY_ZOOM_FACTOR, false, hasContents));
      return;
    case '0':
      event.preventDefault();
      resetZoom();
      return;
  }
}

/** 方向键：同层里沿方向最近的瓦片；向左 / 向上到边时退到父瓦片。 */
function move(direction: TreemapDirection) {
  const from = selectedIndex.value;
  if (from < 0) {
    const first = layout.value.findIndex(tile => tile.entry?.node);
    if (first >= 0) select(first);
    return;
  }
  const next = neighbourTile(layout.value, from, direction);
  if (next >= 0) {
    if (layout.value[next].entry?.node) select(next);
    return;
  }
  const parent = layout.value[from].parent;
  if ((direction === 'left' || direction === 'up') && parent >= 0) select(parent);
}
</script>

<style scoped>
/* ── 树图画布 ──
   瓦片之间的缝隙露出画布底色，顶层目录之间的缝更宽：不画边框也能读出结构。 */
.treemap-surface {
  position: relative;
  flex: 1;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  background: var(--surface-tertiary);
  outline: none;
  user-select: none;
  touch-action: none;
}

.treemap-surface:focus-visible {
  box-shadow: inset 0 0 0 2px var(--primary-color);
}

.treemap-canvas {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
}

.tile-card {
  position: absolute;
  z-index: 2;
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 4px;
  max-width: 320px;
  padding: 8px 10px;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  background: var(--surface-secondary);
  box-shadow: var(--card-shadow-hover);
  color: var(--text-primary);
  font-size: 12px;
  line-height: 1.5;
  pointer-events: none;
}

.card-name {
  max-width: 100%;
  margin: 0;
  overflow: hidden;
  font-weight: 600;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.card-name.deleted {
  color: var(--sev-critical-fg);
  text-decoration: line-through;
}

.card-path {
  max-width: 100%;
  margin: 0;
  overflow: hidden;
  color: var(--text-secondary);
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.card-facts {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 2px 12px;
  margin: 2px 0 0;
}

.card-facts > div {
  display: contents;
}

.card-facts dt {
  color: var(--text-secondary);
}

.card-facts dd {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin: 0;
  font-variant-numeric: tabular-nums;
}

.card-share {
  color: var(--text-secondary);
}

.mono {
  font-family: var(--font-mono);
  font-size: 11px;
}

.treemap-busy {
  position: absolute;
  top: 8px;
  right: 8px;
  display: grid;
  place-items: center;
  width: 24px;
  height: 24px;
  border-radius: 50%;
  background: var(--surface-secondary);
  box-shadow: var(--shadow-sm);
  color: var(--primary-color);
}

.treemap-state {
  position: absolute;
  inset: 0;
  background: var(--surface-tertiary);
}
</style>
