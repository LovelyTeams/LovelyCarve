<template>
<div
  ref="surface"
  class="treemap-surface"
  tabindex="0"
  role="group"
  :aria-roledescription="tr('树图')"
  :aria-label="tr('空间树图：{0}', { 0: directoryName })"
  @pointermove="onPointerMove"
  @pointerleave="hovered = -1"
  @click="onClick"
  @dblclick="onDoubleClick"
  @contextmenu="onContextMenu"
  @wheel="onWheel"
  @keydown="onKeydown"
>
  <template v-if="tiles.length">
    <div ref="layer" class="tile-layer">
      <div v-for="tile in tiles" :key="tile.key" class="tile" :class="tile.classes" :style="tile.style" :data-tile="tile.index">
        <span v-if="tile.label" class="tile-label" :class="tile.labelClass" :style="tile.labelStyle">
          <span class="tile-name">{{ tile.label }}</span>
          <span v-if="tile.sizeText" class="tile-size">{{ tile.sizeText }}</span>
        </span>
      </div>
    </div>
    <div v-if="selectionStyle" class="tile-selection" :style="selectionStyle" aria-hidden="true"></div>
    <div v-if="card" class="tile-card" role="tooltip" :style="card.style">
      <p class="card-name" :class="{ deleted: card.deleted }">{{ card.name }}</p>
      <p v-if="card.path" class="card-path">{{ card.path }}</p>
      <dl class="card-facts">
        <div><dt>{{ tr("大小") }}</dt><dd>{{ card.size }}<span class="card-share">{{ card.share }}</span></dd></div>
        <div v-if="card.files !== null"><dt>{{ tr("文件") }}</dt><dd>{{ card.files }}</dd></div>
        <div v-if="card.dirs !== null"><dt>{{ tr("子目录") }}</dt><dd>{{ card.dirs }}</dd></div>
        <div v-if="card.newest"><dt>{{ tr("最新修改") }}</dt><dd class="mono">{{ card.newest }}</dd></div>
        <div><dt>{{ tr("类别") }}</dt><dd class="card-kind"><span class="legend-swatch" :class="card.swatch"></span>{{ card.kind }}</dd></div>
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
</div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { Camera, CircleAlert, LayoutGrid, List, Loader2, RefreshCw } from 'lucide-vue-next';
import { animateUi, cancelUiAnimation } from '../../../composables/useUiMotion';
import type { FsSizeTreeEntry, FsSizeTreeView } from '../../../lib/ipc/fs/sizeTree';
import type { SizeTreeStatus, TreemapMode } from '../composables/useSizeTree';
import { formatBytes, type DiskFsNode } from '../model/model';
import type { FileNavigationFrame } from '../model/navigation';
import {
  DEFAULT_TREEMAP_OPTIONS, layoutTreemap, neighbourTile, tileBody, tileChain,
  type Rect, type TreemapDirection, type TreemapTile,
} from '../model/treemap';
import { ageBucketOf, formatUnixSeconds, treemapKindLabel, treemapKindOf, type TreemapKind } from '../model/treemapKinds';

const props = defineProps<{
  view: FsSizeTreeView | null;
  status: SizeTreeStatus;
  error: string | null;
  mode: TreemapMode;
  depth: number;
  directoryName: string;
  /** 当前目录的完整路径，拼出各瓦片的路径。 */
  directoryPath: string;
  /** 当前目录是卷根：顶层的 NTFS 元文件按名字判为系统。 */
  volumeRoot: boolean;
  /** 当前目录本身在系统区（如 `$Extend`）内。 */
  systemArea: boolean;
  selectedId: string | null;
  snapshotLabel: string | null;
}>();

const emit = defineEmits<{
  select: [node: DiskFsNode];
  open: [node: DiskFsNode];
  enter: [frames: FileNavigationFrame[]];
  up: [];
  depth: [delta: number];
  contextmenu: [event: MouseEvent, node: DiskFsNode];
  showList: [];
  retry: [];
}>();

