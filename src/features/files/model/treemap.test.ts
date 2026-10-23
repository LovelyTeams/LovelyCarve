import { describe, expect, it } from 'vitest';
import {
  DEFAULT_TREEMAP_OPTIONS, directionGap, directionOffset, hitTile, layoutTreemap, neighbourTile, rectContains,
  squarify, tileBody, tileChain, type Rect, type TreemapSource,
} from './treemap';

interface Node {
  name: string;
  value: number;
  children: Node[] | null;
  others?: number;
}

const file = (name: string, value: number): Node => ({ name, value, children: null });
const dir = (name: string, children: Node[], others = 0): Node => ({
  name,
  value: children.reduce((sum, child) => sum + child.value, 0) + others,
  children,
  others,
});

const source: TreemapSource<Node> = {
  value: node => node.value,
  children: node => node.children,
  othersValue: node => node.others ?? 0,
  isDirectory: node => node.children !== null,
};

const area: Rect = { x: 0, y: 0, w: 800, h: 500 };
const areaOf = (rect: Rect) => rect.w * rect.h;
const names = (tiles: ReturnType<typeof layoutTreemap<Node>>) => tiles.map(tile => tile.entry?.name ?? `${tile.owner.name}:others`);

describe('squarify', () => {
  it('fills the area with every rectangle inside it', () => {
    const rects = squarify([40, 30, 20, 5, 3, 2], area);
    expect(rects.reduce((sum, rect) => sum + areaOf(rect), 0)).toBeCloseTo(areaOf(area), 0);
    for (const rect of rects) {
      expect(rect.x).toBeGreaterThanOrEqual(-0.01);
      expect(rect.y).toBeGreaterThanOrEqual(-0.01);
      expect(rect.x + rect.w).toBeLessThanOrEqual(area.w + 0.01);
      expect(rect.y + rect.h).toBeLessThanOrEqual(area.h + 0.01);
    }
  });

  it('keeps aspect ratios reasonable on the canonical example', () => {
    // 6, 6, 4, 3, 2, 2, 1 on a 6×4 canvas.
    for (const rect of squarify([6, 6, 4, 3, 2, 2, 1], { x: 0, y: 0, w: 600, h: 400 })) {
      expect(Math.max(rect.w / rect.h, rect.h / rect.w)).toBeLessThanOrEqual(4);
    }
  });

  it('returns rectangles in the order of its values, sized by them', () => {
    const rects = squarify([1, 3], { x: 0, y: 0, w: 400, h: 100 });
    expect(areaOf(rects[1]) / areaOf(rects[0])).toBeCloseTo(3, 5);
  });

  it('survives degenerate input', () => {
    expect(squarify([], area)).toEqual([]);
    expect(squarify([0, 0], area).every(rect => areaOf(rect) === 0)).toBe(true);
    expect(areaOf(squarify([1], { x: 0, y: 0, w: 0, h: 10 })[0])).toBe(0);
  });
});

