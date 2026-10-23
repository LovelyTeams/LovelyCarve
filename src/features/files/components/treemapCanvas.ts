/** 树图画布：瓦片画在一块画布上，标签文字直接画进去。
 *
 *  一屏可能有上千块瓦片，每块一个元素会把一帧耗在排版上；画出来还让删除斜线、
 *  选中框与悬停框在同一处、按同一顺序绘制。标签在屏幕坐标里挑选：放大腾出了地方，
 *  就多显示一些名字——这正是放大的意义。文字保持字号不变，只裁到它所属的区域里。 */
import type { Rect } from '../model/treemap';
import type { TreemapKind } from '../model/treemapKinds';
import {
  AGE_MIX, AGE_UNKNOWN_MIX, DEPTH_MIX, KIND_HUE_TOKENS, cssColor, mixColor, parseColor, withAlpha, type Rgba,
} from '../model/treemapPalette';
import { projectRect, sampleTransition, type LayoutTransition, type TreemapView } from '../model/treemapView';

export interface TreemapPalette {
  kindFills: Record<TreemapKind, string[]>;
  ageFills: string[];
  ageUnknown: string;
  text: string;
  textDeeper: string;
  textSecondary: string;
  sizeText: string;
  deleted: string;
  hover: string;
  selection: string;
  hatch: string;
  fontFamily: string;
}

/** 一块要画的瓦片：几何在基准坐标里，颜色已解析。 */
export interface PaintTile {
  rect: Rect;
  header: Rect | null;
  depth: number;
  fill: string;
  deleted: boolean;
  others: boolean;
  label: string | null;
  size: string | null;
}

export interface PaintFrame {
  tiles: readonly PaintTile[];
  view: TreemapView;
  transition: LayoutTransition | null;
  now: number;
  width: number;
  height: number;
  pixelRatio: number;
  hovered: number;
  selected: number;
  palette: TreemapPalette;
}

const NAME_SIZE = 12;
const SIZE_SIZE = 11;
const LINE_HEIGHT = NAME_SIZE * 1.35;
const TEXT_PADDING = NAME_SIZE * 0.42;
const TEXT_INSET = NAME_SIZE * 0.25;
const SIZE_GAP = NAME_SIZE * 0.67;
/** 小于这个屏幕尺寸的区域不放名字：放了也看不清。 */
const LABEL_MIN_WIDTH = 54;
const LABEL_MIN_HEIGHT = 15;
/** 一帧最多排的标签数，面积大的优先。 */
const MAX_LABELS = 150;
const HATCH_SPACING = 6;

const FALLBACK = { neutral: { r: 128, g: 128, b: 128, a: 1 }, surface: { r: 255, g: 255, b: 255, a: 1 }, text: { r: 15, g: 23, b: 42, a: 1 } };

/** 从 `element` 所在主题读出树图配色。`probe` 用来把任何 CSS 颜色写法规范成可解析的形式。 */
export function resolvePalette(element: Element, probe: CanvasRenderingContext2D): TreemapPalette {
  const styles = getComputedStyle(element);
  const read = (token: string, fallback: Rgba): Rgba => {
    const raw = styles.getPropertyValue(token).trim();
    if (!raw) return fallback;
    const parsed = parseColor(raw);
    if (parsed) return parsed;
    probe.fillStyle = '#000000';
    probe.fillStyle = raw;
    return parseColor(String(probe.fillStyle)) ?? fallback;
  };
  const surface = read('--surface-secondary', FALLBACK.surface);
  const text = read('--text-primary', FALLBACK.text);
  const textSecondary = read('--text-secondary', FALLBACK.neutral);
  const primary = read('--primary-color', FALLBACK.neutral);
  const critical = read('--sev-critical-fg', FALLBACK.neutral);
  const kindFills = {} as Record<TreemapKind, string[]>;
  for (const [kind, token] of Object.entries(KIND_HUE_TOKENS) as [TreemapKind, string][]) {
    const hue = read(token, FALLBACK.neutral);
    kindFills[kind] = DEPTH_MIX.map(mix => cssColor(mixColor(hue, surface, mix)));
  }
  return {
    kindFills,
    ageFills: AGE_MIX.map(mix => cssColor(mixColor(primary, surface, mix))),
    ageUnknown: cssColor(mixColor(textSecondary, surface, AGE_UNKNOWN_MIX)),
    text: cssColor(text),
    textDeeper: cssColor(withAlpha(text, 0.88)),
    textSecondary: cssColor(textSecondary),
    sizeText: cssColor(withAlpha(text, 0.72)),
    deleted: cssColor(critical),
    hover: cssColor(withAlpha(text, 0.5)),
    selection: cssColor(primary),
    hatch: cssColor(withAlpha(critical, 0.4)),
    fontFamily: styles.fontFamily || 'sans-serif',
  };
}

