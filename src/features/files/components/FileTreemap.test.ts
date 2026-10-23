// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, h, nextTick, type App } from 'vue';
import type { FsSizeTreeEntry, FsSizeTreeView } from '../../../lib/ipc/fs/sizeTree';
import type { FsNode } from '../../../lib/ipc/fs/types';
import FileTreemap from './FileTreemap.vue';

function node(id: string, name: string, kind: FsNode['kind'], parent: string, deleted = false): FsNode {
  return {
    id, name, kind, parent_id: parent, size: 0, deleted, inode: null,
    timestamps: { modified: null, accessed: null, created: null, mft_changed: null, birth: null },
  };
}

function entry(value: FsNode, bytes: number, children: FsSizeTreeEntry[] | null = null, extensions: FsSizeTreeEntry['extensions'] = []): FsSizeTreeEntry {
  return {
    node: value, bytes, files: children ? children.length : 1, dirs: 0, newest: 1_766_217_600,
    children, others: null, extensions,
  };
}

const users = node('ntfs:part:2:40', 'Users', 'dir', 'part:2');
const photo = node('ntfs:part:2:41', 'photo.jpg', 'file', users.id);
const clip = node('ntfs:part:2:42', 'gone.jpg', 'file', users.id, true);
const pagefile = node('ntfs:part:2:30', 'pagefile.sys', 'file', 'part:2');

const view: FsSizeTreeView = {
  root: {
    node: null, bytes: 1_000, files: 3, dirs: 1, newest: 1_766_217_600, others: null, extensions: [],
    children: [
      entry(users, 700, [entry(photo, 500), entry(clip, 200)], [{ extension: 'jpg', bytes: 700 }]),
      entry(pagefile, 300),
    ],
  },
  truncated: false,
  newestWrite: 1_766_217_600,
  builtAt: '2026-01-01T00:00:00Z',
};

const handlers = {
  onSelect: vi.fn(), onOpen: vi.fn(), onEnter: vi.fn(), onUp: vi.fn(), onDepth: vi.fn(),
  onContextmenu: vi.fn(), onShowList: vi.fn(), onRetry: vi.fn(),
};

let app: App | undefined;
let host: HTMLDivElement;
const sizes = new Map<string, PropertyDescriptor | undefined>();

beforeEach(() => {
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
  app = undefined;
  host.remove();
  for (const [key, descriptor] of sizes) {
    if (descriptor) Object.defineProperty(HTMLElement.prototype, key, descriptor);
    else Reflect.deleteProperty(HTMLElement.prototype, key);
  }
  vi.clearAllMocks();
});

async function mount(props: Partial<Record<string, unknown>> = {}) {
  app = createApp({
    render: () => h(FileTreemap, {
      view, status: 'ready', error: null, mode: 'bytes', depth: 3,
      directoryName: 'C', directoryPath: '/C', volumeRoot: true, systemArea: false,
      selectedId: null, snapshotLabel: null, ...handlers, ...props,
    }),
  });
  app.mount(host);
  for (let index = 0; index < 4; index++) await nextTick();
}

function tile(name: string): HTMLElement {
  const found = [...host.querySelectorAll<HTMLElement>('.tile')].find(element => element.querySelector('.tile-name')?.textContent === name);
  if (!found) throw new Error(`no tile named ${name}`);
  return found;
}

describe('FileTreemap', () => {
  it('draws every entry with its kind, and deleted entries hatched', async () => {
    await mount();
    expect([...host.querySelectorAll('.tile-name')].map(element => element.textContent)).toEqual(['Users', 'photo.jpg', 'gone.jpg', 'pagefile.sys']);
    expect(tile('Users').classList).toContain('k-image');
    expect(tile('Users').classList).toContain('open');
    expect(tile('pagefile.sys').classList).toContain('k-system');
    expect(tile('gone.jpg').classList).toContain('deleted');
  });

  it('selects a tile with its full path and opens or enters on double-click', async () => {
    await mount();
    tile('photo.jpg').dispatchEvent(new MouseEvent('click', { bubbles: true }));
    expect(handlers.onSelect).toHaveBeenCalledWith(expect.objectContaining({ id: photo.id, path: '/C/Users/photo.jpg' }));

    tile('photo.jpg').dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
    expect(handlers.onOpen).toHaveBeenCalledWith(expect.objectContaining({ id: photo.id }));

    tile('Users').dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
    expect(handlers.onEnter).toHaveBeenCalledWith([{ id: users.id, name: 'Users' }]);
  });

  it('goes into the directory under the wheel and out again', async () => {
    await mount({ volumeRoot: false });
    tile('photo.jpg').dispatchEvent(new WheelEvent('wheel', { bubbles: true, cancelable: true, deltaY: -120 }));
    expect(handlers.onEnter).toHaveBeenCalledWith([{ id: users.id, name: 'Users' }]);
    // A level change waits for its view; a fresh surface takes the next gesture.
    app!.unmount();
    await mount({ volumeRoot: false });
    host.querySelector('.treemap-surface')!.dispatchEvent(new WheelEvent('wheel', { bubbles: true, cancelable: true, deltaY: 120 }));
    expect(handlers.onUp).toHaveBeenCalledTimes(1);
  });

  it('walks tiles with the keyboard', async () => {
    await mount();
    const surface = host.querySelector<HTMLElement>('.treemap-surface')!;
    surface.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
    expect(handlers.onSelect).toHaveBeenLastCalledWith(expect.objectContaining({ id: users.id }));
    surface.dispatchEvent(new KeyboardEvent('keydown', { key: ']', bubbles: true }));
    expect(handlers.onDepth).toHaveBeenCalledWith(1);
    surface.dispatchEvent(new KeyboardEvent('keydown', { key: 'Backspace', bubbles: true }));
    expect(handlers.onUp).toHaveBeenCalledTimes(1);
  });

  it('explains why there is nothing to draw', async () => {
    await mount({ view: null, status: 'unsupported' });
    expect(host.textContent).toContain('树图仅支持 NTFS 卷');
    host.querySelector<HTMLButtonElement>('.ui-empty-actions button')!.click();
    expect(handlers.onShowList).toHaveBeenCalledTimes(1);

    app!.unmount();
    await mount({ view: null, status: 'unindexed', snapshotLabel: '快照构建中 · 12 条' });
    expect(host.textContent).toContain('卷索引尚未覆盖此目录');
    expect(host.textContent).toContain('快照构建中 · 12 条');
  });
});
