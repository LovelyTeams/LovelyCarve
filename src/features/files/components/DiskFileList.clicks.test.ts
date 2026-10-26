// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, defineComponent, h, nextTick, ref, type App } from 'vue';
import DiskFileList from './DiskFileList.vue';

const ipc = vi.hoisted(() => ({ invoke: vi.fn(), copy: vi.fn() }));
vi.mock('../../../lib/ipc/client', () => ({ invokeCmd: ipc.invoke }));
vi.mock('../../../lib/ipc/events', () => ({ onEvent: async () => () => {}, emitEvent: vi.fn() }));
vi.mock('../../../lib/ipc/windows', () => ({ openFilePreviewWindow: vi.fn(), focusFilePreviewWindow: vi.fn() }));
vi.mock('../../../composables/clipboard', () => ({ copyText: ipc.copy }));
vi.mock('../composables/containerReads', () => ({ containerFsChildrenPage: (args: unknown) => ipc.invoke('fs_children_page', args), containerFsChildren: (args: unknown) => ipc.invoke('fs_children', args), containerFsFlatPage: (args: unknown) => ipc.invoke('fs_flat_page', args) }));
vi.mock('../../cases/composables/useCaseSession', () => ({ useCaseSession: () => ({ activeCase: ref(null), snapshots: ref([]), buildProgress: ref(new Map()) }) }));
vi.mock('../../preview/composables/usePreviewSync', () => ({ broadcastPreview: vi.fn(), onPreviewBroadcast: async () => () => {} }));
vi.mock('./DiskFileTree.vue', () => ({ default: { render: () => null } }));
vi.mock('../../tasks/components/PluginTaskDialog.vue', () => ({ default: { render: () => null } }));
vi.mock('../../preview/components/DiskFileDetail.vue', () => ({ default: defineComponent({
  props: ['node'],
  setup(props, { expose }) {
    expose({ getReadingState: () => undefined });
    return () => h('div', { 'data-preview': props.node.id });
  },
}) }));

type Entry = { id: string; name: string; kind: 'file' | 'dir'; size: number; parent?: string };
const node = (entry: Entry) => ({
  id: entry.id, name: entry.name, kind: entry.kind, size: entry.size, parent_id: entry.parent ?? 'root', deleted: false, inode: null,
  timestamps: { modified: null, accessed: null, created: null, mft_changed: null, birth: null },
});
const folder = node({ id: 'folder', name: 'Folder', kind: 'dir', size: 0 });
const alpha = node({ id: 'alpha', name: 'alpha.txt', kind: 'file', size: 10 });
const beta = node({ id: 'beta', name: 'beta.txt', kind: 'file', size: 20 });
const gamma = node({ id: 'gamma', name: 'gamma.txt', kind: 'file', size: 30 });
const inner = node({ id: 'inner', name: 'inner.txt', kind: 'file', size: 5, parent: 'folder' });

let app: App;
let host: HTMLDivElement;
async function settle() { for (let index = 0; index < 12; index++) await nextTick(); }
beforeEach(() => {
  vi.resetAllMocks();
  ipc.copy.mockResolvedValue(true);
  ipc.invoke.mockImplementation(async (command, args) => {
    if (command === 'fs_children_page') {
      const items = args.parentId === 'folder' ? [inner] : [folder, alpha, beta, gamma];
      return { items, nextCursor: null, total: items.length };
    }
    if (command === 'fs_mounts') return [];
    return {};
  });
  host = document.createElement('div'); document.body.append(host);
});
afterEach(() => { app?.unmount(); host.remove(); document.body.replaceChildren(); });
async function mount() {
  app = createApp({ render: () => h(DiskFileList, { imageId: 'disk-a' }) });
  app.mount(host); await settle();
}

const row = (name: string) => [...host.querySelectorAll<HTMLTableRowElement>('.disk-file-row')].find(item => item.textContent?.includes(name))!;
const checked = () => [...host.querySelectorAll<HTMLTableRowElement>('.disk-file-row')]
  .filter(item => item.querySelector<HTMLInputElement>('.cell-check input')?.checked)
  .map(item => item.querySelector('.name-text')?.textContent);
