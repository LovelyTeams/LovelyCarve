// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createApp, defineComponent, h, KeepAlive, nextTick, ref, type App } from 'vue';
import { useFileRowMenu } from './useFileRowMenu';
import type { DiskFsNode } from '../model/model';

let app: App | undefined;
let host: HTMLElement;
afterEach(() => { app?.unmount(); app = undefined; host?.remove(); vi.restoreAllMocks(); });
async function setup() {
  const visible = ref(true), preview = ref(true), selectedKey = ref<string | null>(null);
  const filter = ref(false), exportMode = ref(false), checked = ref(new Set(['checked-file']));
  const closeFilter = vi.fn(() => { filter.value = false; });
  const cancelExportMode = vi.fn(() => { exportMode.value = false; });
  const clearChecked = vi.fn(() => { checked.value = new Set(); });
  const refreshMounts = vi.fn(async () => {});
  let menu!: ReturnType<typeof useFileRowMenu>;
  const Panel = defineComponent({ setup() {
    menu = useFileRowMenu({ selectedKey, previewVisible: preview, isChecked: id => checked.value.has(id),
      filterVisible: () => filter.value, closeFilter, exportModeOpen: () => exportMode.value, cancelExportMode,
      hasChecked: () => checked.value.size > 0, clearChecked, refreshMounts });
    return () => h('div');
  } });
  host = document.createElement('div'); document.body.append(host);
  app = createApp({ render: () => h(KeepAlive, null, { default: () => visible.value ? h(Panel) : null }) });
  app.mount(host); await nextTick();
  function key(key: string, target: HTMLElement = document.body, isComposing = false) {
    target.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, isComposing }));
  }
  const node = { id: 'file', rowKey: 'path:file', kind: 'file' } as DiskFsNode;
  const checkedNode = { id: 'checked-file', kind: 'file' } as DiskFsNode;
  return { menu, visible, preview, selectedKey, filter, exportMode, checked, closeFilter, cancelExportMode, clearChecked, refreshMounts, key, node, checkedNode };
}

const contextMenu = (x = 120, y = 80) => new MouseEvent('contextmenu', { clientX: x, clientY: y, cancelable: true });

describe('file list menus', () => {
  it('acts on the clicked row alone unless it is one of the checked rows', async () => {
    const state = await setup();
    const event = contextMenu();
    state.menu.onRowContextMenu(event, state.node);
    expect(event.defaultPrevented).toBe(true);
    expect(state.menu.rowMenu).toMatchObject({ open: true, x: 120, y: 80, target: { node: state.node, scope: 'row' } });
    expect(state.selectedKey.value).toBe('path:file');
    state.menu.onRowContextMenu(contextMenu(), state.checkedNode);
    expect(state.menu.rowMenu.target?.scope).toBe('checked');
    expect(state.selectedKey.value).toBe('checked-file');
  });

  it('closes the filter popover and keeps one menu open', async () => {
    const state = await setup();
    state.filter.value = true;
    state.menu.showListMenu({ x: 4, y: 4 });
    expect(state.closeFilter).toHaveBeenCalled();
    expect(state.menu.listMenu.open).toBe(true);
    state.menu.showHeaderMenu({ x: 4, y: 4 }, 'size');
    expect(state.menu.headerMenu.target).toBe('size');
    state.menu.hideMenus();
    expect(state.menu.anyMenuOpen()).toBe(false);
  });

  it('refreshes mounts before offering to mount a Docker root', async () => {
    const state = await setup();
    state.menu.onRowContextMenu(contextMenu(), { id: 'host-docker:', kind: 'dir' } as DiskFsNode);
    expect(state.refreshMounts).toHaveBeenCalledTimes(1);
  });

  it('closes exactly one Escape layer and respects IME/input ownership', async () => {
    const state = await setup();
    state.menu.onRowContextMenu(contextMenu(), state.node);
    state.filter.value = true; state.exportMode.value = true;
    state.key('Escape', document.body, true);
    expect(state.menu.rowMenu.open).toBe(true);
    state.key('Escape');
    expect(state.menu.rowMenu.open).toBe(false);
    expect(state.filter.value).toBe(true);
    state.key('Escape'); expect(state.filter.value).toBe(false);
    state.key('Escape'); expect(state.exportMode.value).toBe(false);
    const input = document.createElement('input'); host.append(input);
    state.key('Escape', input); expect(state.preview.value).toBe(true);
    state.key('Escape'); expect(state.preview.value).toBe(false);
    expect(state.checked.value.size).toBe(1);
    state.key('Escape'); expect(state.checked.value.size).toBe(0);
  });

  it('removes keyboard ownership when cached and restores it on activation', async () => {
    const state = await setup();
    state.menu.onRowContextMenu(contextMenu(), state.node);
    state.visible.value = false; await nextTick();
    expect(state.menu.rowMenu.open).toBe(false);
    state.key('Escape'); expect(state.preview.value).toBe(true);
    state.menu.onRowContextMenu(contextMenu(), state.node);
    expect(state.menu.rowMenu.open).toBe(false);
    state.visible.value = true; await nextTick();
    state.key('Escape'); expect(state.preview.value).toBe(false);
    app!.unmount(); app = undefined;
    state.preview.value = true;
    state.key('Escape'); expect(state.preview.value).toBe(true);
  });
});
