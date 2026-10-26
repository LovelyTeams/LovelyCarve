/** Where a context menu or one of its submenus goes, in viewport pixels. */
export interface MenuPlacement {
  top: number;
  left: number;
  /** Height the menu may take before it scrolls. */
  maxHeight: number;
  /** The menu opens to the left of its anchor because the right has no room. */
  leftward: boolean;
  /** The menu opens upward because there is no room below. */
  upward: boolean;
}

interface Point {
  x: number;
  y: number;
}

interface Size {
  width: number;
  height: number;
}

interface Box {
  top: number;
  left: number;
  right: number;
}

const VIEWPORT_MARGIN = 8;
/** A submenu overlaps its parent's padding so the pointer crosses no gap. */
const SUBMENU_OVERLAP = 2;
/** A submenu frame starts this much above its trigger, so their first rows line up. */
const SUBMENU_LIFT = 5;

function clamp(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(value, max));
}

/** Opens down and to the right of the pointer, flips to the other side on an
 *  axis where it does not fit, and always stays inside the viewport. */
export function placeContextMenu(point: Point, menu: Size, viewport: Size): MenuPlacement {
  const maxHeight = Math.max(0, viewport.height - VIEWPORT_MARGIN * 2);
  const height = Math.min(menu.height, maxHeight);
  const leftward = point.x + menu.width > viewport.width - VIEWPORT_MARGIN && point.x - menu.width >= VIEWPORT_MARGIN;
  const upward = point.y + height > viewport.height - VIEWPORT_MARGIN && point.y - height >= VIEWPORT_MARGIN;
  return {
    left: leftward ? point.x - menu.width : clamp(point.x, VIEWPORT_MARGIN, viewport.width - VIEWPORT_MARGIN - menu.width),
    top: upward ? point.y - height : clamp(point.y, VIEWPORT_MARGIN, viewport.height - VIEWPORT_MARGIN - height),
    maxHeight,
    leftward,
    upward,
  };
}

/** Opens beside its trigger row: on the right unless only the left has room,
 *  level with the row and shifted up where it would cross the bottom edge. */
export function placeSubmenu(trigger: Box, menu: Size, viewport: Size): MenuPlacement {
  const maxHeight = Math.max(0, viewport.height - VIEWPORT_MARGIN * 2);
  const height = Math.min(menu.height, maxHeight);
  const rightStart = trigger.right - SUBMENU_OVERLAP;
  const leftEnd = trigger.left + SUBMENU_OVERLAP;
  const roomRight = viewport.width - VIEWPORT_MARGIN - rightStart;
  const roomLeft = leftEnd - VIEWPORT_MARGIN;
  const leftward = menu.width > roomRight && roomLeft > roomRight;
  const wantedTop = trigger.top - SUBMENU_LIFT;
  const top = clamp(wantedTop, VIEWPORT_MARGIN, viewport.height - VIEWPORT_MARGIN - height);
  return {
    left: leftward
      ? Math.max(VIEWPORT_MARGIN, leftEnd - menu.width)
      : clamp(rightStart, VIEWPORT_MARGIN, viewport.width - VIEWPORT_MARGIN - menu.width),
    top,
    maxHeight,
    leftward,
    upward: top < wantedTop,
  };
}

/** Drops separators at either end or next to another, and section labels left
 *  without items, once conditional entries have been filtered out. */
export function compactMenuEntries<T extends { kind?: string }>(entries: readonly T[]): T[] {
  const out: T[] = [];
  const trailing = () => out[out.length - 1]?.kind;
  for (const entry of entries) {
    if (entry.kind === 'separator') {
      while (trailing() === 'label') out.pop();
      if (out.length === 0 || trailing() === 'separator') continue;
    }
    out.push(entry);
  }
  while (trailing() === 'separator' || trailing() === 'label') out.pop();
  return out;
}
