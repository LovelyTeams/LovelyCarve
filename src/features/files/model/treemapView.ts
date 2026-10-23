/** 树图的视图变换与层级过渡。
 *
 *  布局始终留在未缩放的基准坐标里（视口像素，倍率 1），平移和缩放只改变视图变换
 *  `屏幕 = (基准 - 原点) × 倍率`，不必重排。滚轮朝指针放大，直到指针下的目录正好
 *  填满视口；下一格就进入它，而那一刻它的内容已经和进入后一样大，所以「放大」和
 *  「进入」读起来是同一个连续动作。在倍率 1 再往回滚就回到上一级。
 *
 *  进出层级时不是移动镜头：每块瓦片从那片区域原先所在的位置画到它现在的位置，
 *  被进入目录里的瓦片就地长大，被离开的目录缩回它在上一级的瓦片里。 */
import { hitTile, tileBody, tileChain, type Rect, type TreemapTile } from './treemap';

export interface TreemapView {
  scale: number;
  originX: number;
  originY: number;
}

export const IDENTITY_VIEW: TreemapView = { scale: 1, originX: 0, originY: 0 };
export const MIN_VIEW_SCALE = 1;
/** 放大的上限：再往里滚就进入指针下的东西，而不是继续放大。 */
export const MAX_VIEW_SCALE = 5;
/** 滚轮每次事件的倍率。 */
export const WHEEL_ZOOM_FACTOR = 1.15;
/** `+` / `-` 每次的倍率（以视口中心为准，不进入目录）。 */
export const KEY_ZOOM_FACTOR = 1.25;
/** 像素滚动量折合成「行」的比例。 */
export const PIXELS_PER_LINE = 24;
/** 按住 Shift 滚动时每行平移的屏幕像素。 */
export const PAN_PIXELS_PER_LINE = 40;

const EPSILON = 1e-6;

export function projectRect(view: TreemapView, rect: Rect): Rect {
  return {
    x: (rect.x - view.originX) * view.scale,
    y: (rect.y - view.originY) * view.scale,
    w: rect.w * view.scale,
    h: rect.h * view.scale,
  };
}

/** 视口点 → 基准坐标点。 */
export function unprojectPoint(view: TreemapView, x: number, y: number): { x: number; y: number } {
  return { x: x / view.scale + view.originX, y: y / view.scale + view.originY };
}

/** 以 `(x, y)` 为中心按 `factor` 缩放，不超过 `ceiling`。指针下的基准点保持不动，
 *  缩放才像是对准了瓦片而不是窗口。 */
export function zoomViewAt(view: TreemapView, x: number, y: number, factor: number, ceiling: number): TreemapView {
  const scale = Math.min(Math.max(view.scale * factor, MIN_VIEW_SCALE), ceiling);
  const base = unprojectPoint(view, x, y);
  return { scale, originX: base.x - x / scale, originY: base.y - y / scale };
}

/** `rect` 恰好放进视口时的倍率。 */
export function fitScale(rect: Rect, width: number, height: number): number {
  if (rect.w <= 0 || rect.h <= 0) return MIN_VIEW_SCALE;
  return Math.min(Math.max(width, 1) / rect.w, Math.max(height, 1) / rect.h);
}

/** 视口当前显示的基准区域。 */
export function visibleBase(view: TreemapView, width: number, height: number): Rect {
  return { x: view.originX, y: view.originY, w: Math.max(width, 1) / view.scale, h: Math.max(height, 1) / view.scale };
}

/** 把视口限制在布局之内：任何时候都不留空白边。 */
export function clampView(view: TreemapView, width: number, height: number): TreemapView {
  const maxX = Math.max(width - width / view.scale, 0);
  const maxY = Math.max(height - height / view.scale, 0);
  return {
    scale: view.scale,
    originX: Math.min(Math.max(view.originX, 0), maxX),
    originY: Math.min(Math.max(view.originY, 0), maxY),
  };
}

/** 按住 Shift 滚动：在放大的视图里上下平移。`lines` 为正是向上滚。 */
export function panView(view: TreemapView, lines: number, width: number, height: number): TreemapView {
  return clampView({ ...view, originY: view.originY - (lines * PAN_PIXELS_PER_LINE) / view.scale }, width, height);
}

/** 一次进出层级的过渡：`src` 是那片区域在旧画面里的位置，`dst` 是同一区域在新画面里的位置（都是视口坐标）。 */
export interface LayoutTransition {
  src: Rect;
  dst: Rect;
  startedAt: number;
  /** 毫秒；跨度越大略长一些，但从不显得慢。 */
  duration: number;
}