const hatches = new WeakMap<CanvasRenderingContext2D, { key: string; pattern: CanvasPattern | null }>();

/** 斜线阴影：每 6 像素一条 1 像素的「/」，按设备像素生成，放大屏上也清晰。按画布缓存。 */
function hatchPattern(context: CanvasRenderingContext2D, color: string, pixelRatio: number): CanvasPattern | null {
  const key = `${color}@${pixelRatio}`;
  const cached = hatches.get(context);
  if (cached?.key === key) return cached.pattern;
  const pattern = createHatch(context, color, pixelRatio);
  hatches.set(context, { key, pattern });
  return pattern;
}

function createHatch(context: CanvasRenderingContext2D, color: string, pixelRatio: number): CanvasPattern | null {
  if (typeof document === 'undefined') return null;
  const size = Math.max(1, Math.round(HATCH_SPACING * pixelRatio));
  const tile = document.createElement('canvas');
  tile.width = size;
  tile.height = size;
  const draw = tile.getContext('2d');
  if (!draw) return null;
  draw.strokeStyle = color;
  draw.lineWidth = Math.max(1, pixelRatio);
  draw.beginPath();
  for (const offset of [-size, 0, size]) {
    draw.moveTo(offset, size);
    draw.lineTo(offset + size, 0);
  }
  draw.stroke();
  const pattern = context.createPattern(tile, 'repeat');
  if (pattern && typeof DOMMatrix !== 'undefined') pattern.setTransform(new DOMMatrix([1 / pixelRatio, 0, 0, 1 / pixelRatio, 0, 0]));
  return pattern;
}

/** 把矩形的四条边各自对齐到设备像素：只取整尺寸会让相邻瓦片之间漏出缝或叠一条线。 */
function snap(rect: Rect, pixelRatio: number): Rect {
  const round = (value: number) => Math.round(value * pixelRatio) / pixelRatio;
  const left = round(rect.x);
  const top = round(rect.y);
  return { x: left, y: top, w: Math.max(0, round(rect.x + rect.w) - left), h: Math.max(0, round(rect.y + rect.h) - top) };
}

function visible(rect: Rect, width: number, height: number): boolean {
  return rect.x < width && rect.y < height && rect.x + rect.w > 0 && rect.y + rect.h > 0;
}

/** 量过的文字宽度；字体与文字都不变时直接复用。 */
const widths = new Map<string, number>();
function measure(context: CanvasRenderingContext2D, font: string, text: string): number {
  const key = `${font}\u0000${text}`;
  let width = widths.get(key);
  if (width === undefined) {
    if (widths.size > 4_000) widths.clear();
    context.font = font;
    width = context.measureText(text).width;
    widths.set(key, width);
  }
  return width;
}