type Tile = TreemapTile<FsSizeTreeEntry>;

/** 能放下名称的最小区域，以及名称下方再放一行大小的高度。 */
const LABEL_MIN_WIDTH = 54;
const LABEL_MIN_HEIGHT = 15;
const STACK_MIN_HEIGHT = 36;
/** 标签左右内边距与名称、大小之间的间隙。 */
const LABEL_CHROME = 20;
/** 滚轮累计到这么多像素才进入或退出一层，触控板的细碎滚动不会连跳几层。 */
const WHEEL_STEP = 60;

const surface = ref<HTMLElement | null>(null);
const layer = ref<HTMLElement | null>(null);
const area = ref({ width: 0, height: 0 });
const measured = computed(() => area.value.width >= 40 && area.value.height >= 40);
const hovered = ref(-1);
const pointer = ref({ x: 0, y: 0 });
/** 键盘或单击选中的瓦片下标；布局变化后按选中节点重新找。 */
const focused = ref(-1);
const announcement = ref('');

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
});
onBeforeUnmount(() => {
  observer?.disconnect();
  if (layer.value) cancelUiAnimation(layer.value);
});

const layout = computed<Tile[]>(() => {
  const view = props.view;
  if (!view || !measured.value) return [];
  const byFiles = props.mode === 'files';
  return layoutTreemap(view.root, { x: 0, y: 0, w: area.value.width, h: area.value.height }, {
    value: entry => (byFiles ? entry.files : entry.bytes),
    children: entry => entry.children,
    othersValue: entry => (entry.others ? (byFiles ? entry.others.files : entry.others.bytes) : 0),
    isDirectory: entry => entry.node?.kind === 'dir',
  }, { ...DEFAULT_TREEMAP_OPTIONS, maxDepth: props.depth });
});

/** 每块瓦片的类别：系统区里的一切都是系统，其余各自判定。 */
const kinds = computed<TreemapKind[]>(() => {
  const result: TreemapKind[] = [];
  for (const tile of layout.value) {
    const inherited = tile.parent >= 0 && result[tile.parent] === 'system';
    result.push(tile.entry
      ? treemapKindOf(tile.entry, { system: props.systemArea || inherited, volumeRoot: props.volumeRoot, depth: tile.depth })
      : inherited ? 'system' : 'other');
  }
  return result;
});

function snapped(rect: Rect) {
  const left = Math.round(rect.x);
  const top = Math.round(rect.y);
  return { left, top, width: Math.max(0, Math.round(rect.x + rect.w) - left), height: Math.max(0, Math.round(rect.y + rect.h) - top) };
}

function boxStyle(rect: Rect): string {
  const box = snapped(rect);
  return `left:${box.left}px;top:${box.top}px;width:${box.width}px;height:${box.height}px`;
}

/** 文字的大致宽度：中日韩字符一个字号宽，其余约六成。只用来决定放不放得下。 */
function textWidth(text: string, fontSize: number): number {
  let width = 0;
  for (const character of text) width += (character.codePointAt(0) ?? 0) >= 0x2e80 ? fontSize : fontSize * 0.6;
  return width;
}

function valueText(bytes: number, files: number): string {
  return props.mode === 'files' ? tr('{0} 个文件', { 0: files.toLocaleString() }) : formatBytes(bytes);
}

function colourClass(tile: Tile, index: number): string {
  if (props.mode !== 'age') return `k-${kinds.value[index]}`;
  const bucket = ageBucketOf(tile.entry?.newest ?? null, props.view?.newestWrite ?? null);
  return bucket === null ? 'age-none' : `age-${bucket}`;
}

