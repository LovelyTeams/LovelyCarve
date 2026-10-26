import { describe, expect, it } from 'vitest';
import { compactMenuEntries, placeContextMenu, placeSubmenu } from './menuPlacement';

const viewport = { width: 1000, height: 800 };

describe('placeContextMenu', () => {
  it('opens down and to the right of the pointer', () => {
    expect(placeContextMenu({ x: 200, y: 100 }, { width: 220, height: 300 }, viewport)).toEqual({
      left: 200,
      top: 100,
      maxHeight: 784,
      leftward: false,
      upward: false,
    });
  });

  it('flips to the left and upward near the bottom-right corner', () => {
    expect(placeContextMenu({ x: 900, y: 700 }, { width: 220, height: 300 }, viewport)).toEqual({
      left: 680,
      top: 400,
      maxHeight: 784,
      leftward: true,
      upward: true,
    });
  });

  it('slides inside the viewport when neither side of the pointer fits', () => {
    const placement = placeContextMenu({ x: 150, y: 300 }, { width: 220, height: 600 }, { width: 300, height: 700 });
    expect(placement.left).toBe(72);
    expect(placement.top).toBe(92);
    expect(placement.leftward).toBe(false);
    expect(placement.upward).toBe(false);
  });

  it('scrolls a menu taller than the viewport', () => {
    const placement = placeContextMenu({ x: 10, y: 400 }, { width: 200, height: 2000 }, viewport);
    expect(placement.maxHeight).toBe(784);
    expect(placement.top).toBe(8);
  });
});

describe('placeSubmenu', () => {
  const trigger = { top: 200, left: 300, right: 500 };

  it('opens on the right, overlapping the parent padding, level with its trigger', () => {
    expect(placeSubmenu(trigger, { width: 160, height: 120 }, viewport)).toEqual({
      left: 498,
      top: 195,
      maxHeight: 784,
      leftward: false,
      upward: false,
    });
  });

  it('opens on the left when only the left has room', () => {
    const placement = placeSubmenu({ top: 200, left: 700, right: 900 }, { width: 160, height: 120 }, viewport);
    expect(placement.leftward).toBe(true);
    expect(placement.left).toBe(542);
  });

  it('moves up instead of crossing the bottom edge', () => {
    const placement = placeSubmenu({ top: 760, left: 300, right: 500 }, { width: 160, height: 120 }, viewport);
    expect(placement.top).toBe(672);
    expect(placement.upward).toBe(true);
  });
});

describe('compactMenuEntries', () => {
  const item = (id: string) => ({ kind: 'item', id });
  const separator = { kind: 'separator' };
  const label = (id: string) => ({ kind: 'label', id });

  it('drops separators at the ends and next to each other', () => {
    expect(compactMenuEntries([separator, item('a'), separator, separator, item('b'), separator])).toEqual([
      item('a'), separator, item('b'),
    ]);
  });

  it('drops a section label whose items were all filtered out', () => {
    expect(compactMenuEntries([item('a'), separator, label('copy'), separator, item('b'), separator, label('tail')])).toEqual([
      item('a'), separator, item('b'),
    ]);
  });

  it('keeps entries without a kind as items', () => {
    expect(compactMenuEntries([{ id: 'a' }, separator, { id: 'b' }])).toEqual([{ id: 'a' }, separator, { id: 'b' }]);
  });
});
