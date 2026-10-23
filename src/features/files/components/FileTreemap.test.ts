// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, h, nextTick, type App } from 'vue';
import type { FsSizeTreeEntry, FsSizeTreeView } from '../../../lib/ipc/fs/sizeTree';
import type { FsNode } from '../../../lib/ipc/fs/types';
import { DEFAULT_TREEMAP_OPTIONS, layoutTreemap, type Rect } from '../model/treemap';
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
const gone = node('ntfs:part:2:42', 'gone.jpg', 'file', users.id, true);
const pagefile = node('ntfs:part:2:30', 'pagefile.sys', 'file', 'part:2');

const view: FsSizeTreeView = {
  root: {
    node: null, bytes: 1_000, files: 3, dirs: 1, newest: 1_766_217_600, others: null, extensions: [],
    children: [
      entry(users, 700, [entry(photo, 500), entry(gone, 200)], [{ extension: 'jpg', bytes: 700 }]),
      entry(pagefile, 300),
    ],
  },
  truncated: false,
  newestWrite: 1_766_217_600,
  builtAt: '2026-01-01T00:00:00Z',
};

// The surface is 800×500; the same layout the component draws.
const tiles = layoutTreemap(view.root, { x: 0, y: 0, w: 800, h: 500 }, {
  value: item => item.bytes,
  children: item => item.children,
  othersValue: item => item.others?.bytes ?? 0,
  isDirectory: item => item.node?.kind === 'dir',
}, { ...DEFAULT_TREEMAP_OPTIONS, maxDepth: 3 });
const tileOf = (name: string) => tiles.find(tile => tile.entry?.node?.name === name)!;
const centre = (rect: Rect) => ({ clientX: rect.x + rect.w / 2, clientY: rect.y + rect.h / 2 });

const handlers = {
  onSelect: vi.fn(), onOpen: vi.fn(), onEnter: vi.fn(), onUp: vi.fn(), onDepth: vi.fn(),
  onContextmenu: vi.fn(), onShowList: vi.fn(), onRetry: vi.fn(),
};

/** A 2D context that records what is painted. */
function recordingContext() {
  return {
    texts: [] as string[],
    fills: 0,
    fillStyle: '' as unknown, strokeStyle: '', lineWidth: 1, font: '', textBaseline: '', textAlign: '',
    setTransform() {}, clearRect() {}, save() {}, restore() {}, beginPath() {}, rect() {}, clip() {}, strokeRect() {},
    moveTo() {}, lineTo() {}, stroke() {},
    fillRect() { this.fills += 1; },
    fillText(text: string) { this.texts.push(text); },
    measureText(text: string) { return { width: text.length * 7 }; },
    createPattern() { return null; },
  };
}

let app: App | undefined;
let host: HTMLDivElement;
let context: ReturnType<typeof recordingContext>;
let frames: FrameRequestCallback[] = [];
const sizes = new Map<string, PropertyDescriptor | undefined>();

function flushFrames() {
  const pending = frames;
  frames = [];
  for (const callback of pending) callback(performance.now());
}

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
  context = recordingContext();
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(() => context as unknown as CanvasRenderingContext2D);
  vi.spyOn(window, 'requestAnimationFrame').mockImplementation(callback => frames.push(callback));
  host = document.createElement('div');
  document.body.append(host);
});

afterEach(() => {
  app?.unmount();
  app = undefined;
  host.remove();
  frames = [];
  for (const [key, descriptor] of sizes) {
    if (descriptor) Object.defineProperty(HTMLElement.prototype, key, descriptor);
    else Reflect.deleteProperty(HTMLElement.prototype, key);
  }
  vi.restoreAllMocks();
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
  flushFrames();
}

const surface = () => host.querySelector<HTMLElement>('.treemap-surface')!;
const mouse = (type: string, point: { clientX: number; clientY: number }, detail = 1) =>
  surface().dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, detail, ...point }));