const tiles = computed(() => layout.value.map((tile, index) => {
  const entry = tile.entry;
  const owned = tile.header ?? tile.rect;
  const labelled = owned.w >= LABEL_MIN_WIDTH && owned.h >= LABEL_MIN_HEIGHT;
  const stacked = !tile.header && tile.rect.h >= STACK_MIN_HEIGHT;
  const classes = [colourClass(tile, index), props.mode === 'age' ? 'aged' : `d${Math.min(tile.depth, 3)}`];
  if (tile.header) classes.push('open');
  if (!entry) classes.push('others');
  if (entry?.node?.deleted) classes.push('deleted');
  const others = entry ? null : tile.owner.others;
  const label = !labelled ? null : entry ? entry.node?.name ?? props.directoryName : tr('其余 {0} 项', { 0: (others?.count ?? 0).toLocaleString() });
  const value = !label ? null : entry ? valueText(entry.bytes, entry.files) : valueText(others?.bytes ?? 0, others?.files ?? 0);
  // 名称优先：大小放在名称下方，或者与名称同一行且两者都放得下，否则不显示。
  const sizeText = value !== null && label !== null
    && (stacked || textWidth(label, 12) + textWidth(value, 11) + LABEL_CHROME <= owned.w) ? value : null;
  return {
    key: `${tile.parent}:${entry?.node?.id ?? 'others'}`,
    index,
    classes,
    style: boxStyle(tile.rect),
    label,
    sizeText,
    labelClass: tile.header ? (tile.depth === 0 ? 'band top' : 'band') : stacked ? 'stacked' : '',
    labelStyle: tile.header ? `height:${Math.round(tile.header.h)}px` : undefined,
  };
}));

/** 选中的瓦片：单击 / 方向键选中的那块，否则按选中节点找到的第一块。 */
const selectedIndex = computed(() => {
  const id = props.selectedId;
  if (!id) return -1;
  if (layout.value[focused.value]?.entry?.node?.id === id) return focused.value;
  return layout.value.findIndex(tile => tile.entry?.node?.id === id);
});
const selectionStyle = computed(() => {
  const tile = layout.value[selectedIndex.value];
  return tile ? boxStyle(tile.rect) : null;
});

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
  const kind = kinds.value[hovered.value] ?? 'other';
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
    kind: treemapKindLabel(kind, tr),
    swatch: `k-${kind}`,
    style: `left:${x}px;top:${y}px;transform:translate(${shiftX},${shiftY})`,
  };
});

function tileIndexOf(event: Event): number {
  const element = (event.target as Element | null)?.closest?.('[data-tile]') as HTMLElement | null | undefined;
  const index = Number(element?.dataset.tile ?? -1);
  return Number.isInteger(index) && index < layout.value.length ? index : -1;
}

/** 瓦片的节点，带上它的完整路径：列表的路径、导出与复制都按这条路径。 */
function nodeOf(index: number): DiskFsNode | null {
  const node = layout.value[index]?.entry?.node;
  if (!node) return null;
  const names = tileChain(layout.value, index).map(position => layout.value[position].entry?.node?.name ?? '');
  const base = props.directoryPath === '/' ? '' : props.directoryPath;
  return { ...node, path: `${base}/${names.join('/')}` };
}

function select(index: number) {
  const node = nodeOf(index);
  if (!node) return;
  focused.value = index;
  emit('select', node);
  const tile = layout.value[index];
  announcement.value = tr('{0}，{1}，占 {2}', {
    0: node.name,
    1: valueText(tile.entry?.bytes ?? 0, tile.entry?.files ?? 0),
    2: shareText(props.mode === 'files' ? tile.entry?.files ?? 0 : tile.entry?.bytes ?? 0) || '—',
  });
}

type PendingTransition = { kind: 'enter'; targetId: string; from: Rect } | { kind: 'up'; childId: string | null };
let pending: PendingTransition | null = null;
let pendingSince = 0;
/** 一次进出层级在新视图到达前不接受下一次；迟迟不到的请求不会一直挡住手势。 */
const PENDING_TIMEOUT_MS = 3000;

function busy(): boolean {
  return pending !== null && performance.now() - pendingSince < PENDING_TIMEOUT_MS;
}

