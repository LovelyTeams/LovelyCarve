// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, defineComponent, h, nextTick, ref, type App } from 'vue';
import type { FsSizeTreeEntry } from '../../../lib/ipc/fs/sizeTree';
import type { FileListNavigator } from '../model/navigation';
import { DEFAULT_TREEMAP_OPTIONS, layoutTreemap } from '../model/treemap';
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
  props: ['node'],
  setup(props, { expose }) {
    expose({ getReadingState: () => undefined });
    return () => h('div', { 'data-preview': props.node.id });
  },
}) }));

const timestamps = { modified: null, accessed: null, created: null, mft_changed: null, birth: null };
const users = { id: 'ntfs:part:2:40', name: 'Users', kind: 'dir', parent_id: 'part:2', size: 0, deleted: false, inode: 40, timestamps };
const mft = { id: 'ntfs:part:2:0', name: '$MFT', kind: 'file', parent_id: 'part:2', size: 300, deleted: false, inode: 0, timestamps };
const photo = { id: 'ntfs:part:2:41', name: 'photo.jpg', kind: 'file', parent_id: users.id, size: 700, deleted: false, inode: 41, timestamps };
const leaf = (node: typeof photo) => ({ node, bytes: node.size, files: 1, dirs: 0, newest: null, children: null, others: null });
const usersEntry = { node: users, bytes: 700, files: 1, dirs: 0, newest: null, others: null, children: [leaf(photo)] };
const usersRoot = { node: null, bytes: 1_000, files: 2, dirs: 1, newest: null, others: null, children: [usersEntry, leaf(mft)] } as unknown as FsSizeTreeEntry;
const views: Record<string, unknown> = {
  'part:2': { root: usersRoot },
  [users.id]: { root: usersEntry },
};

let app: App;
let host: HTMLDivElement;
const panel = ref<FileListNavigator | null>(null);
const sizes = new Map<string, PropertyDescriptor | undefined>();
async function settle() { for (let index = 0; index < 12; index++) await nextTick(); }

beforeEach(() => {
  vi.resetAllMocks();
  ipc.open.mockResolvedValue(undefined);
  ipc.focus.mockResolvedValue(true);
  ipc.invoke.mockImplementation(async (command, args) => {
    const children = (parent: string) => (parent === 'part:2' ? [users, mft] : parent === users.id ? [photo] : []);
    if (command === 'fs_node_locate') return { frames: [{ id: 'part:2', name: 'C' }], file_id: mft.id };
    if (command === 'fs_children_page') return { items: children(args.parentId), nextCursor: null, total: children(args.parentId).length };
    if (command === 'fs_children') return children(args.parentId);
    if (command === 'fs_size_tree') {
      const view = views[args.nodeId];
      return view ? { status: 'ready', view: { truncated: false, newestWrite: null, builtAt: '2026-01-01T00:00:00Z', ...(view as object) } } : { status: 'unsupported' };
    }
    if (command === 'fs_mounts') return [];
    return {};
  });
  for (const key of ['clientWidth', 'clientHeight'] as const) {
    sizes.set(key, Object.getOwnPropertyDescriptor(HTMLElement.prototype, key));
    Object.defineProperty(HTMLElement.prototype, key, {
      configurable: true,
      get(this: HTMLElement) {
        if (!this.classList.contains('treemap-surface')) return 0;
        return key === 'clientWidth' ? 800 : 500;
      },
    });
  }
  host = document.createElement('div');
  document.body.append(host);
});

afterEach(() => {
  app?.unmount();
  host.remove();
  for (const [key, descriptor] of sizes) {
    if (descriptor) Object.defineProperty(HTMLElement.prototype, key, descriptor);
    else Reflect.deleteProperty(HTMLElement.prototype, key);
  }
});

async function mount() {
  app = createApp({ render: () => h(DiskFileList, { ref: panel, imageId: 'disk-a' }) });
  app.mount(host);
  await settle();
}

