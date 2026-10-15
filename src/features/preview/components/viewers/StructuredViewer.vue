<template>
  <div class="structured-viewer">
    <!-- 工具栏 -->
    <div class="toolbar">
      <div class="ui-seg">
        <button class="ui-seg-item" :class="{ active: view === 'graph' }" @click="view = 'graph'" :title="tr('节点图')">
          <Network :size="13" /> {{ tr("图") }}
        </button>
        <button class="ui-seg-item" :class="{ active: view === 'tree' }" @click="view = 'tree'" :title="tr('树视图')">
          <ListTree :size="13" /> {{ tr("树") }}
        </button>
      </div>

      <span class="ui-badge sm mono">{{ format.toUpperCase() }}</span>
      <span v-if="okScene" class="stat mono">{{ nodeCount.toLocaleString() }} {{ tr("节点") }}</span>

      <div class="spacer"></div>

      <template v-if="view === 'graph' && okScene">
        <button class="ui-icon-btn sm" @click="expandAll" :title="tr('全部展开')" :aria-label="tr('全部展开')"><ChevronsUpDown :size="14" /></button>
        <button class="ui-icon-btn sm" @click="collapseAll" :title="tr('全部折叠')" :aria-label="tr('全部折叠')"><ChevronsDownUp :size="14" /></button>
        <span class="ui-divider-v"></span>
        <button class="ui-icon-btn sm" @click="zoomOut" :title="tr('缩小')" :aria-label="tr('缩小')"><ZoomOut :size="14" /></button>
        <span class="zoom mono" @click="resetView" :title="tr('重置缩放')">{{ Math.round(zoom * 100) }}%</span>
        <button class="ui-icon-btn sm" @click="zoomIn" :title="tr('放大')" :aria-label="tr('放大')"><ZoomIn :size="14" /></button>
        <button class="ui-icon-btn sm" @click="doFit" :title="tr('适应窗口')" :aria-label="tr('适应窗口')"><Frame :size="14" /></button>
      </template>
    </div>

    <!-- 错误 / 空 -->
    <div v-if="scene.kind === 'error'" class="state-row mono error">
      <AlertTriangle :size="14" /> {{ tr("无法解析为") }} {{ format.toUpperCase() }} {{ tr("结构：") }}{{ scene.message }}{{ tr("。请切换到「文本 / 十六进制」视图。") }}
    </div>
    <div v-else-if="scene.kind === 'empty'" class="state-row mono">{{ tr("空文档，无可视化内容。") }}</div>

    <!-- 图视图 -->
    <div
      v-else-if="view === 'graph'"
      ref="canvasEl"
      class="canvas"
      :class="{ panning: isPanning }"
      @wheel="onWheel"
      @pointerdown="onPointerDown"
      @pointermove="onPointerMove"
      @pointerup="onPointerUp"
      @pointerleave="onPointerUp"
    >
      <div v-if="overCap" class="cap-banner mono">
        <AlertTriangle :size="12" /> {{ tr("结构较大（超过") }} {{ NODE_CAP.toLocaleString() }} {{ tr("节点），部分深层分支已折叠为摘要；可改用「树」或文本视图。") }}
      </div>

      <div class="viewport" :style="viewportStyle">
        <svg class="edges" :width="bounds.width" :height="bounds.height">
          <path v-for="e in edges" :key="e.id" :d="edgePath(e)" class="edge" />
        </svg>

        <div
          v-for="n in nodes"
          :key="n.id"
          class="node"
          :class="{ prim: n.kind === 'primitive', match: matched.ids.has(n.id) }"
          :style="nodeStyle(n)"
        >
          <div
            v-for="(r, i) in n.rows"
            :key="i"
            class="row"
            :class="{ hit: matched.rows.has(`${n.id}:${i}`) }"
            :title="rowTitle(r)"
          >
            <span class="row-lead">
              <button
                v-if="r.childId"
                class="row-toggle"
                @pointerdown.stop
                @click.stop="toggle(r.childId!)"
                :title="collapsed.has(r.childId) ? tr('展开') : tr('折叠')"
              >
                <Plus v-if="collapsed.has(r.childId)" :size="11" />
                <Minus v-else :size="11" />
              </button>
            </span>
            <span v-if="r.key !== null" class="r-key">{{ r.key }}</span>
            <span v-if="r.key !== null" class="r-colon">:</span>
            <span class="r-val" :class="`t-${r.valueType}`">{{ r.preview }}</span>
          </div>
        </div>
      </div>

      <div class="hint mono">{{ tr("滚轮缩放 · 拖拽平移") }}</div>
    </div>

    <!-- 树视图 -->
    <div v-else class="tree-wrap">
      <StructuredTreeNode :label="null" :value="scene.value" :depth="0" :query="query" />
    </div>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../../composables/useLocale';