/** 进入一块目录瓦片：沿瓦片链把每一层目录都推入面包屑。 */
function enter(index: number) {
  const tile = layout.value[index];
  const node = tile?.entry?.node;
  if (!tile || node?.kind !== 'dir' || busy()) return;
  const frames = tileChain(layout.value, index).map(position => {
    const entry = layout.value[position].entry?.node;
    return { id: entry?.id ?? '', name: entry?.name ?? '' };
  });
  if (frames.some(frame => !frame.id)) return;
  pending = { kind: 'enter', targetId: node.id, from: tileBody(tile) };
  pendingSince = performance.now();
  emit('enter', frames);
}

function goUp() {
  if (busy()) return;
  pending = { kind: 'up', childId: props.view?.root.node?.id ?? null };
  pendingSince = performance.now();
  emit('up');
}

/** 滚轮放大的对象：指针下最深的、有内容的目录。 */
function zoomTarget(index: number): number {
  const chain = tileChain(layout.value, index);
  for (let position = chain.length - 1; position >= 0; position -= 1) {
    const entry = layout.value[chain[position]].entry;
    if (entry?.node?.kind === 'dir' && entry.bytes + entry.files > 0) return chain[position];
  }
  return -1;
}

function onPointerMove(event: PointerEvent) {
  const bounds = surface.value?.getBoundingClientRect();
  if (bounds) pointer.value = { x: event.clientX - bounds.left, y: event.clientY - bounds.top };
  const index = tileIndexOf(event);
  if (index !== hovered.value) hovered.value = index;
}

function onClick(event: MouseEvent) {
  const index = tileIndexOf(event);
  if (index >= 0) select(index);
}

function onDoubleClick(event: MouseEvent) {
  const index = tileIndexOf(event);
  const node = nodeOf(index);
  if (!node) return;
  if (node.kind === 'dir') enter(index);
  else emit('open', node);
}

function onContextMenu(event: MouseEvent) {
  const index = tileIndexOf(event);
  const node = nodeOf(index);
  if (!node) return;
  event.preventDefault();
  select(index);
  emit('contextmenu', event, node);
}

let wheelTravel = 0;
let wheelQuietUntil = 0;
function onWheel(event: WheelEvent) {
  if (!layout.value.length) return;
  event.preventDefault();
  if (busy() || performance.now() < wheelQuietUntil) {
    wheelTravel = 0;
    return;
  }
  const unit = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? area.value.height : 1;
  wheelTravel += event.deltaY * unit;
  if (Math.abs(wheelTravel) < WHEEL_STEP) return;
  const inward = wheelTravel < 0;
  wheelTravel = 0;
  wheelQuietUntil = performance.now() + 350;
  if (inward) {
    const target = zoomTarget(tileIndexOf(event));
    if (target >= 0) enter(target);
  } else if (!props.volumeRoot) {
    goUp();
  }
}

const DIRECTIONS: Record<string, TreemapDirection> = { ArrowLeft: 'left', ArrowRight: 'right', ArrowUp: 'up', ArrowDown: 'down' };

