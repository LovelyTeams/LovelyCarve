// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, defineComponent, h, nextTick, type App } from 'vue';
import type { PreviewSelection } from '../lib/ipc/previewDock';
import type { FilePreviewWindowArgs } from '../lib/ipc/windows';

const mocks = vi.hoisted(() => ({
  selection: undefined as ((selection: PreviewSelection) => void) | undefined,
  cleanup: vi.fn(), request: vi.fn(), dock: vi.fn(), close: vi.fn(), title: vi.fn(), focus: vi.fn(), notify: vi.fn(),
  isAlwaysOnTop: vi.fn(), setAlwaysOnTop: vi.fn(),
}));
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({
  setTitle: mocks.title, close: mocks.close, isMaximized: async () => false, onResized: async () => () => {},
  isAlwaysOnTop: mocks.isAlwaysOnTop, setAlwaysOnTop: mocks.setAlwaysOnTop,
}) }));
vi.mock('../lib/ipc/previewDock', () => ({
  onPreviewSelection: vi.fn(async (_owner: string, _preview: string, handler: (selection: PreviewSelection) => void) => {
    mocks.selection = handler;
    return mocks.cleanup;
  }),
  requestPreviewSelection: mocks.request, requestPreviewDock: mocks.dock,
}));
vi.mock('../lib/ipc/windows', () => ({ focusMainWindow: mocks.focus }));
vi.mock('../composables/useTheme', () => ({ useTheme: () => ({ setTheme: vi.fn() }) }));
vi.mock('../composables/useToast', () => ({ notifyError: mocks.notify }));
vi.mock('../features/preview/composables/usePreviewSync', () => ({ broadcastPreview: vi.fn(), onPreviewBroadcast: vi.fn() }));
vi.mock('../components/feedback/ToastHost.vue', () => ({ default: { render: () => null } }));
vi.mock('../components/feedback/ConfirmDialogHost.vue', () => ({ default: { render: () => null } }));
vi.mock('../features/preview/components/DiskFileDetail.vue', () => ({ default: defineComponent({
  props: ['imageId', 'node', 'fullPath', 'focusTarget', 'ident', 'digest'],
  setup(props, { expose }) {
    expose({ getReadingState: () => undefined });
    return () => h('div', {
      'data-preview': props.node.id, 'data-image': props.imageId, 'data-path': props.fullPath,
      'data-offset': props.focusTarget?.offset, 'data-ident': props.ident?.label, 'data-digest': props.digest?.md5,
    });
  },
}) }));
import FilePreviewWindow from './FilePreviewWindow.vue';

let app: App;
let host: HTMLDivElement;
const original = { imageId: 'image-a', nodeId: 'first', node: { id: 'first', name: 'first.txt' }, fullPath: '/first.txt', title: 'first.txt' };
function selection(revision: number, args: FilePreviewWindowArgs = original): PreviewSelection {
  return { ownerId: 'owner', previewId: 'preview', revision, args };
}
async function settle() { for (let index = 0; index < 12; index++) await nextTick(); }
beforeEach(() => {
  vi.clearAllMocks();
  mocks.title.mockResolvedValue(undefined);
  mocks.dock.mockResolvedValue(undefined);
  mocks.focus.mockResolvedValue(undefined);
  mocks.close.mockResolvedValue(undefined);
  mocks.request.mockResolvedValue(undefined);
  mocks.isAlwaysOnTop.mockResolvedValue(false);
  mocks.setAlwaysOnTop.mockResolvedValue(undefined);
  const params = new URLSearchParams({ ownerId: 'owner', previewId: 'preview', imageId: original.imageId, node: JSON.stringify(original.node), fullPath: original.fullPath });
  window.history.replaceState({}, '', `/?${params}`);
  host = document.createElement('div'); document.body.append(host);
});
afterEach(() => { app?.unmount(); host.remove(); window.history.replaceState({}, '', '/'); });
async function mount() {
  app = createApp(FilePreviewWindow);
  app.mount(host);
  await settle();
}