// JSON / YAML / XML 的「原类型视图」：仿 jsoncrack 把结构化文本渲染成可折叠的
// 节点图（图）与缩进折叠列表（树）。解析全在前端（parse.ts），不走后端/IPC——
// 文本由父级 decodedText 经 :text 传入，与 CodeViewer 同源。
import { computed, nextTick, onMounted, ref, watch } from 'vue';
import {
  AlertTriangle,
  ChevronsDownUp,
  ChevronsUpDown,
  Frame,
  ListTree,
  Minus,
  Network,
  Plus,
  ZoomIn,
  ZoomOut,
} from 'lucide-vue-next';
import { parseStructured, type PFormat } from '../../model/structured/parse';
import { buildGraph, NODE_CAP, rowCenterY, type GEdge, type GNode, type GRow } from '../../model/structured/graph';
import { layoutGraph } from '../../model/structured/layout';
import { useGraphPanZoom } from '../../../../composables/useGraphPanZoom';
import StructuredTreeNode from './StructuredTreeNode.vue';

const props = defineProps<{
  text: string;
  format?: PFormat;
  query?: string;
}>();

const view = ref<'graph' | 'tree'>('graph');
const collapsed = ref<Set<string>>(new Set());
const canvasEl = ref<HTMLElement | null>(null);

const { panX, panY, zoom, isPanning, onWheel, onPointerDown, onPointerMove, onPointerUp, fit, zoomByButton, reset } =
  useGraphPanZoom();

// 解析 → 全量图（仅随文本/格式变；折叠不重建）。
const parsed = computed(() => parseStructured(props.text ?? '', props.format, tr));
const fullGraph = computed(() => (parsed.value.value ? buildGraph(parsed.value.value, tr) : null));

// 折叠感知的布局 + 可见子集。layoutGraph 会就地写回节点坐标。
const scene = computed(() => {
  const p = parsed.value;
  if (p.error === 'empty') return { kind: 'empty' as const, format: p.format };
  if (!p.value || !fullGraph.value) {
    return { kind: 'error' as const, format: p.format, message: p.error ?? tr("解析失败") };
  }
  const g = fullGraph.value;
  const b = layoutGraph(g.nodes, { collapsed: collapsed.value });
  const visNodes = g.nodes.filter((n) => b.visible.has(n.id));
  const visEdges = g.edges.filter((e) => b.visible.has(e.from) && b.visible.has(e.to));
  return {
    kind: 'ok' as const,
    format: p.format,
    value: p.value,
    nodes: visNodes,
    edges: visEdges,
    nodeById: new Map(visNodes.map((n) => [n.id, n] as const)),
    bounds: { width: b.width, height: b.height },
    nodeCount: g.nodeCount,
    overCap: g.overCap,
  };
});

