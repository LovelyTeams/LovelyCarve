// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { installIndicatorMotion } from './indicatorMotion';
import { initUiMotion } from './useUiMotion';
import { useUiPreferences } from './useUiPreferences';

const { motionPreference } = useUiPreferences();
const originalAnimate = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'animate');
let host: HTMLDivElement;
let disposeMotion: () => void;
let uninstall: () => void;
let animate: ReturnType<typeof vi.fn>;

function box(left: number, width: number, top = 0): DOMRect {
  return { left, right: left + width, width, top, bottom: top + 24, height: 24, x: left, y: top, toJSON: () => ({}) } as DOMRect;
}

function control(groupClass: string, itemClass: string, boxes: DOMRect[]): HTMLElement[] {
  const group = document.createElement('div');
  group.className = groupClass;
  const items = boxes.map(rect => {
    const item = document.createElement('button');
    item.className = itemClass;
    vi.spyOn(item, 'getBoundingClientRect').mockReturnValue(rect);
    group.append(item);
    return item;
  });
  host.append(group);
  return items;
}

/** MutationObserver delivers its records after the current task's microtasks. */
const flush = () => new Promise(resolve => setTimeout(resolve, 0));

beforeEach(() => {
  motionPreference.value = 'system';
  vi.stubGlobal('matchMedia', () => Object.assign(new EventTarget(), { matches: false }));
  disposeMotion = initUiMotion();
  animate = vi.fn();
  Object.defineProperty(HTMLElement.prototype, 'animate', { configurable: true, value: animate });
  host = document.createElement('div');
  document.body.append(host);
  uninstall = installIndicatorMotion();
});

afterEach(() => {
  uninstall();
  host.remove();
  disposeMotion();
  motionPreference.value = 'system';
  if (originalAnimate) Object.defineProperty(HTMLElement.prototype, 'animate', originalAnimate);
  else Reflect.deleteProperty(HTMLElement.prototype, 'animate');
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('indicator motion', () => {
  it('slides the segmented thumb from the previous item to the new one', async () => {
    const [list, treemap] = control('ui-seg', 'ui-seg-item', [box(10, 50), box(62, 68)]);
    list.classList.add('active');
    await flush();
    expect(animate).not.toHaveBeenCalled();

    list.classList.remove('active');
    treemap.classList.add('active');
    await flush();

    expect(animate).toHaveBeenCalledOnce();
    expect(animate.mock.contexts[0]).toBe(treemap);
    expect(animate.mock.calls[0]).toEqual([
      [{ left: '-52px', right: '70px' }, { left: '0px', right: '0px' }],
      // Without theme tokens the timing falls back to the documented defaults.
      { duration: 250, easing: 'ease-out', pseudoElement: '::before' },
    ]);
  });

  it('slides the tab underline when aria-selected moves', async () => {
    const [details, hex] = control('ui-tabs', 'ui-tab', [box(0, 60), box(80, 72)]);
    details.setAttribute('aria-selected', 'true');
    hex.setAttribute('aria-selected', 'false');
    await flush();

    details.setAttribute('aria-selected', 'false');
    hex.setAttribute('aria-selected', 'true');
    await flush();

    expect(animate).toHaveBeenCalledOnce();
    expect(animate.mock.contexts[0]).toBe(hex);
    expect(animate.mock.calls[0][1]).toMatchObject({ pseudoElement: '::after' });
    expect(animate.mock.calls[0][0][0]).toEqual({ left: '-80px', right: '92px' });
  });

  it('moves without animation when motion is off, when several items change, or across rows', async () => {
    const [first, second, third] = control('ui-seg', 'ui-seg-item', [box(0, 40), box(42, 40), box(0, 40, 30)]);
    first.setAttribute('aria-pressed', 'true');
    await flush();

    motionPreference.value = 'off';
    first.setAttribute('aria-pressed', 'false');
    second.setAttribute('aria-pressed', 'true');
    await flush();
    expect(animate).not.toHaveBeenCalled();

    motionPreference.value = 'system';
    // Multi-select toggles: two items turn on at once.
    first.setAttribute('aria-pressed', 'true');
    third.setAttribute('aria-pressed', 'true');
    await flush();
    expect(animate).not.toHaveBeenCalled();

    // A wrapped control: only `second` is current, and the next item sits on another row.
    first.setAttribute('aria-pressed', 'false');
    third.setAttribute('aria-pressed', 'false');
    await flush();
    second.setAttribute('aria-pressed', 'false');
    third.setAttribute('aria-pressed', 'true');
    await flush();
    expect(animate).not.toHaveBeenCalled();
  });

  it('ignores class changes that do not change the current item', async () => {
    const [list, treemap] = control('ui-seg', 'ui-seg-item', [box(0, 40), box(42, 40)]);
    list.classList.add('active');
    await flush();
    treemap.classList.add('focus-ring');
    list.classList.add('hovered');
    await flush();
    expect(animate).not.toHaveBeenCalled();
  });
});
