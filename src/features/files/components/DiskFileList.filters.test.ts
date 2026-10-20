// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, defineComponent, h, nextTick, ref, type App } from 'vue';
import type { FileListNavigator } from '../model/navigation';
import DiskFileList from './DiskFileList.vue';

const ipc = vi.hoisted(() => ({ invoke: vi.fn(), save: vi.fn(), open: vi.fn() }));
vi.mock('../../../lib/ipc/client', () => ({ invokeCmd: ipc.invoke }));
vi.mock('../../../lib/ipc/events', () => ({ onEvent: async () => () => {}, emitEvent: vi.fn() }));
vi.mock('../../../lib/ipc/windows', () => ({ openFilePreviewWindow: vi.fn(), focusFilePreviewWindow: vi.fn() }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ save: ipc.save, open: ipc.open }));
vi.mock('../composables/containerReads', () => ({ containerFsChildrenPage: (args: unknown) => ipc.invoke('fs_children_page', args), containerFsChildren: (args: unknown) => ipc.invoke('fs_children', args), containerFsFlatPage: (args: unknown) => ipc.invoke('fs_flat_page', args) }));
vi.mock('../../cases/composables/useCaseSession', () => ({ useCaseSession: () => ({ activeCase: ref(null), snapshots: ref([]), buildProgress: ref(new Map()) }) }));
vi.mock('../../preview/composables/usePreviewSync', () => ({ broadcastPreview: vi.fn(), onPreviewBroadcast: async () => () => {} }));
vi.mock('./DiskFileTree.vue', () => ({ default: { render: () => null } }));
vi.mock('../../tasks/components/PluginTaskDialog.vue', () => ({ default: { render: () => null } }));
vi.mock('../../preview/components/DiskFileDetail.vue', () => ({ default: defineComponent({
  setup(_, { expose }) {
    expose({ getReadingState: () => undefined });
    return () => null;
  },
}) }));

type Entry = { id: string; name: string; kind: 'file' | 'dir'; size: number };
const node = (entry: Entry) => ({ ...entry, parent_id: 'root', deleted: false, inode: null, timestamps: { modified: null, accessed: null, created: null, mft_changed: null, birth: null } });
const photo = node({ id: 'photo', name: 'photo.jpg', kind: 'file', size: 2048 });
const report = node({ id: 'report', name: 'report.pdf', kind: 'file', size: 4096 });

let app: App;
let host: HTMLDivElement;
const panel = ref<FileListNavigator | null>(null);
async function settle() { for (let index = 0; index < 12; index++) await nextTick(); }
function listing(args: { query?: { filter?: { nameQuery?: string; extensions?: string[] } } }) {
  const filter = args.query?.filter;
  return [photo, report].filter(item => (!filter?.nameQuery || item.name.includes(filter.nameQuery))
    && (!filter?.extensions?.length || filter.extensions.some(extension => item.name.endsWith(`.${extension}`))));
}
beforeEach(() => {
  vi.resetAllMocks();
  ipc.invoke.mockImplementation(async (command, args) => {
    if (command === 'fs_children_page') {
      const items = listing(args);
      return { items, nextCursor: null, total: items.length };
    }
    if (command === 'fs_flat_page') return { items: [{ node: photo, path: '/sub/photo.jpg' }], total: 1, truncated: false };
    if (command === 'fs_flat_ext_counts') return { counts: { jpg: 1 }, total: 1 };
    if (command === 'fs_mounts') return [];
    return {};
  });
  host = document.createElement('div'); document.body.append(host);
});
afterEach(() => { app?.unmount(); host.remove(); vi.useRealTimers(); });
async function mount() {
  app = createApp({ render: () => h(DiskFileList, { ref: panel, imageId: 'disk-a' }) });
  app.mount(host); await settle();
}
const button = (text: string) => [...host.querySelectorAll<HTMLButtonElement>('button')].find(element => element.textContent?.trim() === text)!;
const requests = (command: string) => ipc.invoke.mock.calls.filter(([name]) => name === command).map(([, args]) => args);