function onKeydown(event: KeyboardEvent) {
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
  switch (event.key) {
    case 'Enter': {
      event.preventDefault();
      const index = selectedIndex.value;
      const node = nodeOf(index);
      if (node?.kind === 'dir') enter(index);
      else if (node) emit('open', node);
      return;
    }
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

/** 新视图到达：进入时新的一层从被点的瓦片里长出来，返回时刚离开的目录缩回它的瓦片，
 *  其余情况淡入。这让「放大」和「进入」读起来是同一个动作。 */
watch(() => props.view, async (view, previous) => {
  hovered.value = -1;
  focused.value = -1;
  if (!view || view === previous) return;
  const transition = pending;
  pending = null;
  await nextTick();
  const element = layer.value;
  if (!element) return;
  const { width, height } = area.value;
  let from: string | null = null;
  if (transition?.kind === 'enter' && view.root.node?.id === transition.targetId) {
    const rect = transition.from;
    const scale = Math.min(rect.w / width, rect.h / height);
    from = `translate(${rect.x + (rect.w - width * scale) / 2}px, ${rect.y + (rect.h - height * scale) / 2}px) scale(${scale})`;
  } else if (transition?.kind === 'up' && transition.childId) {
    const index = layout.value.findIndex(tile => tile.entry?.node?.id === transition.childId);
    const rect = index >= 0 ? tileBody(layout.value[index]) : null;
    if (rect && rect.w > 1 && rect.h > 1) {
      const scale = Math.min(width / rect.w, height / rect.h);
      from = `translate(${(width - rect.w * scale) / 2 - rect.x * scale}px, ${(height - rect.h * scale) / 2 - rect.y * scale}px) scale(${scale})`;
    }
  }
  if (from && Number.isFinite(width) && width > 0) {
    animateUi(element, [{ transform: from, opacity: 0.35 }, { transform: 'none', opacity: 1 }], undefined, { speed: 'normal', easing: 'out' });
  } else {
    animateUi(element, [{ opacity: 0.4 }, { opacity: 1 }]);
  }
});

// 加载失败或换到无数据的状态时，未完成的过渡作废，下一次手势照常响应。
watch(() => props.status, status => {
  if (status !== 'loading' && status !== 'ready') pending = null;
});
</script>

<style scoped src="../styles/treemap-kinds.css"></style>
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
}

.treemap-surface:focus-visible {
  box-shadow: inset 0 0 0 2px var(--primary-color);
}

.tile-layer {
  position: absolute;
  inset: 0;
  transform-origin: 0 0;
}

/* 类别给出色相，层级越深混入越多，嵌套不靠边框也能读出。 */
.tile {
  --tile-mix: 20%;
  position: absolute;
  box-sizing: border-box;
  overflow: hidden;
  background-color: color-mix(in srgb, var(--tile-hue) var(--tile-mix), var(--surface-secondary));
}

.tile.d1 { --tile-mix: 28%; }
.tile.d2 { --tile-mix: 36%; }
.tile.d3 { --tile-mix: 44%; }
.tile.aged { --tile-mix: var(--age-mix); }

.tile:hover {
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--text-primary) 50%, transparent);
}

/* 已删除：斜线阴影与红色划线名称，与列表的已删除行同一套提示。 */
.tile.deleted {
  background-image: repeating-linear-gradient(135deg, color-mix(in srgb, var(--sev-critical-fg) 40%, transparent) 0 1px, transparent 1px 6px);
}

.tile.deleted .tile-name {
  color: var(--sev-critical-fg);
  text-decoration: line-through;
}

.tile.others .tile-name {
  color: var(--text-secondary);
  font-style: italic;
}

.tile-label {
  position: absolute;
  top: 0;
  right: 0;
  left: 0;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 1px 6px;
  color: var(--text-primary);
  font-size: 12px;
  line-height: 16px;
  white-space: nowrap;
  pointer-events: none;
}

.tile-label.band {
  padding-top: 0;
  padding-bottom: 0;
}

/* 顶层目录的名称带：名称加粗，大小靠右排成一列。 */
.tile-label.top {
  justify-content: space-between;
  font-weight: 600;
}

.tile-label.stacked {
  flex-direction: column;
  align-items: flex-start;
  gap: 0;
  padding-top: 2px;
}

.tile-name {
  min-width: 0;
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
}

.tile-size {
  flex-shrink: 0;
  color: color-mix(in srgb, var(--text-primary) 72%, transparent);
  font-size: 11px;
  font-weight: 400;
  font-variant-numeric: tabular-nums;
}

.tile-selection {
  position: absolute;
  box-sizing: border-box;
  border: 2px solid var(--primary-color);
  box-shadow: 0 0 0 1px var(--surface-secondary);
  pointer-events: none;
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
}
</style>
