import { describe, expect, it } from 'vitest';
import { DEFAULT_TREEMAP_OPTIONS, hitTile, layoutTreemap, tileBody, type Rect, type TreemapSource, type TreemapTile } from './treemap';
import {
  IDENTITY_VIEW, MAX_VIEW_SCALE, WHEEL_ZOOM_FACTOR, clampView, fitScale, layoutTransition, panView, projectRect,
  sampleTransition, transitionOrigin, unprojectPoint, visibleBase, wheelLines, zoomStep, zoomViewAt, type TreemapView,
} from './treemapView';

interface Node {
  name: string;
  value: number;
  children: Node[] | null;
}

const file = (name: string, value: number): Node => ({ name, value, children: null });
const dir = (name: string, children: Node[]): Node => ({ name, value: children.reduce((sum, child) => sum + child.value, 0), children });
const source: TreemapSource<Node> = {
  value: node => node.value,
  children: node => node.children,
  othersValue: () => 0,
  isDirectory: node => node.children !== null,
};
const hasContents = (tile: TreemapTile<Node>) => tile.entry?.children !== null && (tile.entry?.value ?? 0) > 0;

const WIDTH = 800;
const HEIGHT = 600;
const root = dir('root', [
  dir('big', [file('a', 300), dir('nested', [file('b', 120), file('c', 80)])]),
  file('solo', 200),
]);
const tiles = layoutTreemap(root, { x: 0, y: 0, w: WIDTH, h: HEIGHT }, source, DEFAULT_TREEMAP_OPTIONS);
const indexOf = (name: string) => tiles.findIndex(tile => tile.entry?.name === name);
const centre = (rect: Rect) => ({ x: rect.x + rect.w / 2, y: rect.y + rect.h / 2 });

describe('treemap view transform', () => {
  const view: TreemapView = { scale: 2, originX: 100, originY: 50 };

  it('projects and unprojects as inverses', () => {
    const point = unprojectPoint(view, 400, 300);
    const screen = projectRect(view, { x: point.x, y: point.y, w: 10, h: 10 });
    expect(screen.x).toBeCloseTo(400, 3);
    expect(screen.y).toBeCloseTo(300, 3);
    expect(screen.w).toBeCloseTo(20, 3);
  });

  it('keeps the point under the cursor still while zooming', () => {
    const before = unprojectPoint(IDENTITY_VIEW, 300, 200);
    const zoomed = zoomViewAt(IDENTITY_VIEW, 300, 200, 2, MAX_VIEW_SCALE);
    const screen = projectRect(zoomed, { x: before.x, y: before.y, w: 1, h: 1 });
    expect(screen.x).toBeCloseTo(300, 2);
    expect(screen.y).toBeCloseTo(200, 2);
  });

  it('never leaves a margin', () => {
    expect(clampView({ scale: 2, originX: -500, originY: 900 }, WIDTH, HEIGHT)).toEqual({ scale: 2, originX: 0, originY: 300 });
    expect(clampView({ scale: 1, originX: 40, originY: 40 }, WIDTH, HEIGHT)).toEqual(IDENTITY_VIEW);
    expect(visibleBase({ scale: 2, originX: 100, originY: 50 }, WIDTH, HEIGHT)).toEqual({ x: 100, y: 50, w: 400, h: 300 });
  });

  it('pans a magnified view up and down, within the layout', () => {
    const magnified = { scale: 2, originX: 0, originY: 100 };
    expect(panView(magnified, 1, WIDTH, HEIGHT).originY).toBe(80);
    expect(panView(magnified, -100, WIDTH, HEIGHT).originY).toBe(300);
    expect(panView(IDENTITY_VIEW, -3, WIDTH, HEIGHT)).toEqual(IDENTITY_VIEW);
  });

  it('reads a wheel scrolled away from the user as a zoom in', () => {
    expect(wheelLines(-120, 0)).toBe(5);
    expect(wheelLines(3, 1)).toBe(-3);
    expect(fitScale({ x: 0, y: 0, w: 400, h: 200 }, WIDTH, HEIGHT)).toBe(2);
  });
});

