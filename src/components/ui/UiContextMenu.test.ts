// @vitest-environment happy-dom
import { createApp, h, nextTick, reactive, type App } from 'vue';
import { afterEach, describe, expect, it } from 'vitest';
import UiContextMenu, { type UiContextMenuEntry } from './UiContextMenu.vue';

let app: App | null = null;
let outside: HTMLButtonElement;
const calls: string[] = [];
const pageKeys: string[] = [];
const state = reactive({ first: false, second: false });

const FIRST_ENTRIES: UiContextMenuEntry[] = [
  { id: 'open', label: 'Open', action: () => calls.push(document.activeElement === outside ? 'open:focus-returned' : 'open:focus-elsewhere') },
  { id: 'disabled', label: 'Disabled', disabled: true, action: () => calls.push('disabled') },
  { kind: 'separator' },
  {
    kind: 'submenu', id: 'copy', label: 'Copy', items: [
      { id: 'copy:name', label: 'Name', action: () => calls.push('copy:name') },
      { id: 'copy:path', label: 'Path', action: () => calls.push('copy:path') },
    ],
  },
  { id: 'toggle', label: 'Toggle', checked: true },
];
const SECOND_ENTRIES: UiContextMenuEntry[] = [{ id: 'other', label: 'Other' }];

function onPageKey(event: KeyboardEvent) {
  pageKeys.push(event.key);
}

function mount() {
  calls.length = 0;
  pageKeys.length = 0;
  state.first = false;
  state.second = false;
  outside = document.createElement('button');
  outside.textContent = 'outside';
  document.body.append(outside);
  const host = document.createElement('div');
  document.body.append(host);
  document.addEventListener('keydown', onPageKey);
  app = createApp({
    render: () => [
      h(UiContextMenu, {
        open: state.first,
        'onUpdate:open': (value: boolean) => { state.first = value; },
        onSelect: (id: string) => calls.push(`select:${id}`),
        x: 40,
        y: 50,
        entries: FIRST_ENTRIES,
        label: 'First',
        heading: 'report.pdf',
        subheading: '12 KB',
      }),
      h(UiContextMenu, {
        open: state.second,
        'onUpdate:open': (value: boolean) => { state.second = value; },
        x: 10,
        y: 10,
        entries: SECOND_ENTRIES,
        label: 'Second',
      }),
    ],
  });
  app.mount(host);
}

async function settle() {
  await nextTick();
  await nextTick();
  await nextTick();
}

async function openFirst() {
  outside.focus();
  state.first = true;
  await settle();
}

const menu = (label: string) => document.querySelector<HTMLElement>(`[role=menu][aria-label="${label}"]`);
const item = (label: string) => [...document.querySelectorAll<HTMLElement>('[role^=menuitem]')]
  .find(element => element.querySelector('.ui-context-menu-text')?.textContent === label)!;
const focusedLabel = () => (document.activeElement as HTMLElement | null)?.querySelector('.ui-context-menu-text')?.textContent;

async function press(key: string) {
  (document.activeElement ?? document.body).dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }));
  await settle();
}

afterEach(() => {
  app?.unmount();
  app = null;
  document.removeEventListener('keydown', onPageKey);
  document.body.replaceChildren();
});

describe('UiContextMenu', () => {
  it('opens at the point with the focus inside and names its target', async () => {
    mount();
    await openFirst();
    const root = menu('First')!;
    expect(root).not.toBeNull();
    expect(document.activeElement).toBe(root);
    expect(root.style.left).toBe('40px');
    expect(root.style.top).toBe('50px');
    const header = document.getElementById(root.getAttribute('aria-describedby')!);
    expect(header?.textContent).toContain('report.pdf');
    expect(header?.textContent).toContain('12 KB');
  });

  it('moves through the enabled items with the arrow keys, Home and End', async () => {
    mount();
    await openFirst();
    await press('ArrowDown');
    expect(focusedLabel()).toBe('Open');
    await press('ArrowDown');
    expect(focusedLabel()).toBe('Copy');
    await press('ArrowDown');
    expect(focusedLabel()).toBe('Toggle');
    await press('ArrowDown');
    expect(focusedLabel()).toBe('Open');
    await press('ArrowUp');
    expect(focusedLabel()).toBe('Toggle');
    await press('Home');
    expect(focusedLabel()).toBe('Open');
    await press('End');
    expect(focusedLabel()).toBe('Toggle');
    expect(pageKeys).toEqual([]);
  });

  it('closes and returns the focus before running the chosen action', async () => {
    mount();
    await openFirst();
    item('Open').click();
    await settle();
    expect(calls).toEqual(['open:focus-returned', 'select:open']);
    expect(menu('First')).toBeNull();
    expect(state.first).toBe(false);
    expect(document.activeElement).toBe(outside);
  });

  it('ignores disabled items', async () => {
    mount();
    await openFirst();
    item('Disabled').click();
    await settle();
    expect(calls).toEqual([]);
    expect(menu('First')).not.toBeNull();
  });

  it('closes on Escape without letting the key reach the page', async () => {
    mount();
    await openFirst();
    await press('ArrowDown');
    await press('Escape');
    expect(menu('First')).toBeNull();
    expect(pageKeys).toEqual([]);
    expect(document.activeElement).toBe(outside);
  });

  it('closes on a press outside but not on one inside', async () => {
    mount();
    await openFirst();
    menu('First')!.dispatchEvent(new Event('pointerdown', { bubbles: true }));
    await settle();
    expect(menu('First')).not.toBeNull();
    document.body.dispatchEvent(new Event('pointerdown', { bubbles: true }));
    await settle();
    expect(menu('First')).toBeNull();
    expect(state.first).toBe(false);
  });

  it('opens a submenu with ArrowRight, leaves it with ArrowLeft and runs its items', async () => {
    mount();
    await openFirst();
    await press('ArrowDown');
    await press('ArrowDown');
    expect(item('Copy').getAttribute('aria-expanded')).toBe('false');
    await press('ArrowRight');
    expect(menu('Copy')).not.toBeNull();
    expect(item('Copy').getAttribute('aria-expanded')).toBe('true');
    expect(focusedLabel()).toBe('Name');
    await press('ArrowDown');
    expect(focusedLabel()).toBe('Path');
    await press('ArrowLeft');
    expect(menu('Copy')).toBeNull();
    expect(focusedLabel()).toBe('Copy');
    await press('ArrowRight');
    item('Path').click();
    await settle();
    expect(calls).toEqual(['copy:path', 'select:copy:path']);
    expect(menu('First')).toBeNull();
  });

  it('closes only the submenu on Escape inside it', async () => {
    mount();
    await openFirst();
    await press('ArrowDown');
    await press('ArrowDown');
    await press('ArrowRight');
    await press('Escape');
    expect(menu('Copy')).toBeNull();
    expect(menu('First')).not.toBeNull();
    expect(focusedLabel()).toBe('Copy');
  });

  it('marks toggles as checkbox items', async () => {
    mount();
    await openFirst();
    expect(item('Toggle').getAttribute('role')).toBe('menuitemcheckbox');
    expect(item('Toggle').getAttribute('aria-checked')).toBe('true');
    expect(item('Open').getAttribute('role')).toBe('menuitem');
    expect(item('Open').hasAttribute('aria-checked')).toBe(false);
  });

  it('closes the open menu when another one opens', async () => {
    mount();
    await openFirst();
    state.second = true;
    await settle();
    expect(menu('Second')).not.toBeNull();
    expect(menu('First')).toBeNull();
    expect(state.first).toBe(false);
  });
});
