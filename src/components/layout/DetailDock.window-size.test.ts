// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, defineComponent, h, KeepAlive, nextTick, ref, type App } from 'vue';
import DetailDock from './DetailDock.vue';
import FilePreviewDock from '../../features/preview/components/FilePreviewDock.vue';
import UiTransition from '../ui/UiTransition.vue';
import { useUiPreferences } from '../../composables/useUiPreferences';

const nativeWindow = vi.hoisted(() => ({
  outerPosition: vi.fn(async () => ({ x: 100, y: 40 })),
  innerSize: vi.fn(async () => ({ width: 1440, height: 900 })),
  outerSize: vi.fn(async () => ({ width: 1440, height: 900 })),
  isMaximized: vi.fn(async () => false),
  isFullscreen: vi.fn(async () => false),
  setSize: vi.fn(async () => {}),
  setPosition: vi.fn(async () => {}),
}));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => nativeWindow,
  currentMonitor: async () => ({ workArea: { position: { x: 0, y: 0 }, size: { width: 2560, height: 1440 } } }),
}));

type TestAnimation = { onfinish: (() => void) | null; oncancel: (() => void) | null; cancel: () => void };
const animations: TestAnimation[] = [];
const originalAnimate = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'animate');
let app: App | undefined;
async function settle() {
  for (let index = 0; index < 20; index++) {
    await nextTick();
    for (const animation of animations.splice(0)) animation.onfinish?.();
  }
}
beforeEach(() => {
  vi.clearAllMocks();
  vi.stubGlobal('__TAURI_INTERNALS__', {});
  Object.defineProperty(HTMLElement.prototype, 'animate', { configurable: true, value: vi.fn(() => {
    const animation: TestAnimation = { onfinish: null, oncancel: null, cancel: () => {} };
    animations.push(animation);
    return animation as unknown as Animation;
  }) });
});
afterEach(async () => {
  app?.unmount();
  await settle();
  app = undefined;
  document.body.replaceChildren();
  if (originalAnimate) Object.defineProperty(HTMLElement.prototype, 'animate', originalAnimate);
  else Reflect.deleteProperty(HTMLElement.prototype, 'animate');
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('detail docks keep native window bounds', () => {
  it.each(['on', 'off'] as const)('keeps the window fixed across cached pages, detail toggles and width changes (motion %s)', async preference => {
    useUiPreferences().motionPreference.value = preference;
    const page = ref('search');
    const showDetail = ref(true);
    const showPreview = ref(true);
    const SearchPage = defineComponent({ setup: () => () => h(UiTransition, { kind: 'dock' }, {
      default: () => showDetail.value ? h(DetailDock, {
        label: '文件详情', title: 'selected.txt', closeLabel: '关闭文件详情',
        onClose: () => { showDetail.value = false; },
      }, { default: () => h('p', 'selected.txt') }) : null,
    }) });
    const FilesPage = defineComponent({ setup: () => () => h(UiTransition, { kind: 'dock' }, {
      default: () => showPreview.value ? h(FilePreviewDock, {
        name: 'preview.txt', opening: false, canDetach: true,
        onClose: () => { showPreview.value = false; },
      }) : null,
    }) });
    const TimelinePage = defineComponent({ render: () => h('p', '时间线') });
    const host = document.createElement('div');
    document.body.append(host);
    app = createApp({ render: () => h(KeepAlive, null, {
      default: () => h(page.value === 'search' ? SearchPage : page.value === 'files' ? FilesPage : TimelinePage),
    }) });
    app.mount(host);
    await settle();
    const originalDock = host.querySelector('.detail-dock');
    expect(originalDock).not.toBeNull();

    for (const next of ['timeline', 'files', 'timeline', 'search']) {
      page.value = next;
      await settle();
    }
    expect(host.querySelector('.detail-dock')).toBe(originalDock);
    expect(host.textContent).toContain('selected.txt');

    const divider = host.querySelector('[role="separator"]') as HTMLElement;
    const initialWidth = Number(divider.getAttribute('aria-valuenow'));
    divider.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowLeft', bubbles: true }));
    await settle();
    expect(Number(divider.getAttribute('aria-valuenow'))).toBe(initialWidth + 24);
    // Double-clicking the divider restores the default width.
    divider.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
    await settle();
    expect(Number(divider.getAttribute('aria-valuenow'))).toBe(380);

    (host.querySelector('[aria-label="关闭文件详情"]') as HTMLButtonElement).click();
    await settle();
    expect(host.querySelector('.detail-dock')).toBeNull();
    showDetail.value = true;
    await settle();
    expect(host.querySelector('.detail-dock')).not.toBeNull();
    expect((host.querySelector('.detail-dock') as HTMLElement).style.minWidth).toBe('');

    page.value = 'files';
    await settle();
    const previewDivider = host.querySelector('.preview-dock [role="separator"]') as HTMLElement;
    previewDivider.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
    await settle();
    previewDivider.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
    await settle();
    expect(Number(previewDivider.getAttribute('aria-valuenow'))).toBe(460);
    (host.querySelector('[aria-label="关闭预览"]') as HTMLButtonElement).click();
    await settle();
    expect(host.querySelector('.preview-dock')).toBeNull();
    showPreview.value = true;
    await settle();
    expect(host.querySelector('.preview-dock')).not.toBeNull();
    app.unmount();
    app = undefined;
    await settle();
    expect(nativeWindow.setSize).not.toHaveBeenCalled();
    expect(nativeWindow.setPosition).not.toHaveBeenCalled();
  });
});