export function layoutTransition(src: Rect, dst: Rect, startedAt: number): LayoutTransition {
  const ratio = dst.w > 1 ? src.w / dst.w : 1;
  const magnitude = Math.min(Math.max(Math.log2(Math.max(Math.abs(ratio), 1)), 0), 4);
  return { src, dst, startedAt, duration: 130 + Math.floor(magnitude * 45) };
}

/** 新布局里的一块矩形在切换之前位于哪里。 */
export function transitionOrigin(transition: LayoutTransition, rect: Rect): Rect {
  const scale = transition.dst.w > 0 ? transition.src.w / transition.dst.w : 1;
  return {
    x: (rect.x - transition.dst.x) * scale + transition.src.x,
    y: (rect.y - transition.dst.y) * scale + transition.src.y,
    w: rect.w * scale,
    h: rect.h * scale,
  };
}

/** 此刻要画的矩形，以及过渡是否仍在进行。 */
export function sampleTransition(transition: LayoutTransition, rect: Rect, now: number): { rect: Rect; running: boolean } {
  const elapsed = transition.duration > 0 ? (now - transition.startedAt) / transition.duration : 1;
  if (elapsed >= 1) return { rect, running: false };
  const eased = 1 - (1 - Math.max(elapsed, 0)) ** 3;
  const from = transitionOrigin(transition, rect);
  const lerp = (start: number, end: number) => start + (end - start) * eased;
  return {
    rect: { x: lerp(from.x, rect.x), y: lerp(from.y, rect.y), w: lerp(from.w, rect.w), h: lerp(from.h, rect.h) },
    running: true,
  };
}

/** 视口点下最深的、有内容可显示的目录瓦片：在那一点缩放就是朝它缩放。没有时为 -1。 */
export function zoomTarget<T>(
  tiles: readonly TreemapTile<T>[],
  view: TreemapView,
  x: number,
  y: number,
  hasContents: (tile: TreemapTile<T>) => boolean,
): number {
  const point = unprojectPoint(view, x, y);
  const hovered = hitTile(tiles, point.x, point.y);
  if (hovered < 0) return -1;
  const chain = tileChain(tiles, hovered);
  for (let position = chain.length - 1; position >= 0; position -= 1) {
    if (hasContents(tiles[chain[position]])) return chain[position];
  }
  return -1;
}

/** 一格缩放的结果：换一个视图（附上滚轮正朝向的目录，没有时为 -1）、进入某块目录（附上它此刻在屏幕上的内容区）、
 *  回到上一级，或什么都不做。 */
export type ZoomStep =
  | { kind: 'view'; view: TreemapView; target: number }
  | { kind: 'enter'; target: number; from: Rect }
  | { kind: 'ascend' }
  | { kind: 'none' };

/** 朝 `(x, y)` 缩放一格。`descend` 为真时（滚轮）放大停在指针下目录恰好填满视口的倍率，
 *  到了这个上限再放大就进入它；在最小倍率再缩小就回到上一级。键盘的 `+` / `-` 只放大缩小。 */
export function zoomStep<T>(
  tiles: readonly TreemapTile<T>[],
  view: TreemapView,
  width: number,
  height: number,
  x: number,
  y: number,
  factor: number,
  descend: boolean,
  hasContents: (tile: TreemapTile<T>) => boolean,
): ZoomStep {
  if (factor < 1 && view.scale <= MIN_VIEW_SCALE + EPSILON) return descend ? { kind: 'ascend' } : { kind: 'none' };
  const target = descend ? zoomTarget(tiles, view, x, y, hasContents) : -1;
  const body = target >= 0 ? tileBody(tiles[target]) : null;
  const ceiling = Math.min(Math.max(body ? fitScale(body, width, height) : MAX_VIEW_SCALE, MIN_VIEW_SCALE), MAX_VIEW_SCALE);
  if (factor > 1 && body && view.scale >= ceiling - EPSILON) return { kind: 'enter', target, from: projectRect(view, body) };
  return { kind: 'view', view: clampView(zoomViewAt(view, x, y, factor, ceiling), width, height), target };
}

/** 滚轮事件折合成行（正为向上滚，即放大）。 */
export function wheelLines(deltaY: number, deltaMode: number): number {
  return deltaMode === 0 ? -deltaY / PIXELS_PER_LINE : -deltaY;
}
