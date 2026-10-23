/** 树图布局：Bruls、Huizing 与 van Wijk 的 squarified 算法。
 *
 *  一行瓦片只在最差长宽比仍在改善时继续加宽，否则在剩余区域另起一行，得到接近
 *  正方形、便于阅读的马赛克，而不是切片法的细条。
 *
 *  布局以像素为单位，坐标系是未缩放的视口。要再分的目录先在顶部留出一条名称带，
 *  子项排在带下方：父目录的名字永远不压在子项上，名称带本身也属于父目录，
 *  不必瞄准边缘就能点中它。放不下名称带的目录保持完整、不再细分——名字压在
 *  子项上比少显示一层更糟，进入该目录就能看到细节。 */

/** 文件列表的两种视图：逐行的列表，与按空间占用的树图（仅 NTFS）。 */
export type FileViewMode = 'list' | 'treemap';

/** 分区或逻辑卷的根节点（`part:N` / `lvm:N:M`）。 */
export function isVolumeRootId(id: string): boolean {
  return /^part:\d+$/.test(id) || /^lvm:\d+:\d+$/.test(id);
}

/** 树图目前只服务 NTFS：NTFS 目录，或列出了 NTFS 条目的卷根。卷根是否 NTFS 由后端最终判定。 */
export function treemapAvailableAt(directoryId: string, entries: readonly { id: string }[]): boolean {
  if (directoryId.startsWith('ntfs:')) return true;
  return isVolumeRootId(directoryId) && entries.some(entry => entry.id.startsWith('ntfs:') || entry.id.startsWith('ntfs-ads:'));
}

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface TreemapOptions {
  /** 一次画出的层数；1 只画根的子项。 */
  maxDepth: number;
  /** 目录内兄弟之间的间隙，也是向内的缩进。 */
  padding: number;
  /** 顶层瓦片之间更宽的间隙，让第一层结构先于细节被读出。 */
  paddingOuter: number;
  /** 小于该像素的瓦片不画：既看不清也点不中。 */
  minTile: number;
  /** 顶层目录名称带的高度。 */
  header: number;
  /** 更深层打开的目录的名称带高度。 */
  headerInner: number;
  /** 宽度不足该值的目录不留名称带。 */
  minHeaderWidth: number;
}

export const DEFAULT_TREEMAP_OPTIONS: TreemapOptions = {
  maxDepth: 3,
  padding: 1,
  paddingOuter: 3,
  minTile: 6,
  header: 22,
  headerInner: 16,
  minHeaderWidth: 44,
};

/** 布局读取条目的方式；与具体的数据结构无关。 */
export interface TreemapSource<T> {
  value: (entry: T) => number;
  /** 显示的子项；叶子与未展开的目录为 null。 */
  children: (entry: T) => readonly T[] | null;
  /** 合并的尾部（`others`）的值，没有时为 0。 */
  othersValue: (entry: T) => number;
  isDirectory: (entry: T) => boolean;
}

export interface TreemapTile<T> {
  /** 条目；合并的尾部为 null，其所属目录见 `owner`。 */
  entry: T | null;
  /** 包含该瓦片的条目（顶层瓦片为视图根）。 */
  owner: T;
  /** 父瓦片在结果数组中的下标，顶层为 -1。 */
  parent: number;
  rect: Rect;
  /** 嵌套深度：0 为视图根的子项。 */
  depth: number;
  /** 打开的目录为自己的名称留出的带；其子项排在带下方。 */
  header: Rect | null;
}

export function insetRect(rect: Rect, padding: number): Rect {
  return {
    x: rect.x + padding,
    y: rect.y + padding,
    w: Math.max(0, rect.w - padding * 2),
    h: Math.max(0, rect.h - padding * 2),
  };
}

export function rectContains(rect: Rect, x: number, y: number): boolean {
  return x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h;
}

/** 打开的目录里，子项占据的区域：名称带下方的部分。 */
export function tileBody<T>(tile: TreemapTile<T>): Rect {
  if (!tile.header) return tile.rect;
  const top = tile.header.y + tile.header.h;
  return { x: tile.rect.x, y: top, w: tile.rect.w, h: tile.rect.y + tile.rect.h - top };
}

