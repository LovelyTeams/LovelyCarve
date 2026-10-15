import { sourceText, type Translator } from '../../../../i18n/translate';
// PValue（有序值树）→ 节点图模型 {nodes, edges}，仿 jsoncrack 的形态：
// 每个容器（object/array）是一个多行节点，行内原子值就地显示（key: value），
// 行内嵌套容器则从该行引一条边到子节点。原子根（整文件只是个标量）退化为单节点。
//
// 几何常量在此导出，layout.ts 与 StructuredViewer.vue 共用——边的起点要落在
// 父节点「那一行」的竖直中心，三处必须用同一套 ROW_H / 内边距。
import type { PValue } from './parse';

export type PType = 'object' | 'array' | 'string' | 'number' | 'boolean' | 'null';

/** 节点内的一行。容器行带 childId（连向子节点），原子行 childId 为 null。 */
export interface GRow {
  /** object 的键 / array 的下标字符串；原子根节点为 null。 */
  key: string | null;
  /** 原子行：值文本（截断）；容器行：摘要 {N} / [N]。 */
  preview: string;
  valueType: PType;
  childId: string | null;
}

export interface GNode {
  id: string;
  parentId: string | null;
  depth: number;
  kind: 'object' | 'array' | 'primitive';
  rows: GRow[];
  /** 行序排列的子节点 id（= 引出边的目标），layout 据此摆放子树。 */
  childIds: string[];
  hasChildren: boolean;
  width: number;
  height: number;
  // 由 layout.ts 写入。
  x: number;
  y: number;
}

export interface GEdge {
  id: string;
  from: string;
  to: string;
  /** 边在父节点中引出的行下标，用于把边起点对到那一行。 */
  fromRowIndex: number;
}

export interface GraphResult {
  nodes: GNode[];
  edges: GEdge[];
  nodeCount: number;
  /** 触及 NODE_CAP：深层容器被截成不可展开的叶子行，需提示用户。 */
  overCap: boolean;
}

// ── 几何 ──────────────────────────────────────────────────────────────────
export const ROW_H = 22;
export const NODE_PAD_Y = 6;
const CHAR_PX = 7.2; // Cascadia Mono ~13px 的近似字宽
const PAD_X = 26; // 行左右内边距 + 折叠角标余量
const MIN_W = 140;
const MAX_W = 460;
const PREVIEW_MAX = 60;

/** 第 i 行在节点局部坐标里的竖直中心。 */
export function rowCenterY(i: number): number {
  return NODE_PAD_Y + i * ROW_H + ROW_H / 2;
}

/** 节点总数上限——超过则停止下钻，避免巨型文件卡死布局/渲染。 */
export const NODE_CAP = 2000;

/** 单个节点的最大行数——扁平超大容器只渲染前 ROW_CAP 行 + 一条省略行。 */
const ROW_CAP = 200;

export function buildGraph(value: PValue, tr: Translator = sourceText): GraphResult {
  const ctx: BuildCtx = { nodes: [], edges: [], idSeq: 0, overCap: false };

  if (value.t === 'object' || value.t === 'array') {
    buildContainer(ctx, value, 0, null, tr);
  } else {
    const id = `n${ctx.idSeq++}`;
    const node: GNode = {
      id,
      parentId: null,
      depth: 0,
      kind: 'primitive',
      rows: [{ key: null, preview: primitiveText(value), valueType: value.t, childId: null }],
      childIds: [],
      hasChildren: false,
      width: 0,
      height: 0,
      x: 0,
      y: 0,
    };
    sizeNode(node);
    ctx.nodes.push(node);
  }

  return { nodes: ctx.nodes, edges: ctx.edges, nodeCount: ctx.nodes.length, overCap: ctx.overCap };
}

interface BuildCtx {
  nodes: GNode[];
  edges: GEdge[];
  idSeq: number;
  overCap: boolean;
}

function buildContainer(
  ctx: BuildCtx,
  v: Extract<PValue, { t: 'object' | 'array' }>,
  depth: number,
  parentId: string | null,
  tr: Translator = sourceText,
): GNode {
  const id = `n${ctx.idSeq++}`;
  const node: GNode = {
    id,
    parentId,
    depth,
    kind: v.t,
    rows: [],
    childIds: [],
    hasChildren: false,
    width: 0,
    height: 0,
    x: 0,
    y: 0,
  };
  ctx.nodes.push(node);

  const allItems: [string, PValue][] =
    v.t === 'object' ? v.entries : v.items.map((it, i) => [String(i), it]);
  // 单节点行数上限：扁平的超大对象/数组（如 1MiB 文件里的几万项）否则会渲染成
  // 一个几万行、上万像素高的巨型节点直接卡死。截断行不参与下钻，尾部补一条省略行。
  const truncated = allItems.length > ROW_CAP;
  const items = truncated ? allItems.slice(0, ROW_CAP) : allItems;
  if (truncated) ctx.overCap = true;

  items.forEach(([key, val], rowIndex) => {
    if (val.t === 'object' || val.t === 'array') {
      const len = val.t === 'object' ? val.entries.length : val.items.length;
      const expandable = len > 0 && ctx.nodes.length < NODE_CAP;
      if (expandable) {
        const child = buildContainer(ctx, val, depth + 1, id, tr);
        ctx.edges.push({ id: `e_${id}_${child.id}`, from: id, to: child.id, fromRowIndex: rowIndex });
        node.childIds.push(child.id);
        node.hasChildren = true;
        node.rows.push({ key, preview: summary(val), valueType: val.t, childId: child.id });
      } else {
        if (len > 0) ctx.overCap = true; // 非空但被 cap 截断
        node.rows.push({ key, preview: summary(val), valueType: val.t, childId: null });
      }
    } else {
      node.rows.push({ key, preview: primitiveText(val), valueType: val.t, childId: null });
    }
  });

  if (truncated) {
    node.rows.push({
      key: null,
      preview: tr("… 其余 {0} 项已省略", { 0: (allItems.length - ROW_CAP).toLocaleString() }),
      valueType: 'null',
      childId: null,
    });
  }

  sizeNode(node);
  return node;
}

function summary(v: Extract<PValue, { t: 'object' | 'array' }>): string {
  return v.t === 'object' ? `{${v.entries.length}}` : `[${v.items.length}]`;
}

function primitiveText(v: PValue): string {
  switch (v.t) {
    case 'string':
      return truncate(v.v, PREVIEW_MAX);
    case 'number':
      return String(v.v);
    case 'boolean':
      return v.v ? 'true' : 'false';
    case 'null':
      return 'null';
    case 'object':
      return `{${v.entries.length}}`;
    case 'array':
      return `[${v.items.length}]`;
  }
}

function truncate(s: string, max: number): string {
  const oneLine = s.replace(/\s+/g, ' ');
  return oneLine.length > max ? `${oneLine.slice(0, max)}…` : oneLine;
}

function sizeNode(node: GNode): void {
  let maxChars = 1;
  for (const r of node.rows) {
    const keyPart = r.key !== null ? `${r.key}: ` : '';
    maxChars = Math.max(maxChars, keyPart.length + r.preview.length);
  }
  node.width = Math.min(MAX_W, Math.max(MIN_W, Math.round(maxChars * CHAR_PX + PAD_X)));
  node.height = node.rows.length * ROW_H + NODE_PAD_Y * 2;
}
