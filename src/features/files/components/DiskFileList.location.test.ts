// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, defineComponent, h, nextTick, ref, type App } from 'vue';
import type { FileListNavigator } from '../model/navigation';
import DiskFileList from './DiskFileList.vue';

const ipc = vi.hoisted(() => ({ invoke: vi.fn(), open: vi.fn(), focus: vi.fn(), emit: vi.fn() }));
vi.mock('../../../lib/ipc/client', () => ({ invokeCmd: ipc.invoke }));
vi.mock('../../../lib/ipc/events', () => ({ onEvent: async () => () => {}, emitEvent: ipc.emit }));
vi.mock('../../../lib/ipc/windows', () => ({ openFilePreviewWindow: ipc.open, focusFilePreviewWindow: ipc.focus }));
vi.mock('../composables/containerReads', () => ({ containerFsChildrenPage: (args: unknown) => ipc.invoke('fs_children_page', args), containerFsChildren: (args: unknown) => ipc.invoke('fs_children', args), containerFsFlatPage: (args: unknown) => ipc.invoke('fs_flat_page', args) }));
vi.mock('../../cases/composables/useCaseSession', () => ({ useCaseSession: () => ({ activeCase: ref(null), snapshots: ref([]), buildProgress: ref(new Map()) }) }));
vi.mock('../../preview/composables/usePreviewSync', () => ({ broadcastPreview: vi.fn(), onPreviewBroadcast: async () => () => {} }));
vi.mock('./DiskFileTree.vue', () => ({ default: { render: () => null } }));
vi.mock('../../tasks/components/PluginTaskDialog.vue', () => ({ default: { render: () => null } }));
vi.mock('../../preview/components/DiskFileDetail.vue', () => ({ default: defineComponent({
  props: ['node', 'focusTarget'],
  setup(props, { expose }) {
    expose({ getReadingState: () => undefined });
    return () => h('div', { 'data-preview': props.node.id, 'data-offset': props.focusTarget?.offset, 'data-length': props.focusTarget?.length });
  },
}) }));

let app: App;
let host: HTMLDivElement;
const imageId = ref('disk-a');
const panel = ref<FileListNavigator | null>(null);
const node = { id: 'part:0:42', name: 'evidence.txt', kind: 'file', parent_id: 'part:0:dir', size: 8_000_000, deleted: false, inode: null, timestamps: { modified: null, accessed: null, created: null, mft_changed: null, birth: null } };
async function settle() { for (let index = 0; index < 12; index++) await nextTick(); }
beforeEach(() => {
  vi.resetAllMocks();
  ipc.open.mockResolvedValue(undefined);
  ipc.focus.mockResolvedValue(true);
  imageId.value = 'disk-a';
  ipc.invoke.mockImplementation(async (command, args) => {
    if (command === 'fs_node_locate') return { frames: [{ id: 'part:0', name: '分区 0' }, { id: 'part:0:dir', name: '目录' }], file_id: node.id };
    if (command === 'fs_children_page') return { items: args.parentId === 'part:0:dir' ? [node] : [], nextCursor: null, total: args.parentId === 'part:0:dir' ? 1 : 0 };
    if (command === 'fs_children') return args.parentId === 'part:0:dir' ? [node] : [];
    if (command === 'fs_mounts') return [];
    return {};
  });
  host = document.createElement('div'); document.body.append(host);
});
afterEach(() => { app?.unmount(); host.remove(); vi.useRealTimers(); });
async function mount() {
  app = createApp({ render: () => h(DiskFileList, { ref: panel, imageId: imageId.value }) });
  app.mount(host); await settle();
}