describe('treemap layout transitions', () => {
  it('starts where the region was and ends where it is', () => {
    // A region that filled the viewport lands in the top-left quarter of the new layout:
    // everything inside it starts twice as large, where it was.
    const transition = layoutTransition({ x: 0, y: 0, w: 800, h: 600 }, { x: 0, y: 0, w: 400, h: 300 }, 0);
    const tile = { x: 200, y: 150, w: 200, h: 150 };
    expect(transitionOrigin(transition, tile)).toEqual({ x: 400, y: 300, w: 400, h: 300 });
    expect(sampleTransition(transition, tile, 10).running).toBe(true);
    expect(sampleTransition(transition, tile, 10_000)).toEqual({ rect: tile, running: false });
  });

  it('grows the tiles of a directory being entered', () => {
    const transition = layoutTransition({ x: 100, y: 100, w: 200, h: 200 }, { x: 0, y: 0, w: 800, h: 600 }, 0);
    const tile = { x: 400, y: 300, w: 100, h: 100 };
    const origin = transitionOrigin(transition, tile);
    expect(origin.w).toBeLessThan(tile.w);
    expect(origin.x).toBeGreaterThan(100);
    expect(origin.x).toBeLessThan(300);
  });

  it('takes a little longer for longer pulls, never more than 310 ms', () => {
    expect(layoutTransition({ x: 0, y: 0, w: 100, h: 100 }, { x: 0, y: 0, w: 800, h: 600 }, 0).duration).toBe(130);
    expect(layoutTransition({ x: 0, y: 0, w: 1600, h: 100 }, { x: 0, y: 0, w: 400, h: 100 }, 0).duration).toBe(220);
    expect(layoutTransition({ x: 0, y: 0, w: 1e6, h: 1 }, { x: 0, y: 0, w: 10, h: 1 }, 0).duration).toBe(310);
  });
});

describe('zoom steps', () => {
  it('magnifies toward the pointer until the directory under it fills the view, then goes in', () => {
    const nested = indexOf('nested');
    const point = centre(tileBody(tiles[nested]));
    const ceiling = Math.min(fitScale(tileBody(tiles[nested]), WIDTH, HEIGHT), MAX_VIEW_SCALE);
    let view = IDENTITY_VIEW;
    const scales: number[] = [];
    for (let notch = 0; notch < 40; notch += 1) {
      const step = zoomStep(tiles, view, WIDTH, HEIGHT, point.x, point.y, WHEEL_ZOOM_FACTOR, true, hasContents);
      if (step.kind !== 'view') {
        expect(step.kind).toBe('enter');
        if (step.kind !== 'enter') return;
        expect(step.target).toBe(nested);
        // It enters from where the directory's contents are on screen right now.
        expect(step.from).toEqual(projectRect(view, tileBody(tiles[nested])));
        break;
      }
      view = step.view;
      scales.push(view.scale);
    }
    expect(scales.length).toBeGreaterThan(1);
    expect(scales).toEqual([...scales].sort((left, right) => left - right));
    expect(scales.at(-1)).toBeCloseTo(ceiling, 6);
    // The point under the pointer stayed on the same directory all along.
    const under = unprojectPoint(view, point.x, point.y);
    expect(hitTile(tiles, under.x, under.y)).toBeGreaterThanOrEqual(nested);
  });

  it('goes up a level when zooming out at full size, and only from the wheel', () => {
    const point = centre(tiles[indexOf('solo')].rect);
    expect(zoomStep(tiles, IDENTITY_VIEW, WIDTH, HEIGHT, point.x, point.y, 1 / WHEEL_ZOOM_FACTOR, true, hasContents)).toEqual({ kind: 'ascend' });
    expect(zoomStep(tiles, IDENTITY_VIEW, WIDTH, HEIGHT, point.x, point.y, 1 / WHEEL_ZOOM_FACTOR, false, hasContents)).toEqual({ kind: 'none' });
    // Magnified, it zooms back out a notch first (here over `solo`, a file: no ceiling below the limit).
    const magnified = { scale: 2, originX: 300, originY: 100 };
    const out = zoomStep(tiles, magnified, WIDTH, HEIGHT, 600, 300, 1 / WHEEL_ZOOM_FACTOR, true, hasContents);
    expect(out.kind === 'view' && out.view.scale).toBeCloseTo(2 / WHEEL_ZOOM_FACTOR, 6);
  });

  it('magnifies a file with no directory above it up to the limit, without going anywhere', () => {
    const point = centre(tiles[indexOf('solo')].rect);
    let view = IDENTITY_VIEW;
    for (let notch = 0; notch < 30; notch += 1) {
      const step = zoomStep(tiles, view, WIDTH, HEIGHT, point.x, point.y, WHEEL_ZOOM_FACTOR, true, hasContents);
      expect(step.kind).toBe('view');
      if (step.kind === 'view') view = step.view;
    }
    expect(view.scale).toBe(MAX_VIEW_SCALE);
  });

  it('magnifies by keyboard without going in', () => {
    const big = centre(tileBody(tiles[indexOf('big')]));
    let view = IDENTITY_VIEW;
    for (let press = 0; press < 12; press += 1) {
      const step = zoomStep(tiles, view, WIDTH, HEIGHT, big.x, big.y, 1.25, false, hasContents);
      expect(step.kind).toBe('view');
      if (step.kind === 'view') view = step.view;
    }
    expect(view.scale).toBe(MAX_VIEW_SCALE);
  });
});