const okScene = computed(() => (scene.value.kind === 'ok' ? scene.value : null));
const format = computed<PFormat>(() => scene.value.format);
const nodes = computed<GNode[]>(() => okScene.value?.nodes ?? []);
const edges = computed<GEdge[]>(() => okScene.value?.edges ?? []);
const bounds = computed(() => okScene.value?.bounds ?? { width: 0, height: 0 });
const nodeCount = computed(() => okScene.value?.nodeCount ?? 0);
const overCap = computed(() => okScene.value?.overCap ?? false);

const viewportStyle = computed(() => ({
  transform: `translate(${panX.value}px, ${panY.value}px) scale(${zoom.value})`,
}));

const q = computed(() => (props.query ?? '').trim().toLowerCase());
// 命中预计算：只随查询词 / 可见节点变化重算（computed 缓存），平移重渲染时
// 仅做 O(1) 的 Set.has，不会每帧重扫全部节点行。
const matched = computed(() => {
  const ids = new Set<string>();
  const rows = new Set<string>();
  const query = q.value;
  if (!query) return { ids, rows };
  for (const n of nodes.value) {
    let nodeHit = false;
    n.rows.forEach((r, i) => {
      const inRow =
        (r.key !== null && r.key.toLowerCase().includes(query)) ||
        r.preview.toLowerCase().includes(query);
      if (inRow) {
        rows.add(`${n.id}:${i}`);
        nodeHit = true;
      }
    });
    if (nodeHit) ids.add(n.id);
  }
  return { ids, rows };
});

function nodeStyle(n: GNode) {
  return { left: `${n.x}px`, top: `${n.y}px`, width: `${n.width}px` };
}

function rowTitle(r: GRow): string {
  const k = r.key !== null ? `${r.key}: ` : '';
  return `${k}${r.preview}`;
}

function edgePath(e: GEdge): string {
  const map = okScene.value?.nodeById;
  const from = map?.get(e.from);
  const to = map?.get(e.to);
  if (!from || !to) return '';
  const x1 = from.x + from.width;
  const y1 = from.y + rowCenterY(e.fromRowIndex);
  const x2 = to.x;
  const y2 = to.y + to.height / 2;
  const dx = Math.max(36, (x2 - x1) * 0.5);
  return `M ${x1} ${y1} C ${x1 + dx} ${y1}, ${x2 - dx} ${y2}, ${x2} ${y2}`;
}

// ── 折叠 ──
function toggle(childId: string): void {
  const s = new Set(collapsed.value);
  if (s.has(childId)) s.delete(childId);
  else s.add(childId);
  collapsed.value = s;
}
function expandAll(): void {
  collapsed.value = new Set();
  nextTick(doFit);
}
function collapseAll(): void {
  const g = fullGraph.value;
  if (!g) return;
  const s = new Set<string>();
  for (const n of g.nodes) if (n.parentId !== null) s.add(n.id);
  collapsed.value = s;
  nextTick(doFit);
}

// ── 缩放/适应 ──
function viewportSize() {
  const r = canvasEl.value?.getBoundingClientRect();
  return { width: r?.width ?? 0, height: r?.height ?? 0 };
}
function doFit(): void {
  if (!okScene.value || !canvasEl.value) return;
  fit(okScene.value.bounds, viewportSize());
}
function zoomIn(): void {
  zoomByButton(1.2, viewportSize());
}
function zoomOut(): void {
  zoomByButton(1 / 1.2, viewportSize());
}
function resetView(): void {
  reset();
}

function fitIfGraph(): void {
  if (view.value === 'graph') doFit();
}

onMounted(() => nextTick(fitIfGraph));
watch(
  () => [props.text, props.format],
  () => {
    collapsed.value = new Set();
    nextTick(fitIfGraph);
  },
);
watch(view, (v) => {
  if (v === 'graph') nextTick(fitIfGraph);
});
</script>

