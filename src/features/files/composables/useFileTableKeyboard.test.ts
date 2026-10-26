// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { effectScope, ref } from 'vue';
import { useFileTableKeyboard } from './useFileTableKeyboard';
import type { DiskFsNode } from '../model/model';

function node(id: string, name = id, kind = 'file', rowKey = id): DiskFsNode {
  return { id, name, kind, rowKey, size: 0, deleted: false } as DiskFsNode;
}
const scopes: ReturnType<typeof effectScope>[] = [];
afterEach(() => { for (const scope of scopes.splice(0)) scope.stop(); vi.useRealTimers(); });
function setup(rows = [node('a'), node('b')]) {
  const scope = effectScope(); scopes.push(scope);
  const container = ref(document.createElement('div'));
  const selectedKey = ref<string | null>(null);
  const checkedIds = ref(new Set<string>());
  const menu = ref(false);
  const loading = ref(false);
  const activate = vi.fn(), preview = vi.fn(), goUp = vi.fn(), toggleAll = vi.fn(), setAnchor = vi.fn(), openMenu = vi.fn();
  const keyboard = scope.run(() => useFileTableKeyboard({ rows: () => rows, selectedKey, checkedIds, container,
    rowHeight: 36, menuOpen: () => menu.value, loading: () => loading.value, activate, preview, goUp, toggleAll, setAnchor, openMenu }))!;
  function key(key: string, init: KeyboardEventInit = {}, target: HTMLElement = container.value) {
    const event = new KeyboardEvent('keydown', { key, cancelable: true, ...init });
    Object.defineProperty(event, 'target', { value: target });
    keyboard.onTableKeydown(event);
    return event;
  }
  return { keyboard, key, selectedKey, checkedIds, menu, loading, activate, preview, goUp, toggleAll, setAnchor, openMenu, scope };
}

describe('file table keyboard ownership', () => {
  it('moves the cursor by row key and checks the shared hard-link node only once', () => {
    const state = setup([node('same', 'left', 'file', 'left'), node('same', 'right', 'file', 'right')]);
    state.key('ArrowDown'); state.key('ArrowDown');
    expect(state.selectedKey.value).toBe('right');
    state.key(' ');
    expect(state.checkedIds.value).toEqual(new Set(['same']));
    expect(state.setAnchor).toHaveBeenCalledWith('same');
    state.key('ArrowUp'); state.key(' ');
    expect(state.checkedIds.value).toEqual(new Set());
  });
  it('leaves IME, open-menu and nested control key events to their owners', () => {
    const state = setup();
    expect(state.key('ArrowDown', { isComposing: true }).defaultPrevented).toBe(false);
    const ime = state.key('ArrowDown');
    Object.defineProperty(ime, 'keyCode', { value: 229 });
    state.selectedKey.value = null; state.keyboard.onTableKeydown(ime);
    expect(state.selectedKey.value).toBeNull();
    state.menu.value = true;
    expect(state.key('ArrowDown').defaultPrevented).toBe(false);
    state.menu.value = false;
    expect(state.key(' ', {}, document.createElement('button')).defaultPrevented).toBe(false);
    expect(state.selectedKey.value).toBeNull();
  });
  it('opens file preview and navigates directories through separate callbacks', () => {
    const file = node('a'), directory = node('b', 'folder', 'dir');
    const state = setup([file, directory]);
    state.key('Home'); state.key('Enter');
    expect(state.preview).toHaveBeenCalledWith(file);
    state.key('End'); state.key('Enter');
    expect(state.activate).toHaveBeenCalledWith(directory);
    state.loading.value = true; state.key('Backspace');
    expect(state.goUp).not.toHaveBeenCalled();
    state.loading.value = false; state.key('ArrowLeft', { altKey: true });
    expect(state.goUp).toHaveBeenCalledOnce();
  });
  it('opens the cursor row menu from the keyboard, or the list menu without a cursor', () => {
    const file = node('a');
    const state = setup([file, node('b')]);
    expect(state.key('ContextMenu').defaultPrevented).toBe(true);
    expect(state.openMenu).toHaveBeenLastCalledWith(null);
    state.key('Home');
    state.key('F10', { shiftKey: true });
    expect(state.openMenu).toHaveBeenLastCalledWith(file);
    expect(state.preview).not.toHaveBeenCalled();
  });
  it('keeps spaces inside typeahead names and clears its timer on disposal', () => {
    vi.useFakeTimers();
    const state = setup([node('old', 'Old folder'), node('new', 'New Folder')]);
    state.key('n'); state.key('e'); state.key('w'); state.key(' ');
    expect(state.selectedKey.value).toBe('new');
    expect(state.checkedIds.value.size).toBe(0);
    expect(vi.getTimerCount()).toBe(1);
    state.scope.stop();
    expect(vi.getTimerCount()).toBe(0);
  });
});