describe('file list filter bar', () => {
  it('shows a server-filtered empty page as filtered, with a removable condition chip', async () => {
    vi.useFakeTimers();
    await mount();
    host.querySelector<HTMLButtonElement>('.head-funnel[title="过滤「名称」"]')!.click();
    await settle();
    const input = document.querySelector<HTMLInputElement>('.disk-filter-popover input[type="text"]')!;
    input.value = 'missing'; input.dispatchEvent(new Event('input', { bubbles: true }));
    await vi.advanceTimersByTimeAsync(250);
    await settle();
    expect(host.querySelectorAll('.disk-file-row')).toHaveLength(0);
    expect(host.querySelector('.table-state')?.textContent).toContain('没有符合条件的条目');
    expect(host.querySelector('.table-state')?.textContent).not.toContain('空目录');
    expect(host.querySelector('.filter-chip')?.textContent).toContain('名称');
    expect(host.querySelector('.filter-chip')?.textContent).toContain('missing');
    expect(host.querySelector('.list-footer')?.textContent).toContain('符合条件');
    host.querySelector<HTMLButtonElement>('[aria-label="移除条件「名称」"]')!.click();
    await vi.advanceTimersByTimeAsync(250);
    await settle();
    expect(host.querySelector('.filter-chip')).toBeNull();
    expect(host.querySelectorAll('.disk-file-row')).toHaveLength(2);
    expect(host.querySelector('.list-footer')?.textContent).toContain('共');
  });

  it('lists all subfolders and returns to the current folder, clearing the keyword', async () => {
    vi.useFakeTimers();
    await mount();
    button('所有子目录').click();
    await settle();
    expect(requests('fs_flat_page')).toContainEqual(expect.objectContaining({ deepsearch: true, parentId: 'root' }));
    expect(host.querySelector('.name-parent')?.textContent).toBe('sub/');
    const search = host.querySelector<HTMLInputElement>('[aria-label="搜索文件名或路径"]')!;
    search.value = 'photo'; search.dispatchEvent(new Event('input', { bubbles: true }));
    await vi.advanceTimersByTimeAsync(600);
    await settle();
    expect(requests('fs_flat_page').at(-1)).toMatchObject({ keyword: 'photo' });
    const directoryLoads = requests('fs_children_page').length;
    button('当前目录').click();
    await vi.advanceTimersByTimeAsync(600);
    await settle();
    expect(search.value).toBe('');
    expect(requests('fs_children_page').length).toBeGreaterThan(directoryLoads);
    expect(button('当前目录').getAttribute('aria-pressed')).toBe('true');
  });

  it('reports a failed listing in place and retries it', async () => {
    const original = ipc.invoke.getMockImplementation()!;
    ipc.invoke.mockImplementation(async (command, args) => command === 'fs_children_page' ? Promise.reject(new Error('I/O error at 0x1f3a0000')) : original(command, args));
    await mount();
    expect(host.querySelector('[role="alert"]')?.textContent).toContain('目录读取失败');
    expect(host.querySelector('[role="alert"]')?.textContent).toContain('I/O error at 0x1f3a0000');
    ipc.invoke.mockImplementation(original);
    button('重试').click();
    await settle();
    expect(host.querySelectorAll('.disk-file-row')).toHaveLength(2);
  });

  it('exports the cursor row from the filter bar when nothing is checked', async () => {
    ipc.save.mockResolvedValue(null);
    await mount();
    expect(button('导出').disabled).toBe(true);
    [...host.querySelectorAll<HTMLTableRowElement>('.disk-file-row')].find(row => row.textContent?.includes(report.name))!.click();
    await settle();
    expect(button('导出').title).toBe('导出「report.pdf」');
    button('导出').click();
    await settle();
    expect(ipc.save).toHaveBeenCalledWith(expect.objectContaining({ defaultPath: 'report.pdf' }));
  });

  it('keeps the folder type tally after the backend filters by a type', async () => {
    await mount();
    host.querySelector<HTMLButtonElement>('.category-trigger')!.click();
    await settle();
    const items = () => [...document.querySelectorAll<HTMLElement>('.category-menu .category-item')]
      .map(item => `${item.querySelector('.item-label')?.textContent} ${item.querySelector('.item-count')?.textContent}`);
    expect(items()).toEqual(['全部类型 2', '图片 1', 'PDF 1']);
    [...document.querySelectorAll<HTMLButtonElement>('.category-menu .category-item')].find(item => item.textContent?.includes('图片'))!.click();
    await new Promise(resolve => setTimeout(resolve, 250));
    await settle();
    expect(requests('fs_children_page').at(-1)).toMatchObject({ query: { filter: { extensions: expect.arrayContaining(['jpg']) } } });
    expect(host.querySelectorAll('.disk-file-row')).toHaveLength(1);
    expect(items()).toEqual(['全部类型 2', '图片 1', 'PDF 1']);
    expect(host.querySelector('.category-trigger')?.textContent).toContain('图片');
  });
});