/** 把 `area` 按 `values` 的比例切成矩形，结果与 `values` 同序（不是布局顺序）。 */
export function squarify(values: readonly number[], area: Rect): Rect[] {
  const rects: Rect[] = values.map(() => ({ x: 0, y: 0, w: 0, h: 0 }));
  const total = values.reduce((sum, value) => (value > 0 ? sum + value : sum), 0);
  if (total <= 0 || area.w <= 0 || area.h <= 0) return rects;

  const order = values
    .map((value, index) => ({ value, index }))
    .filter(item => item.value > 0)
    .sort((left, right) => right.value - left.value)
    .map(item => item.index);
  const scale = (area.w * area.h) / total;
  const areas = order.map(index => values[index] * scale);

  const free = { ...area };
  let start = 0;
  while (start < areas.length) {
    const side = Math.min(free.w, free.h);
    let end = start + 1;
    let rowSum = areas[start];
    let rowWorst = worstRatio(areas, start, end, rowSum, side);
    while (end < areas.length) {
      const candidateSum = rowSum + areas[end];
      const candidateWorst = worstRatio(areas, start, end + 1, candidateSum, side);
      if (candidateWorst > rowWorst) break;
      rowSum = candidateSum;
      rowWorst = candidateWorst;
      end += 1;
    }

    if (free.w >= free.h) {
      // 左侧一列，瓦片自上而下。
      const stripW = Math.min(rowSum / free.h, free.w);
      let y = free.y;
      for (let index = start; index < end; index += 1) {
        const height = stripW > 0 ? Math.max(0, Math.min(areas[index] / stripW, free.y + free.h - y)) : 0;
        rects[order[index]] = { x: free.x, y, w: stripW, h: height };
        y += height;
      }
      free.x += stripW;
      free.w -= stripW;
    } else {
      // 顶部一行，瓦片从左到右。
      const stripH = Math.min(rowSum / free.w, free.h);
      let x = free.x;
      for (let index = start; index < end; index += 1) {
        const width = stripH > 0 ? Math.max(0, Math.min(areas[index] / stripH, free.x + free.w - x)) : 0;
        rects[order[index]] = { x, y: free.y, w: width, h: stripH };
        x += width;
      }
      free.y += stripH;
      free.h -= stripH;
    }
    start = end;
  }
  return rects;
}

/** 沿 `side` 排成一行的 `areas[start..end]` 中最差（最大）的长宽比。 */
function worstRatio(areas: readonly number[], start: number, end: number, rowSum: number, side: number): number {
  if (rowSum <= 0 || side <= 0) return Number.POSITIVE_INFINITY;
  const thickness = rowSum / side;
  let worst = 0;
  for (let index = start; index < end; index += 1) {
    const area = areas[index];
    if (area <= 0) continue;
    const other = area / thickness;
    worst = Math.max(worst, thickness / other, other / thickness);
  }
  return worst;
}

/** 名称带；瓦片太小、留不出可用的子项区域时为 null。顶层用整条带，更深层用细带。 */
function headerBand(rect: Rect, depth: number, options: TreemapOptions): Rect | null {
  const height = depth === 0 ? options.header : options.headerInner;
  if (rect.w < options.minHeaderWidth || rect.h - height < options.minTile * 3) return null;
  return { x: rect.x, y: rect.y, w: rect.w, h: height };
}

/** 布局 `root` 之下的全部内容。结果按深度优先、父先于子排列：按序绘制即可让
 *  子项盖在父项上，逆序查找即可命中最深的瓦片。 */