/** 画一帧，返回过渡是否仍在进行（是则需要下一帧）。 */
export function paintTreemap(context: CanvasRenderingContext2D, frame: PaintFrame): boolean {
  const { tiles, view, transition, now, width, height, pixelRatio, palette } = frame;
  context.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0);
  context.clearRect(0, 0, width, height);
  let running = false;
  const animate = (rect: Rect): Rect => {
    if (!transition) return rect;
    const sample = sampleTransition(transition, rect, now);
    running ||= sample.running;
    return sample.rect;
  };

  const drawn: (Rect | null)[] = new Array(tiles.length).fill(null);
  const hatch = tiles.some(tile => tile.deleted) ? hatchPattern(context, palette.hatch, pixelRatio) : null;
  tiles.forEach((tile, index) => {
    const rect = projectRect(view, animate(tile.rect));
    if (rect.w <= 0.5 || rect.h <= 0.5 || !visible(rect, width, height)) return;
    const box = snap(rect, pixelRatio);
    drawn[index] = box;
    context.fillStyle = tile.fill;
    context.fillRect(box.x, box.y, box.w, box.h);
    if (tile.deleted && hatch) {
      context.fillStyle = hatch;
      context.fillRect(box.x, box.y, box.w, box.h);
    }
  });

  // 外框在所有填充之后画：目录的子瓦片会盖住它自己的填充，先画的选中框只会剩缝隙里的几段。
  const ring = (index: number, lineWidth: number, color: string) => {
    const box = drawn[index];
    if (!box || box.w <= lineWidth || box.h <= lineWidth) return;
    context.strokeStyle = color;
    context.lineWidth = lineWidth;
    context.strokeRect(box.x + lineWidth / 2, box.y + lineWidth / 2, box.w - lineWidth, box.h - lineWidth);
  };
  if (frame.hovered >= 0 && frame.hovered !== frame.selected) ring(frame.hovered, 1, palette.hover);
  if (frame.selected >= 0) ring(frame.selected, 2, palette.selection);

  paintLabels(context, frame, drawn, animate);
  return running;
}

function paintLabels(
  context: CanvasRenderingContext2D,
  frame: PaintFrame,
  drawn: readonly (Rect | null)[],
  animate: (rect: Rect) => Rect,
) {
  const { tiles, view, width, height, palette } = frame;
  const candidates: { index: number; area: number; owned: Rect }[] = [];
  tiles.forEach((tile, index) => {
    if (!drawn[index] || !tile.label) return;
    const base = animate(tile.rect);
    const owned = projectRect(view, tile.header ? animate(tile.header) : base);
    if (owned.w < LABEL_MIN_WIDTH || owned.h < LABEL_MIN_HEIGHT || !visible(owned, width, height)) return;
    candidates.push({ index, area: base.w * base.h, owned });
  });
  candidates.sort((left, right) => right.area - left.area);
  context.textBaseline = 'alphabetic';
  context.textAlign = 'left';
  for (const { index, owned } of candidates.slice(0, MAX_LABELS)) {
    const tile = tiles[index];
    const label = tile.label ?? '';
    const mask = owned;
    const originX = mask.x + TEXT_PADDING;
    const baseline = mask.y + TEXT_INSET + NAME_SIZE;
    const bold = tile.depth === 0 && tile.header !== null;
    const nameFont = `${tile.others ? 'italic ' : ''}${bold ? 600 : 400} ${NAME_SIZE}px ${palette.fontFamily}`;
    const sizeFont = `400 ${SIZE_SIZE}px ${palette.fontFamily}`;
    const nameWidth = measure(context, nameFont, label);
    context.save();
    context.beginPath();
    context.rect(mask.x, mask.y, mask.w, mask.h);
    context.clip();
    context.font = nameFont;
    context.fillStyle = tile.deleted ? palette.deleted : tile.others ? palette.textSecondary : tile.depth === 0 ? palette.text : palette.textDeeper;
    context.fillText(label, originX, baseline);
    if (tile.deleted) {
      context.fillRect(originX, baseline - NAME_SIZE * 0.32, nameWidth, 1);
    }
    if (tile.size) {
      const sizeWidth = measure(context, sizeFont, tile.size);
      context.font = sizeFont;
      context.fillStyle = palette.sizeText;
      // 收起的瓦片够高就把大小放在名字下面；顶层名称带把大小靠右排成一列；
      // 其余跟在名字后面，放不下就不放。
      const stacked = tile.header === null && mask.h >= LINE_HEIGHT * 2 + TEXT_INSET;
      if (stacked) {
        context.fillText(tile.size, originX, baseline + LINE_HEIGHT * 0.92);
      } else if (tile.header !== null && tile.depth === 0) {
        const sizeX = mask.x + mask.w - sizeWidth - TEXT_PADDING;
        if (sizeX > originX + nameWidth + TEXT_PADDING) context.fillText(tile.size, sizeX, baseline);
      } else if (nameWidth + SIZE_GAP + sizeWidth <= mask.w - TEXT_PADDING * 2) {
        context.fillText(tile.size, originX + nameWidth + SIZE_GAP, baseline);
      }
    }
    context.restore();
  }
}