describe('detached preview selection', () => {
  it('toggles the native always-on-top state and exposes the pressed state', async () => {
    await mount();
    const button = host.querySelector<HTMLButtonElement>('.pin-btn')!;
    expect(button.getAttribute('aria-pressed')).toBe('false');
    button.click();
    await settle();
    expect(mocks.setAlwaysOnTop).toHaveBeenLastCalledWith(true);
    expect(button.getAttribute('aria-pressed')).toBe('true');
    expect(button.title).toBe('取消置顶');
    button.click();
    await settle();
    expect(mocks.setAlwaysOnTop).toHaveBeenLastCalledWith(false);
    expect(button.getAttribute('aria-pressed')).toBe('false');
  });

  it('reads the native pin state and retains it when an update fails', async () => {
    mocks.isAlwaysOnTop.mockResolvedValueOnce(true);
    await mount();
    const button = host.querySelector<HTMLButtonElement>('.pin-btn')!;
    mocks.setAlwaysOnTop.mockRejectedValueOnce(new Error('native failure'));
    button.click();
    await settle();
    expect(button.getAttribute('aria-pressed')).toBe('true');
    expect(button.disabled).toBe(false);
    expect(mocks.notify).toHaveBeenCalledWith('窗口操作失败', 'Error: native failure');
  });

  it('disables pinning while a native operation is pending', async () => {
    await mount();
    let complete!: () => void;
    mocks.setAlwaysOnTop.mockImplementationOnce(() => new Promise<void>(resolve => { complete = resolve; }));
    const button = host.querySelector<HTMLButtonElement>('.pin-btn')!;
    button.click();
    await settle();
    expect(button.disabled).toBe(true);
    button.click();
    expect(mocks.setAlwaysOnTop).toHaveBeenCalledOnce();
    complete();
    await settle();
    expect(button.disabled).toBe(false);
    expect(button.getAttribute('aria-pressed')).toBe('true');
  });

  it('requests the latest file after subscribing and replaces all file data atomically', async () => {
    const latest = { ...original, imageId: 'image-b', nodeId: 'second', node: { id: 'second', name: 'second.txt' }, fullPath: '/other/second.txt', title: 'second.txt', focusTarget: { offset: 512, length: 8 }, ident: { label: 'Text' }, digest: { md5: 'digest' } };
    mocks.request.mockImplementationOnce(async () => mocks.selection?.(selection(3, latest)));
    await mount();
    expect(mocks.request).toHaveBeenCalledExactlyOnceWith('owner', 'preview');
    const preview = host.querySelector('[data-preview]')!;
    expect([...preview.attributes].filter(attribute => /^data-(preview|image|path|offset|ident|digest)$/.test(attribute.name)).map(attribute => [attribute.name, attribute.value])).toEqual([
      ['data-preview', 'second'], ['data-image', 'image-b'], ['data-path', '/other/second.txt'],
      ['data-offset', '512'], ['data-ident', 'Text'], ['data-digest', 'digest'],
    ]);
    expect(host.querySelector('.pw-title-text')?.textContent).toBe('second.txt');
    expect(mocks.title).toHaveBeenLastCalledWith('second.txt');
  });

  it('ignores old revisions and clears the previous file caches and byte target', async () => {
    await mount();
    mocks.selection?.(selection(5, { ...original, focusTarget: { offset: 128, length: 4 }, ident: { label: 'Text' }, digest: { md5: 'old' } }));
    mocks.selection?.(selection(6, { ...original, nodeId: 'latest', node: { id: 'latest', name: 'latest.txt' }, fullPath: '/latest.txt' }));
    mocks.selection?.(selection(5));
    await settle();
    const preview = host.querySelector('[data-preview]')!;
    expect(preview.getAttribute('data-preview')).toBe('latest');
    expect(['data-offset', 'data-ident', 'data-digest'].map(name => preview.getAttribute(name))).toEqual([null, null, null]);
  });

  it('closes only after docking is acknowledged and releases selection subscriptions', async () => {
    await mount();
    mocks.dock.mockRejectedValueOnce(new Error('owner unavailable'));
    host.querySelector<HTMLButtonElement>('.dock-btn')!.click();
    await settle();
    expect(mocks.close).not.toHaveBeenCalled();
    host.querySelector<HTMLButtonElement>('.dock-btn')!.click();
    await settle();
    expect(mocks.close).toHaveBeenCalledOnce();
    app.unmount();
    expect(mocks.cleanup).toHaveBeenCalledOnce();
  });
});
