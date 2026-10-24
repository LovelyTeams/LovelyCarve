import { motionTiming, useUiMotion } from './useUiMotion';

/** Selection indicators drawn by CSS on the current item: the segmented-control
 *  thumb (`.ui-seg-item::before`) and the tab underline (`.ui-tab::after`). */
const INDICATORS = [
  { itemClass: 'ui-seg-item', pseudoElement: '::before' },
  { itemClass: 'ui-tab', pseudoElement: '::after' },
] as const;
const STATE_ATTRIBUTES = ['class', 'aria-pressed', 'aria-selected', 'aria-checked'] as const;
type StateAttribute = typeof STATE_ATTRIBUTES[number];

interface GroupChange {
  pseudoElement: string;
  gained: HTMLElement[];
  lost: HTMLElement[];
}

/** Mirrors the primitives' current-item selectors in ui.css. */
function isCurrent(read: (attribute: StateAttribute) => string | null): boolean {
  return (read('class') ?? '').split(/\s+/).includes('active')
    || read('aria-pressed') === 'true'
    || read('aria-selected') === 'true'
    || read('aria-checked') === 'true';
}

/** Slide the new indicator from the previous item's box to its own. */
function slide(from: HTMLElement, to: HTMLElement, pseudoElement: string): void {
  if (typeof to.animate !== 'function') return;
  const previous = from.getBoundingClientRect();
  const next = to.getBoundingClientRect();
  // Items on another row (a wrapped control) or not laid out jump instead.
  if (!previous.width || !next.width || Math.abs(previous.top - next.top) > 2) return;
  const left = previous.left - next.left;
  const right = next.right - previous.right;
  if (Math.abs(left) < 1 && Math.abs(right) < 1) return;
  to.animate(
    [{ left: `${left}px`, right: `${right}px` }, { left: '0px', right: '0px' }],
    { ...motionTiming(to, 'normal', 'snap'), pseudoElement },
  );
}

function handle(records: MutationRecord[]): void {
  if (!useUiMotion().motionEnabled.value) return;
  // The first record per attribute carries the value from before this batch.
  const before = new Map<HTMLElement, Map<StateAttribute, string | null>>();
  for (const record of records) {
    const item = record.target;
    if (!(item instanceof HTMLElement)) continue;
    if (!INDICATORS.some(({ itemClass }) => item.classList.contains(itemClass))) continue;
    const attribute = record.attributeName as StateAttribute;
    const values = before.get(item) ?? new Map<StateAttribute, string | null>();
    if (!values.has(attribute)) values.set(attribute, record.oldValue);
    before.set(item, values);
  }
  const groups = new Map<Element, GroupChange>();
  for (const [item, values] of before) {
    const group = item.parentElement;
    const indicator = INDICATORS.find(({ itemClass }) => item.classList.contains(itemClass));
    if (!group || !indicator) continue;
    const wasCurrent = isCurrent(attribute => values.has(attribute) ? values.get(attribute) ?? null : item.getAttribute(attribute));
    const current = isCurrent(attribute => item.getAttribute(attribute));
    if (wasCurrent === current) continue;
    const change = groups.get(group) ?? { pseudoElement: indicator.pseudoElement, gained: [], lost: [] };
    (current ? change.gained : change.lost).push(item);
    groups.set(group, change);
  }
  for (const { pseudoElement, gained, lost } of groups.values()) {
    // Only a plain hand-over between two siblings slides; multi-select toggles jump.
    if (gained.length === 1 && lost.length === 1) slide(lost[0], gained[0], pseudoElement);
  }
}

let uninstall: (() => void) | null = null;

/** When the current item of a segmented control or tab strip changes, the thumb
 *  or underline slides over from the previous item instead of jumping. One
 *  observer serves every control in the window, so components keep plain
 *  `ui-seg` / `ui-tabs` markup; with motion off the indicator simply moves. */
export function installIndicatorMotion(root: Node = document.documentElement): () => void {
  if (uninstall) return uninstall;
  if (typeof MutationObserver === 'undefined') return () => {};
  const observer = new MutationObserver(handle);
  observer.observe(root, { subtree: true, attributes: true, attributeOldValue: true, attributeFilter: [...STATE_ATTRIBUTES] });
  uninstall = () => {
    observer.disconnect();
    uninstall = null;
  };
  return uninstall;
}