describe('file list location integration', () => {
  it('opens the selected folder in preview and allows detaching without first selecting a file', async () => {
    const folder = { ...node, id: 'folder:646f6373', name: '文档', kind: 'dir', parent_id: 'folder:', size: 0 };
    const original = ipc.invoke.getMockImplementation()!;
    ipc.invoke.mockImplementation(async (command, args) => command === 'fs_children_page' && args.parentId === 'root'
      ? { items: [folder], nextCursor: null, total: 1 }
      : original(command, args));
    await mount();
    host.querySelector<HTMLTableRowElement>('.disk-file-row')!.click();
    await settle();
    expect(host.querySelector('.preview-dock')).toBeNull();
    host.querySelector<HTMLButtonElement>('[aria-label="右侧预览"]')!.click();
    await settle();
    expect(host.querySelector('[data-preview]')?.getAttribute('data-preview')).toBe(folder.id);
    const detach = host.querySelector<HTMLButtonElement>('[aria-label="拆分为独立窗口"]')!;
    expect(detach.disabled).toBe(false);
    detach.click();
    await settle();
    expect(ipc.open).toHaveBeenCalledWith(expect.objectContaining({ nodeId: folder.id, node: expect.objectContaining({ kind: 'dir', name: '文档' }) }), expect.any(Function));
    expect(host.querySelector('.preview-dock')).toBeNull();
  });

  it('updates an open or detached preview when selecting a directory row', async () => {
    const folder = { ...node, id: 'part:0:folder', name: '文档', kind: 'dir', size: 0 };
    const original = ipc.invoke.getMockImplementation()!;
    ipc.invoke.mockImplementation(async (command, args) => command === 'fs_children_page' && args.parentId === 'part:0:dir'
      ? { items: [node, folder], nextCursor: null, total: 2 }
      : original(command, args));
    await mount();
    await panel.value!.openFileByNodeId(node.id);
    await settle();
    const row = [...host.querySelectorAll<HTMLTableRowElement>('.disk-file-row')].find(element => element.textContent?.includes(folder.name))!;
    row.click();
    await settle();
    expect(host.querySelector('[data-preview]')?.getAttribute('data-preview')).toBe(folder.id);
    host.querySelector<HTMLButtonElement>('[aria-label="拆分为独立窗口"]')!.click();
    await settle();
    expect(ipc.open).toHaveBeenCalledWith(expect.objectContaining({ nodeId: folder.id, fullPath: '/分区 0/目录/文档' }), expect.any(Function));
    const fileRow = [...host.querySelectorAll<HTMLTableRowElement>('.disk-file-row')].find(element => element.textContent?.includes(node.name))!;
    fileRow.click();
    await settle();
    row.click();
    await settle();
    expect(ipc.emit.mock.calls.filter(([event]) => event === 'file-preview-selection').at(-1)?.[1].args.node.id).toBe(folder.id);
    expect(host.querySelector('.preview-dock')).toBeNull();
    expect(ipc.open).toHaveBeenCalledOnce();
  });

  it('keeps a teleported column filter focused and open for IME and nested select interaction', async () => {
    await mount();
    await panel.value!.openFileByNodeId(node.id);
    await settle();
    host.querySelector<HTMLButtonElement>('.head-funnel[title="过滤「大小」"]')!.click();
    await settle();
    const popover = document.querySelector<HTMLElement>('.disk-filter-popover')!;
    const input = popover.querySelector<HTMLInputElement>('input[type="text"]')!;
    expect(document.activeElement).toBe(input);
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, isComposing: true }));
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, isComposing: true }));
    await settle();
    expect(document.querySelector('.disk-filter-popover')).toBe(popover);
    popover.querySelector<HTMLButtonElement>('[aria-label="大小上限单位"]')!.click();
    await settle();
    const option = [...document.querySelectorAll<HTMLElement>('.ui-select-option')].find(element => element.textContent === 'KB')!;
    option.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
    option.click();
    await settle();
    expect(document.querySelector('.disk-filter-popover')).toBe(popover);
    expect(popover.querySelector('[aria-label="大小上限单位"]')?.getAttribute('aria-expanded')).toBe('false');
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    await settle();
    expect(document.querySelector('.disk-filter-popover')).toBeNull();
    expect(host.querySelector('[data-preview]')).not.toBeNull();
  });

  it('clears a hidden column filter while retaining column settings and the current directory', async () => {
    vi.useFakeTimers();
    const original = ipc.invoke.getMockImplementation()!;
    ipc.invoke.mockImplementation(async (command, args) => command === 'fs_children_page' && args.query?.filter?.nameQuery === 'missing'
      ? { items: [], nextCursor: null, total: 0 } : original(command, args));
    await mount();
    await panel.value!.openFileByNodeId(node.id);
    await settle();
    host.querySelector<HTMLButtonElement>('.head-funnel[title="过滤「名称」"]')!.click();
    await settle();
    const input = document.querySelector<HTMLInputElement>('.disk-filter-popover input[type="text"]')!;
    input.value = 'missing'; input.dispatchEvent(new Event('input', { bubbles: true }));
    await settle();
    await vi.advanceTimersByTimeAsync(250);
    await settle();
    expect(ipc.invoke.mock.calls.filter(([command]) => command === 'fs_children_page').map(([, args]) => args)).toContainEqual(expect.objectContaining({ query: expect.objectContaining({ filter: expect.objectContaining({ nameQuery: 'missing' }) }) }));
    expect(host.querySelectorAll('.disk-file-row')).toHaveLength(0);
    host.querySelector<HTMLButtonElement>('[aria-label="显示列"]')!.click();
    await settle();
    const setting = [...host.querySelectorAll<HTMLLabelElement>('.settings-item')].find(element => element.textContent?.includes('名称'))!;
    setting.querySelector<HTMLInputElement>('input')!.click();
    await settle();
    await vi.advanceTimersByTimeAsync(250);
    await settle();
    expect(document.querySelector('.disk-filter-popover')).toBeNull();
    expect(host.querySelectorAll('.disk-file-row')).toHaveLength(1);
    expect(host.querySelector('.breadcrumb')?.textContent).toContain('目录');
    expect(host.querySelector('.col-settings')).not.toBeNull();
  });

  it('keeps the inline dock hidden while the detached preview follows row clicks and result jumps', async () => {
    const second = { ...node, id: 'second', name: 'second.txt' };
    const original = ipc.invoke.getMockImplementation()!;
    ipc.invoke.mockImplementation(async (command, args) => command === 'fs_children_page' && args.parentId === 'part:0:dir'
      ? { items: [node, second], nextCursor: null, total: 2 }
      : original(command, args));
    await mount();
    await panel.value!.openFileByNodeId(node.id);
    await settle();
    host.querySelector<HTMLButtonElement>('[aria-label="拆分为独立窗口"]')!.click();
    await settle();
    expect(ipc.open).toHaveBeenCalledOnce();
    expect(host.querySelector('[data-preview]')).toBeNull();
    const row = [...host.querySelectorAll<HTMLTableRowElement>('.disk-file-row')].find(element => element.textContent?.includes(second.name))!;
    row.click();
    await settle();
    expect(host.querySelector('[data-preview]')).toBeNull();
    expect(ipc.emit.mock.calls.filter(([event]) => event === 'file-preview-selection').at(-1)?.[1].args.node.id).toBe(second.id);
    await panel.value!.openFileByNodeId(node.id, 256, 12);
    await settle();
    expect(host.querySelector('[data-preview]')).toBeNull();
    expect(ipc.emit.mock.calls.filter(([event]) => event === 'file-preview-selection').at(-1)?.[1].args.focusTarget).toMatchObject({ offset: 256, length: 12 });
    expect(ipc.open).toHaveBeenCalledOnce();
    ipc.open.mock.calls[0][1]();
    await settle();
    expect(host.querySelector('[data-preview]')).toBeNull();
    row.click();
    await settle();
    expect(host.querySelector('[data-preview]')).not.toBeNull();
  });

  it('retains the inline preview if native window creation fails', async () => {
    ipc.open.mockRejectedValueOnce(new Error('window unavailable'));
    await mount();
    await panel.value!.openFileByNodeId(node.id);
    await settle();
    host.querySelector<HTMLButtonElement>('[aria-label="拆分为独立窗口"]')!.click();
    await settle();
    expect(host.querySelector('[data-preview]')?.getAttribute('data-preview')).toBe(node.id);
  });
  it('uses a live plugin source chain and selects its file without a byte offset', async () => {
    const original = ipc.invoke.getMockImplementation()!;
    ipc.invoke.mockImplementation(async (command, args) => command === 'fs_source_locate'
      ? { frames: [{ id: 'part:0', name: '来源卷' }, { id: 'part:0:dir', name: '目录' }], file_id: node.id }
      : original(command, args));
    await mount();
    await panel.value!.openFileBySource('part:0', '目录/evidence.txt');
    await settle();
    expect(ipc.invoke).toHaveBeenCalledWith('fs_source_locate', { imageId: 'disk-a', rootId: 'part:0', path: '目录/evidence.txt' });
    expect(ipc.invoke.mock.calls.some(([command]) => command === 'fs_node_locate')).toBe(false);
    expect(host.querySelector('[data-selected="true"]')?.textContent).toContain(node.name);
    expect(host.querySelector('[data-preview]')?.getAttribute('data-offset')).toBeNull();
  });
  it('renders a large directory from one bounded page instead of fetching it all', async () => {
    const original = ipc.invoke.getMockImplementation()!;
    ipc.invoke.mockImplementation(async (command, args) => {
      if (command === 'fs_children_page') return {
        items: Array.from({ length: 500 }, (_, index) => ({ ...node, id: `row-${index}`, name: `file-${index}.txt` })),
        nextCursor: 'next-page', total: 10_000,
      };
      return original(command, args);
    });
    await mount();
    expect(ipc.invoke.mock.calls.filter(([command]) => command === 'fs_children_page')).toHaveLength(1);
    expect(host.querySelector('.list-footer')?.textContent).toContain('10,000');
    expect(host.querySelectorAll('.disk-file-row').length).toBeLessThan(100);
  });
  it('loads through the cursor to locate a file beyond the first directory page', async () => {
    const original = ipc.invoke.getMockImplementation()!;
    ipc.invoke.mockImplementation(async (command, args) => {
      if (command === 'fs_children_page' && args.parentId === 'part:0:dir') {
        return args.cursor
          ? { items: [node], nextCursor: null, total: 2 }
          : { items: [{ ...node, id: 'other', name: 'other.txt' }], nextCursor: 'next-page', total: 2 };
      }
      return original(command, args);
    });
    await mount();
    await panel.value!.openFileByNodeId(node.id);
    await settle();
    expect(host.querySelector('[data-preview]')?.getAttribute('data-preview')).toBe(node.id);
    expect(ipc.invoke).toHaveBeenCalledWith('fs_children_page', expect.objectContaining({ cursor: 'next-page' }));
  });
  it('clears a pending keyword filter, loads the parent, selects the row and passes byte focus to preview', async () => {
    await mount();
    const input = host.querySelector<HTMLInputElement>('[aria-label="搜索文件名或路径"]')!;
    input.value = 'unrelated'; input.dispatchEvent(new Event('input', { bubbles: true }));
    await nextTick();
    await panel.value!.openFileByNodeId(node.id, 4_000_000, 8);
    await settle();
    expect(input.value).toBe('');
    expect(host.querySelector('[data-selected="true"]')?.textContent).toContain(node.name);
    expect(host.querySelector('[data-preview]')?.getAttribute('data-offset')).toBe('4000000');
    expect(host.querySelector('[data-preview]')?.getAttribute('data-length')).toBe('8');
    expect(host.querySelector('.breadcrumb')?.textContent).toContain('目录');
  });

  it('reports a missing source instead of claiming success', async () => {
    await mount();
    ipc.invoke.mockImplementation(async command => command === 'fs_node_locate' ? { frames: [], file_id: node.id } : command === 'fs_children_page' ? { items: [], nextCursor: null, total: 0 } : []);
    await expect(panel.value!.openFileByNodeId(node.id)).rejects.toThrow('文件已不在原目录');
    expect(host.querySelector('[data-preview]')).toBeNull();
  });

  it('drops a late locate response after switching evidence', async () => {
    await mount();
    let finish!: (value: unknown) => void;
    ipc.invoke.mockImplementation(async command => {
      if (command === 'fs_node_locate') return new Promise(resolve => { finish = resolve; });
      return [];
    });
    const operation = panel.value!.openFileByNodeId(node.id);
    imageId.value = 'disk-b';
    await settle();
    finish({ frames: [{ id: 'part:0:dir', name: '旧目录' }], file_id: node.id });
    await operation;
    expect(host.querySelector('.breadcrumb')?.textContent).not.toContain('旧目录');
    expect(host.querySelector('[data-preview]')).toBeNull();
  });
});