export function layoutTreemap<T>(root: T, area: Rect, source: TreemapSource<T>, options: TreemapOptions = DEFAULT_TREEMAP_OPTIONS): TreemapTile<T>[] {
  const tiles: TreemapTile<T>[] = [];
  placeChildren(root, area, 0, -1);
  return tiles;

  function placeChildren(node: T, region: Rect, depth: number, parent: number) {
    const children = source.children(node);
    if (!children || region.w <= 0 || region.h <= 0) return;
    // 自己按值排序：度量切换或手工构造的树也不能得出糟糕的布局。
    const ranked = children
      .filter(child => source.value(child) > 0)
      .sort((left, right) => source.value(right) - source.value(left));
    const values = ranked.map(child => source.value(child));
    const othersValue = source.othersValue(node);
    if (othersValue > 0) values.push(othersValue);
    if (values.length === 0) return;

    squarify(values, region).forEach((raw, slot) => {
      const rect = insetRect(raw, depth === 0 ? options.paddingOuter : options.padding);
      if (rect.w < options.minTile || rect.h < options.minTile) return;
      const child = slot < ranked.length ? ranked[slot] : null;
      if (child === null) {
        tiles.push({ entry: null, owner: node, parent, rect, depth, header: null });
        return;
      }
      const subdividable = source.isDirectory(child) && depth + 1 < options.maxDepth && source.children(child) !== null;
      const header = subdividable ? headerBand(rect, depth, options) : null;
      const index = tiles.length;
      tiles.push({ entry: child, owner: node, parent, rect, depth, header });
      if (header) placeChildren(child, tileBody(tiles[index]), depth + 1, index);
    });
  }
}

/** 包含某点的最深瓦片的下标。子项排在父项之后并缩进在父项之内，逆序第一个命中即最具体。 */
export function hitTile<T>(tiles: readonly TreemapTile<T>[], x: number, y: number): number {
  for (let index = tiles.length - 1; index >= 0; index -= 1) {
    if (rectContains(tiles[index].rect, x, y)) return index;
  }
  return -1;
}

/** 从视图根到 `index` 的瓦片下标链（含自身）。 */
export function tileChain<T>(tiles: readonly TreemapTile<T>[], index: number): number[] {
  const chain: number[] = [];
  for (let current = index; current >= 0; current = tiles[current]?.parent ?? -1) chain.unshift(current);
  return chain;
}

export type TreemapDirection = 'left' | 'right' | 'up' | 'down';

/** 从 `from` 到 `rect` 沿方向轴的距离；`rect` 不在该方向上时为 null。 */
export function directionGap(direction: TreemapDirection, from: Rect, rect: Rect): number | null {
  const epsilon = 0.5;
  let gap: number;
  switch (direction) {
    case 'right': gap = rect.x - (from.x + from.w); break;
    case 'left': gap = from.x - (rect.x + rect.w); break;
    case 'down': gap = rect.y - (from.y + from.h); break;
    case 'up': gap = from.y - (rect.y + rect.h); break;
  }
  return gap >= -epsilon ? Math.max(0, gap) : null;
}

/** `rect` 偏离方向轴的程度：同一行 / 列上的邻居优先于任意一侧的瓦片。 */
export function directionOffset(direction: TreemapDirection, from: Rect, rect: Rect): number {
  const overlap = (aStart: number, aEnd: number, bStart: number, bEnd: number) => Math.max(0, Math.min(aEnd, bEnd) - Math.max(aStart, bStart));
  if (direction === 'left' || direction === 'right') {
    return Math.max(0, Math.min(from.h, rect.h) - overlap(from.y, from.y + from.h, rect.y, rect.y + rect.h));
  }
  return Math.max(0, Math.min(from.w, rect.w) - overlap(from.x, from.x + from.w, rect.x, rect.x + rect.w));
}

/** 方向键要移到的同层瓦片：同一深度里沿方向最近、且最贴近方向轴的那个。
 *  只在同层移动——方向键一步跳进子项会让选中的层级无从预料。没有时为 -1。 */
export function neighbourTile<T>(tiles: readonly TreemapTile<T>[], from: number, direction: TreemapDirection): number {
  const origin = tiles[from];
  if (!origin) return -1;
  let best = -1;
  let bestScore = Number.POSITIVE_INFINITY;
  tiles.forEach((tile, index) => {
    if (index === from || tile.depth !== origin.depth) return;
    const gap = directionGap(direction, origin.rect, tile.rect);
    if (gap === null) return;
    const score = gap + directionOffset(direction, origin.rect, tile.rect) * 2.5;
    if (score < bestScore) {
      bestScore = score;
      best = index;
    }
  });
  return best;
}