<style scoped>
.structured-viewer {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  background: var(--surface-primary);
}
/* ── 工具栏 ── */
.toolbar {
  flex: none;
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 40px;
  padding: 6px 12px;
  border-bottom: 1px solid var(--border-color);
  background: var(--surface-secondary);
}
.stat {
  font-size: 11px;
  color: var(--text-secondary);
}
.spacer {
  flex: 1;
}
.zoom {
  font-size: 11px;
  color: var(--text-secondary);
  min-width: 38px;
  text-align: center;
  cursor: pointer;
  user-select: none;
}

/* ── 状态行 ── */
.state-row {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--text-secondary);
  padding: 14px 16px;
}
.state-row.error {
  color: var(--sev-critical-fg);
}

/* ── 画布 ── */
.canvas {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: hidden;
  cursor: grab;
  background-color: var(--surface-primary);
  background-image: radial-gradient(var(--border-color) 1px, transparent 1px);
  background-size: 22px 22px;
}
.canvas.panning {
  cursor: grabbing;
}
.viewport {
  position: absolute;
  top: 0;
  left: 0;
  transform-origin: 0 0;
  will-change: transform;
}
.edges {
  position: absolute;
  top: 0;
  left: 0;
  overflow: visible;
  pointer-events: none;
}
.edge {
  fill: none;
  stroke: var(--border-color);
  stroke-width: 1.5;
}

.node {
  position: absolute;
  box-sizing: border-box;
  /* 6px 上下内边距 = graph.ts 的 NODE_PAD_Y，rowCenterY/节点高度据此对齐边端口。 */
  padding: 6px 0;
  background: var(--surface-secondary);
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  box-shadow: var(--card-shadow);
  overflow: hidden;
}
.node.match {
  border-color: var(--accent-color);
  box-shadow: 0 0 0 2px var(--primary-glow);
}
.node.prim {
  border-style: dashed;
}

.row {
  box-sizing: border-box;
  display: flex;
  align-items: center;
  gap: 4px;
  height: 22px;
  padding: 0 8px;
  font-family: var(--font-mono);
  font-size: 12px;
  line-height: 22px;
  white-space: nowrap;
}
.row + .row {
  border-top: 1px solid var(--border-color);
}
.row.hit {
  background: var(--sev-warn-bg);
}

.row-lead {
  width: 14px;
  flex: none;
  display: inline-flex;
  align-items: center;
  justify-content: center;
}
.row-toggle {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 14px;
  height: 14px;
  padding: 0;
  border: 1px solid var(--border-color);
  border-radius: 3px;
  background: var(--surface-tertiary);
  color: var(--text-secondary);
  cursor: pointer;
}
.row-toggle:hover {
  color: var(--text-primary);
  border-color: var(--accent-color);
}

.r-key {
  color: var(--accent-color);
  flex: none;
  max-width: 55%;
  overflow: hidden;
  text-overflow: ellipsis;
}
.r-colon {
  color: var(--text-secondary);
  flex: none;
}
.r-val {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  color: var(--text-primary);
}
.r-val.t-string {
  color: var(--sev-success-fg);
}
.r-val.t-number {
  color: var(--chart-3);
}
.r-val.t-boolean {
  color: var(--chart-4);
}
.r-val.t-null {
  color: var(--text-secondary);
  font-style: italic;
}
.r-val.t-object,
.r-val.t-array {
  color: var(--text-secondary);
}

.cap-banner {
  position: absolute;
  top: 8px;
  left: 8px;
  right: 8px;
  z-index: 2;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 10px;
  font-size: 11px;
  border: 1px solid var(--sev-warn-border);
  border-radius: var(--radius-sm);
  background: var(--sev-warn-bg);
  color: var(--sev-warn-fg);
}
.hint {
  position: absolute;
  bottom: 8px;
  right: 10px;
  font-size: 11px;
  color: var(--text-secondary);
  background: var(--glass-bg);
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-sm);
  padding: 2px 7px;
  pointer-events: none;
}

/* ── 树视图 ── */
.tree-wrap {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: 8px 6px;
}
</style>