/** happy-dom's WheelEvent is a UIEvent; a browser's carries MouseEvent's pointer fields too. */
function wheel(point: { clientX: number; clientY: number }, deltaY: number, shiftKey = false) {
  const event = new WheelEvent('wheel', { bubbles: true, cancelable: true, deltaY });
  Object.defineProperties(event, {
    clientX: { value: point.clientX },
    clientY: { value: point.clientY },
    shiftKey: { value: shiftKey },
  });
  return surface().dispatchEvent(event);
}
const key = (name: string) => surface().dispatchEvent(new KeyboardEvent('keydown', { key: name, bubbles: true }));

describe('FileTreemap', () => {
  it('paints every tile and labels the ones with room for a name', async () => {
    await mount();
    expect(context.fills).toBeGreaterThanOrEqual(tiles.length);
    expect(context.texts).toEqual(expect.arrayContaining(['Users', 'photo.jpg', 'gone.jpg', 'pagefile.sys']));
  });

  it('selects with a click, opens files and goes into directories on a double click', async () => {
    await mount();
    mouse('click', centre(tileOf('photo.jpg').rect));
    expect(handlers.onSelect).toHaveBeenCalledWith(expect.objectContaining({ id: photo.id, path: '/C/Users/photo.jpg' }));
    mouse('dblclick', centre(tileOf('photo.jpg').rect), 2);
    expect(handlers.onOpen).toHaveBeenCalledWith(expect.objectContaining({ id: photo.id }));
    mouse('dblclick', centre(tileOf('Users').header!), 2);
    expect(handlers.onEnter).toHaveBeenCalledWith([{ id: users.id, name: 'Users' }]);
  });

  it('goes into a selected directory on a second click', async () => {
    await mount({ selectedId: users.id });
    mouse('click', centre(tileOf('Users').header!));
    expect(handlers.onEnter).toHaveBeenCalledWith([{ id: users.id, name: 'Users' }]);
    expect(handlers.onSelect).not.toHaveBeenCalled();
  });

  it('magnifies with the wheel until the directory fills the view, and goes in on the next notch', async () => {
    await mount();
    const point = centre(tileOf('photo.jpg').rect);
    wheel(point, -120);
    expect(handlers.onEnter).not.toHaveBeenCalled();
    let notches = 1;
    while (!handlers.onEnter.mock.calls.length && notches < 20) {
      wheel(point, -120);
      notches += 1;
    }
    expect(notches).toBeGreaterThan(1);
    expect(handlers.onEnter).toHaveBeenCalledWith([{ id: users.id, name: 'Users' }]);
  });

  it('magnifies the canvas, so the same point hits what is now under it', async () => {
    await mount();
    // The gap between Users and pagefile.sys: nothing there at full size.
    mouse('click', { clientX: 560, clientY: 300 });
    expect(handlers.onSelect).not.toHaveBeenCalled();
    key('+');
    mouse('click', { clientX: 560, clientY: 300 });
    expect(handlers.onSelect).toHaveBeenLastCalledWith(expect.objectContaining({ id: gone.id }));
    key('0');
    handlers.onSelect.mockClear();
    mouse('click', { clientX: 560, clientY: 300 });
    expect(handlers.onSelect).not.toHaveBeenCalled();
  });

  it('goes up when zooming out at full size, but not out of the volume', async () => {
    await mount({ volumeRoot: false });
    wheel(centre(tileOf('pagefile.sys').rect), 120);
    expect(handlers.onUp).toHaveBeenCalledTimes(1);
    app!.unmount();
    await mount({ volumeRoot: true });
    wheel(centre(tileOf('pagefile.sys').rect), 120);
    expect(handlers.onUp).toHaveBeenCalledTimes(1);
  });

  it('walks tiles with the keyboard', async () => {
    await mount();
    key('ArrowRight');
    expect(handlers.onSelect).toHaveBeenLastCalledWith(expect.objectContaining({ id: users.id }));
    key(']');
    expect(handlers.onDepth).toHaveBeenCalledWith(1);
    key('Backspace');
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