const switchButton = (label: string) => [...host.querySelectorAll<HTMLButtonElement>('.view-switch button')].find(button => button.textContent?.includes(label))!;
const sizeTreeRequests = () => ipc.invoke.mock.calls.filter(([command]) => command === 'fs_size_tree').map(([, args]) => args.nodeId);
/** Pages of the listing itself; a row's child count asks for one entry. */
const listLoads = (parentId: string) => ipc.invoke.mock.calls
  .filter(([command, args]) => command === 'fs_children_page' && args.parentId === parentId && args.pageSize > 1).length;

/** Opens the treemap at the volume root and double-clicks the Users tile. */
async function enterUsers() {
  await panel.value!.openFileByNodeId(mft.id);
  await settle();
  switchButton('树图').click();
  await settle();
  // The treemap is painted on a canvas: double-click where the Users tile is drawn.
  const drawn = layoutTreemap(usersRoot, { x: 0, y: 0, w: 800, h: 500 }, {
    value: item => item.bytes, children: item => item.children,
    othersValue: () => 0, isDirectory: item => item.node?.kind === 'dir',
  }, { ...DEFAULT_TREEMAP_OPTIONS, maxDepth: 3 });
  const header = drawn.find(tile => tile.entry?.node?.id === users.id)!.header!;
  host.querySelector('.treemap-surface')!.dispatchEvent(new MouseEvent('dblclick', {
    bubbles: true, detail: 2, clientX: header.x + header.w / 2, clientY: header.y + header.h / 2,
  }));
  await settle();
}

describe('file list treemap view', () => {
  it('opens on NTFS directories and shares navigation with the list', async () => {
    await mount();
    expect(switchButton('树图').disabled).toBe(true);
    await panel.value!.openFileByNodeId(mft.id);
    await settle();
    expect(switchButton('树图').disabled).toBe(false);

    await enterUsers();
    expect(ipc.invoke).toHaveBeenCalledWith('fs_size_tree', expect.objectContaining({ imageId: 'disk-a', nodeId: 'part:2', depth: 3, metric: 'bytes', includeDeleted: false }));
    // The volume root needs no parent; Users' parent is the root, already at hand.
    expect(sizeTreeRequests()).toEqual(['part:2', users.id]);
    expect(host.querySelector('.crumb.current')?.textContent).toContain('Users');
    // The list is hidden: it waits for the zoom to play, but loads as soon as it is shown.
    expect(listLoads(users.id)).toBe(0);

    switchButton('列表').click();
    await settle();
    expect(listLoads(users.id)).toBe(1);
    expect(host.querySelector('.treemap-surface')).toBeNull();
    expect([...host.querySelectorAll('.disk-file-row')].map(row => row.textContent)).toEqual([expect.stringContaining('photo.jpg')]);

    // Back in the treemap, the same directory comes from the client cache.
    switchButton('树图').click();
    await settle();
    expect(sizeTreeRequests()).toEqual(['part:2', users.id]);
    expect(host.querySelector('.treemap-surface')).not.toBeNull();
  });

  it('keeps the hidden list in step after the zoom has played', async () => {
    await mount();
    await enterUsers();
    expect(listLoads(users.id)).toBe(0);
    await new Promise(resolve => setTimeout(resolve, 450));
    await settle();
    expect(listLoads(users.id)).toBe(1);
    // Showing the list later loads nothing more.
    switchButton('列表').click();
    await settle();
    expect(listLoads(users.id)).toBe(1);
    expect([...host.querySelectorAll('.disk-file-row')].map(row => row.textContent)).toEqual([expect.stringContaining('photo.jpg')]);
  });

  it('leaves the treemap for the list when a search starts', async () => {
    await mount();
    await panel.value!.openFileByNodeId(mft.id);
    await settle();
    switchButton('树图').click();
    await settle();
    const search = host.querySelector<HTMLInputElement>('.location-search input')!;
    search.value = 'photo';
    search.dispatchEvent(new Event('input', { bubbles: true }));
    await settle();
    expect(host.querySelector('.treemap-surface')).toBeNull();
    expect(switchButton('列表').getAttribute('aria-pressed')).toBe('true');
  });
});