const preview = () => host.querySelector('[data-preview]')?.getAttribute('data-preview') ?? null;
/** Pages of the folder's listing; a row's child count asks for one entry. */
const folderLoads = () => ipc.invoke.mock.calls
  .filter(([command, args]) => command === 'fs_children_page' && args.parentId === 'folder' && args.pageSize > 1).length;
async function click(target: Element, init: MouseEventInit = {}) {
  target.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, detail: 1, ...init }));
  await settle();
}
async function rightClick(target: Element) {
  target.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 120, clientY: 90 }));
  await settle();
}
const menu = (label: string) => document.querySelector<HTMLElement>(`[role=menu][aria-label="${label}"]`);
const menuItem = (label: string) => [...document.querySelectorAll<HTMLElement>('[role^=menuitem]')]
  .find(item => item.querySelector('.ui-context-menu-text')?.textContent === label)!;

describe('file list clicks', () => {
  it('selects with a click, checks with Ctrl and Shift, and opens with a double click', async () => {
    await mount();
    await click(row('Folder').querySelector('.name-text')!);
    // A directory name is no longer a link: the click only selects the row.
    expect(folderLoads()).toBe(0);
    expect(row('Folder').hasAttribute('data-selected')).toBe(true);
    expect(preview()).toBeNull();

    await click(row('alpha.txt'), { ctrlKey: true });
    expect(checked()).toEqual(['alpha.txt']);
    expect(preview()).toBeNull();
    await click(row('gamma.txt'), { shiftKey: true });
    expect(checked()).toEqual(['alpha.txt', 'beta.txt', 'gamma.txt']);
    await click(row('beta.txt'), { ctrlKey: true });
    expect(checked()).toEqual(['alpha.txt', 'gamma.txt']);

    await click(row('beta.txt'));
    expect(preview()).toBe('beta');
    expect(checked()).toEqual(['alpha.txt', 'gamma.txt']);

    row('Folder').dispatchEvent(new MouseEvent('dblclick', { bubbles: true, detail: 2 }));
    await settle();
    expect(folderLoads()).toBe(1);
    expect(row('inner.txt')).toBeTruthy();
  });

  it('acts on the right-clicked row, or on every checked row when it is checked', async () => {
    await mount();
    await click(row('alpha.txt'), { ctrlKey: true });
    await click(row('beta.txt'), { ctrlKey: true });

    await rightClick(row('gamma.txt'));
    expect(menu('文件操作')?.querySelector('.ui-context-menu-title')?.textContent).toBe('gamma.txt');
    menuItem('复制名称').click();
    await settle();
    expect(ipc.copy).toHaveBeenLastCalledWith('gamma.txt', expect.any(String));
    expect(checked()).toEqual(['alpha.txt', 'beta.txt']);

    await rightClick(row('beta.txt'));
    expect(menu('文件操作')?.querySelector('.ui-context-menu-title')?.textContent).toBe('已勾选 2 项');
    menuItem('复制名称（2）').click();
    await settle();
    expect(ipc.copy).toHaveBeenLastCalledWith('alpha.txt\nbeta.txt', expect.any(String));
  });

  it('opens the directory menu on the blank area and the column menu on the header', async () => {
    await mount();
    await rightClick(host.querySelector('.table-wrap')!);
    expect(menu('当前目录操作')).not.toBeNull();
    menuItem('全选').click();
    await settle();
    expect(checked()).toEqual(['Folder', 'alpha.txt', 'beta.txt', 'gamma.txt']);

    await rightClick(host.querySelector('th[data-column="size"]')!);
    expect(menu('列操作')?.querySelector('.ui-context-menu-title')?.textContent).toBe('大小');
    menuItem('隐藏此列').click();
    await settle();
    expect(host.querySelector('th[data-column="size"]')).toBeNull();
  });
});
