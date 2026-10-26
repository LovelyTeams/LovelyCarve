import { shallowReactive } from 'vue';

export interface ContextMenuState<T> {
  open: boolean;
  /** Viewport point the menu opens at. */
  x: number;
  y: number;
  /** What the menu acts on. Kept after it closes, so a chosen action still sees it. */
  target: T | null;
}

/** The ContextMenu key and Shift+F10 open a context menu from the keyboard. */
export function isContextMenuKey(event: KeyboardEvent): boolean {
  return event.key === 'ContextMenu' || (event.shiftKey && event.key === 'F10');
}

/** Where a keyboard-opened menu goes: just below the start of the row or tile it acts on. */
export function menuPointBelow(rect: { left: number; bottom: number; width: number }): { x: number; y: number } {
  return { x: rect.left + Math.min(rect.width / 2, 24), y: rect.bottom };
}

/** One surface's context menu: bind `menu.open` (`v-model:open`), `menu.x`
 *  and `menu.y` to `UiContextMenu`. */
export function useContextMenu<T>() {
  const menu = shallowReactive<ContextMenuState<T>>({ open: false, x: 0, y: 0, target: null });

  function openAt(x: number, y: number, target: T) {
    menu.x = x;
    menu.y = y;
    menu.target = target;
    menu.open = true;
  }

  /** Opens at the pointer of a `contextmenu` event instead of the native menu. */
  function openFromEvent(event: MouseEvent, target: T) {
    event.preventDefault();
    openAt(event.clientX, event.clientY, target);
  }

  /** Opens below an element: keyboard invocation on the row or tile in focus. */
  function openBelow(element: Element, target: T) {
    const point = menuPointBelow(element.getBoundingClientRect());
    openAt(point.x, point.y, target);
  }

  function close() {
    menu.open = false;
  }

  return { menu, openAt, openFromEvent, openBelow, close };
}