describe('layoutTreemap', () => {
  it('places children inside their parent, parents before children', () => {
    const root = dir('root', [dir('big', [file('inside', 100), file('also', 50)]), file('small', 10)]);
    const tiles = layoutTreemap(root, area, source);
    expect(names(tiles)).toEqual(['big', 'inside', 'also', 'small']);
    const [parent, child] = tiles;
    expect(child.depth).toBe(1);
    expect(child.parent).toBe(0);
    expect(rectContains(parent.rect, child.rect.x, child.rect.y)).toBe(true);
    expect(rectContains(parent.rect, child.rect.x + child.rect.w - 0.5, child.rect.y + child.rect.h - 0.5)).toBe(true);
  });

  it('stops at the direct children with one level', () => {
    const root = dir('root', [dir('big', [file('inside', 100)])]);
    const tiles = layoutTreemap(root, area, source, { ...DEFAULT_TREEMAP_OPTIONS, maxDepth: 1 });
    expect(names(tiles)).toEqual(['big']);
    expect(tiles[0].header).toBeNull();
  });

  it('keeps a band for an open directory name, above all of its contents', () => {
    const root = dir('root', [dir('big', [file('inside', 100), file('also', 50)])]);
    const tiles = layoutTreemap(root, area, source);
    const header = tiles[0].header!;
    expect(header.y).toBe(tiles[0].rect.y);
    expect(header.h).toBe(DEFAULT_TREEMAP_OPTIONS.header);
    for (const tile of tiles.slice(1)) expect(tile.rect.y).toBeGreaterThanOrEqual(header.y + header.h);
    expect(tileBody(tiles[0]).y).toBe(header.y + header.h);
    // The band is the parent's own: hitting it finds the parent.
    expect(hitTile(tiles, header.x + header.w / 2, header.y + header.h / 2)).toBe(0);
  });

  it('leaves a directory whole when there is no room for its band', () => {
    const root = dir('root', [dir('big', [file('inside', 100)])]);
    const tiles = layoutTreemap(root, area, source, { ...DEFAULT_TREEMAP_OPTIONS, header: 400, minTile: 150 });
    expect(tiles).toHaveLength(1);
    expect(tiles[0].header).toBeNull();
  });

  it('never opens a leaf or a directory the view did not expand', () => {
    const closed: Node = { name: 'closed', value: 40, children: null };
    const root = dir('root', [file('solo', 10), { ...closed }]);
    const tiles = layoutTreemap(root, area, { ...source, isDirectory: node => node.name === 'closed' });
    expect(tiles.every(tile => tile.header === null)).toBe(true);
  });

  it('draws the merged tail as one tile of its owner', () => {
    const root = dir('root', [file('a', 50), file('b', 30)], 20);
    const tiles = layoutTreemap(root, area, source);
    expect(names(tiles)).toEqual(['a', 'b', 'root:others']);
    expect(tiles[2].entry).toBeNull();
    expect(tiles[2].owner).toBe(root);
  });

  it('drops tiles too small to draw and anything worth nothing', () => {
    const root = dir('root', [file('huge', 1_000_000), file('speck', 1), file('nothing', 0)]);
    expect(names(layoutTreemap(root, area, source))).toEqual(['huge']);
  });
});

describe('hit testing and movement', () => {
  const root = dir('root', [dir('big', [file('inside', 100), file('also', 60)]), file('small', 40), file('tiny', 30)]);
  const tiles = layoutTreemap(root, area, source);

  it('finds the deepest tile under a point and the chain above it', () => {
    const inside = names(tiles).indexOf('inside');
    const rect = tiles[inside].rect;
    expect(hitTile(tiles, rect.x + rect.w / 2, rect.y + rect.h / 2)).toBe(inside);
    expect(hitTile(tiles, -10, -10)).toBe(-1);
    expect(tileChain(tiles, inside)).toEqual([0, inside]);
  });

  it('only sees gaps on the side a direction points to', () => {
    const from = { x: 100, y: 100, w: 50, h: 50 };
    const right = { x: 200, y: 100, w: 50, h: 50 };
    const left = { x: 0, y: 100, w: 50, h: 50 };
    expect(directionGap('right', from, right)).toBe(50);
    expect(directionGap('right', from, left)).toBeNull();
    expect(directionGap('left', from, left)).toBe(50);
    expect(directionGap('down', from, right)).toBeNull();
    expect(directionOffset('right', from, right)).toBe(0);
    expect(directionOffset('right', from, { x: 200, y: 160, w: 20, h: 20 })).toBeGreaterThan(0);
  });

  it('moves between tiles of the same level only', () => {
    const big = names(tiles).indexOf('big');
    const small = names(tiles).indexOf('small');
    const towardSmall = (['left', 'right', 'up', 'down'] as const).find(direction => neighbourTile(tiles, big, direction) === small);
    expect(towardSmall).toBeDefined();
    for (const direction of ['left', 'right', 'up', 'down'] as const) {
      const next = neighbourTile(tiles, big, direction);
      if (next >= 0) expect(tiles[next].depth).toBe(0);
    }
  });
});
