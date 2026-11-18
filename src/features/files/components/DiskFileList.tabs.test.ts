// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, defineComponent, h, nextTick, ref, type App } from 'vue';
import DiskFileList from './DiskFileList.vue';

const ipc = vi.hoisted(() => ({ invoke: vi.fn(), copy: vi.fn(), open: vi.fn(), focus: vi.fn() }));
vi.mock('../../../lib/ipc/client', () => ({ invokeCmd: ipc.invoke }));
vi.mock('../../../lib/ipc/events', () => ({ onEvent: async () => () => {}, emitEvent: vi.fn() }));
vi.mock('../../../lib/ipc/windows', () => ({ openFilePreviewWindow: ipc.open, focusFilePreviewWindow: ipc.focus }));
vi.mock('../../../composables/clipboard', () => ({ copyText: ipc.copy }));
vi.mock('../composables/containerReads', () => ({ containerFsChildrenPage: (args: unknown) => ipc.invoke('fs_children_page', args), containerFsChildren: (args: unknown) => ipc.invoke('fs_children', args), containerFsFlatPage: (args: unknown) => ipc.invoke('fs_flat_page', args) }));
vi.mock('../../cases/composables/useCaseSession', () => ({ useCaseSession: () => ({ activeCase: ref(null), snapshots: ref([]), buildProgress: ref(new Map()) }) }));
vi.mock('../../preview/composables/usePreviewSync', () => ({ broadcastPreview: vi.fn(), onPreviewBroadcast: async () => () => {} }));
vi.mock('./DiskFileTree.vue', () => ({ default: { render: () => null } }));
vi.mock('../../tasks/components/PluginTaskDialog.vue', () => ({ default: { render: () => null } }));
const readingStates = vi.hoisted(() => new Map<string, unknown>());
vi.mock('../../preview/components/DiskFileDetail.vue', () => ({ default: defineComponent({
  props: ['node', 'readingState', 'fullPath'],
  setup(props, { expose }) {
    expose({ getReadingState: () => readingStates.get(props.node.id) });
    return () => h('div', { 'data-preview': props.node.id, 'data-path': props.fullPath, 'data-restored': props.readingState?.scrollTop });
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
let now = 0;
const imageId = ref('disk-a');
async function settle() { for (let index = 0; index < 12; index++) await nextTick(); }
beforeEach(() => {
  vi.resetAllMocks();
  readingStates.clear();
  imageId.value = 'disk-a';
  // Activating a directory ignores clicks for 300 ms; tests step past that window.
  now = 0;
  vi.spyOn(performance, 'now').mockImplementation(() => now);
  ipc.copy.mockResolvedValue(true);
  ipc.invoke.mockImplementation(async (command, args) => {
    if (command === 'fs_children_page') {
      const items = args.parentId === 'folder' ? [inner] : [folder, alpha, beta, gamma];
      return { items, nextCursor: null, total: items.length };
    }
    if (command === 'fs_node_locate') return { frames: [], file_id: args.nodeId };
    if (command === 'fs_mounts') return [];
    return {};
  });
  host = document.createElement('div'); document.body.append(host);
});
afterEach(() => { app?.unmount(); host.remove(); document.body.replaceChildren(); vi.restoreAllMocks(); });
async function mount() {
  app = createApp({ render: () => h(DiskFileList, { imageId: imageId.value }) });
  app.mount(host); await settle();
}

const row = (name: string) => [...host.querySelectorAll<HTMLTableRowElement>('.disk-file-row')].find(item => item.textContent?.includes(name))!;
async function click(target: Element, init: MouseEventInit = {}) {
  now += 1000;
  target.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, detail: 1, ...init }));
  await settle();
}
async function doubleClick(target: Element) {
  now += 1000;
  target.dispatchEvent(new MouseEvent('dblclick', { bubbles: true, detail: 2 }));
  await settle();
}
/** Tab labels in strip order; the preview slot is marked with `*`, the active tab with `>`. */
const tabs = () => [...host.querySelectorAll<HTMLElement>('.preview-tab')].map(tab =>
  `${tab.classList.contains('active') ? '>' : ''}${tab.querySelector('.preview-tab-label')?.textContent}${tab.classList.contains('transient') ? '*' : ''}`);
const shown = () => host.querySelector('.preview-instance.active [data-preview]')?.getAttribute('data-preview') ?? null;
const tab = (name: string) => [...host.querySelectorAll<HTMLElement>('.preview-tab')].find(item => item.textContent?.includes(name))!;
const folderLoads = () => ipc.invoke.mock.calls.filter(([command, args]) => command === 'fs_children_page' && args.parentId === 'folder' && args.pageSize > 1).length;

describe('file preview tabs', () => {
  it('keeps opened files while the list moves and switches between them without going back', async () => {
    await mount();
    await click(row('alpha.txt'));
    expect(tabs()).toEqual(['>alpha.txt*']);
    await click(row('beta.txt'));
    expect(tabs()).toEqual(['>beta.txt*']);

    await doubleClick(row('beta.txt'));
    await click(row('gamma.txt'));
    expect(tabs()).toEqual(['beta.txt', '>gamma.txt*']);

    await doubleClick(row('Folder'));
    expect(folderLoads()).toBe(1);
    expect(tabs()).toEqual(['beta.txt', '>gamma.txt*']);
    await click(row('inner.txt'));
    expect(tabs()).toEqual(['beta.txt', '>inner.txt*']);
    expect(host.querySelector('.preview-instance.active [data-path]')?.getAttribute('data-path')).toBe('/Folder/inner.txt');

    await click(tab('beta.txt').querySelector('.preview-tab-activate')!);
    expect(shown()).toBe('beta');
    expect(host.querySelector('.preview-instance.active [data-path]')?.getAttribute('data-path')).toBe('/beta.txt');
    expect(folderLoads()).toBe(1);
    expect(host.querySelector('.breadcrumb')?.textContent).toContain('Folder');

    // Double-clicking the preview slot's tab keeps its file open.
    await click(tab('inner.txt').querySelector('.preview-tab-activate')!);
    await doubleClick(tab('inner.txt').querySelector('.preview-tab-activate')!);
    expect(tabs()).toEqual(['beta.txt', '>inner.txt']);
    await click(tab('beta.txt').querySelector('.preview-tab-close')!);
    expect(tabs()).toEqual(['>inner.txt']);
    expect(shown()).toBe('inner');
  });

  it('lets a directory in the preview slot go when the list enters it', async () => {
    await mount();
    await doubleClick(row('alpha.txt'));
    await click(row('Folder'));
    expect(tabs()).toEqual(['alpha.txt', '>Folder*']);
    await doubleClick(row('Folder'));
    expect(tabs()).toEqual(['>alpha.txt']);
    expect(shown()).toBe('alpha');
  });

  it('remembers the open files of each evidence and their reading position', async () => {
    await mount();
    await doubleClick(row('alpha.txt'));
    readingStates.set('alpha', { imageId: 'disk-a', nodeId: 'alpha', tab: 'hex', searchQuery: '', encoding: '', page: 1, pageSize: 2048, currentMatch: -1, scrollTop: 96, scrollLeft: 0, codeScrollTop: 0, codeScrollLeft: 0 });
    imageId.value = 'disk-b';
    await settle();
    expect(tabs()).toEqual([]);
    expect(host.querySelector('.preview-instance')).toBeNull();
    imageId.value = 'disk-a';
    await settle();
    expect(tabs()).toEqual(['>alpha.txt']);
    expect(host.querySelector('.preview-instance.active [data-preview]')?.getAttribute('data-restored')).toBe('96');
  });

  it('opens every checked file in its own tab from the row menu', async () => {
    await mount();
    await click(row('alpha.txt'), { ctrlKey: true });
    await click(row('beta.txt'), { ctrlKey: true });
    await click(row('gamma.txt'), { ctrlKey: true });
    row('beta.txt').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 120, clientY: 90 }));
    await settle();
    const item = [...document.querySelectorAll<HTMLElement>('[role^=menuitem]')].find(element => element.textContent?.includes('预览勾选文件（3）'))!;
    item.click();
    await settle();
    expect(tabs()).toEqual(['>alpha.txt', 'beta.txt', 'gamma.txt']);
    expect(shown()).toBe('alpha');
  });

  it('keeps the tabs while the preview is hidden and closes the others from the tab menu', async () => {
    await mount();
    await doubleClick(row('alpha.txt'));
    await doubleClick(row('beta.txt'));
    host.querySelector<HTMLButtonElement>('[aria-label="关闭预览"]')!.click();
    await settle();
    expect(host.querySelector('.preview-dock')).toBeNull();
    host.querySelector<HTMLButtonElement>('[aria-label="右侧预览"]')!.click();
    await settle();
    expect(tabs()).toEqual(['alpha.txt', '>beta.txt']);

    tab('alpha.txt').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 40, clientY: 40 }));
    await settle();
    [...document.querySelectorAll<HTMLElement>('[role^=menuitem]')].find(element => element.textContent?.includes('关闭其他标签页'))!.click();
    await settle();
    expect(tabs()).toEqual(['>alpha.txt']);
  });
});
