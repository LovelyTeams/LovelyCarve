// @vitest-environment happy-dom
import { createApp, h, nextTick, ref, type App } from 'vue';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import Tabbar from './Tabbar.vue';
import Titlebar from './Titlebar.vue';
import type { WorkspaceTab } from '../workspace/useTabManager';

vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ minimize: vi.fn(), toggleMaximize: vi.fn(), close: vi.fn() }) }));

const tabs = ref<WorkspaceTab[]>([]);
const activeTabId = ref<string | null>(null);
const isBusy = ref(false);
const onActivate = vi.fn();
const onClose = vi.fn();
const onNewTab = vi.fn();
const disconnect = vi.fn();
let resized: () => void;
let app: App | undefined;
let host: HTMLElement;

function evidence(id: string, pendingLoad = false): WorkspaceTab {
  return { id, filePath: `D:/证据/${id}.E01`, fileName: `${id}.E01`, mode: 'image', lastActivePanel: 'info', lastActiveSeq: 0, pendingLoad };
}
function mount() {
  app = createApp({ render: () => h(Tabbar, { tabs: tabs.value, activeTabId: activeTabId.value, isBusy: isBusy.value, onActivate, onClose, onNewTab }) });
  app.mount(host);
}
function activation(id: string) { return host.querySelector<HTMLButtonElement>(`[data-tab-id="${id}"] .tab-activate`)!; }
function closeButton(id: string) { return host.querySelector<HTMLButtonElement>(`[data-tab-id="${id}"] .tab-close`)!; }
async function settle() { await nextTick(); await nextTick(); }
function bounds(element: Element, left: number, right: number) {
  vi.spyOn(element, 'getBoundingClientRect').mockReturnValue({ left, right, width: right - left } as DOMRect);
}

beforeEach(() => {
  vi.clearAllMocks();
  tabs.value = [evidence('first'), evidence('last', true)];
  activeTabId.value = 'first';
  isBusy.value = false;
  vi.stubGlobal('ResizeObserver', class {
    constructor(callback: () => void) { resized = callback; }
    observe() {}
    disconnect = disconnect;
  });
  host = document.createElement('div');
  document.body.append(host);
});
afterEach(() => { app?.unmount(); host.remove(); vi.unstubAllGlobals(); });

describe('titlebar evidence navigation', () => {
  it('exposes the current evidence and separate named close and new actions', () => {
    mount();
    expect(activation('first').getAttribute('aria-current')).toBe('page');
    expect(activation('last').hasAttribute('aria-current')).toBe(false);
    expect(closeButton('first').getAttribute('aria-label')).toBe('关闭 first.E01');
    expect(host.querySelector('button button')).toBeNull();
    closeButton('first').click();
    expect(onClose.mock.calls).toEqual([['first']]);
    expect(onActivate).not.toHaveBeenCalled();
    activation('last').click();
    expect(onActivate.mock.calls).toEqual([['last']]);
    expect(activation('last').title).toContain('等待当前分析完成');
    host.querySelector<HTMLButtonElement>('[aria-label="新标签页"]')!.click();
    expect(onNewTab).toHaveBeenCalledOnce();
  });

  it('keeps the new action outside the scroll viewport', () => {
    mount();
    expect(host.querySelector('.tabbar-scroll [aria-label="新标签页"]')).toBeNull();
    expect(host.querySelector('.tabbar > [aria-label="新标签页"]')).not.toBeNull();
  });

  it('reveals evidence when switching in either direction and after resizing', async () => {
    mount();
    const region = host.querySelector<HTMLElement>('.tabbar-scroll')!;
    bounds(region, 0, 300);
    bounds(activation('last').parentElement!, 420, 600);
    activeTabId.value = 'last';
    await settle();
    expect(region.scrollLeft).toBe(304);
    bounds(activation('first').parentElement!, -304, -124);
    activeTabId.value = 'first';
    await settle();
    expect(region.scrollLeft).toBe(0);
    bounds(activation('first').parentElement!, 4, 184);
    bounds(region, 0, 160);
    resized();
    expect(region.scrollLeft).toBe(28);
    app.unmount();
    app = undefined;
    expect(disconnect).toHaveBeenCalledOnce();
  });

  it('reveals the active tab again when another tab is inserted before it', async () => {
    mount();
    const region = host.querySelector<HTMLElement>('.tabbar-scroll')!;
    bounds(region, 0, 300);
    bounds(activation('first').parentElement!, 320, 500);
    tabs.value.unshift(evidence('inserted'));
    await settle();
    expect(region.scrollLeft).toBe(204);
  });

  it('blocks switching and closing inactive evidence while busy, including middle click', () => {
    isBusy.value = true;
    mount();
    expect([activation('last').disabled, closeButton('last').disabled]).toEqual([true, true]);
    activation('last').click();
    closeButton('last').click();
    activation('last').dispatchEvent(new MouseEvent('auxclick', { button: 1, bubbles: true }));
    expect(onActivate).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    closeButton('first').dispatchEvent(new MouseEvent('auxclick', { button: 1, bubbles: true }));
    expect(onClose.mock.calls).toEqual([['first']]);
    expect(host.querySelector<HTMLButtonElement>('[aria-label="新标签页"]')!.disabled).toBe(false);
  });

  it('restores focus after an asynchronous close removes the focused tab', async () => {
    mount();
    closeButton('first').focus();
    closeButton('first').click();
    await settle();
    expect(document.activeElement).toBe(closeButton('first'));
    tabs.value = [evidence('last')];
    activeTabId.value = 'last';
    await settle();
    expect(document.activeElement).toBe(activation('last'));
  });

  it('focuses the new action after the last focused tab is removed', async () => {
    tabs.value = [evidence('first')];
    mount();
    closeButton('first').focus();
    tabs.value = [];
    activeTabId.value = null;
    await settle();
    expect(document.activeElement).toBe(host.querySelector('[aria-label="新标签页"]'));
  });

  it('uses a stable Chinese case entry and puts the name in its tooltip', () => {
    app = createApp(Titlebar, { caseName: 'Case', onOpenCase: onClose });
    app.mount(host);
    const entry = host.querySelector<HTMLButtonElement>('[aria-label="案件管理"]')!;
    expect(entry.textContent?.trim()).toBe('案件管理');
    expect(entry.title).toBe('案件管理 · Case');
    entry.click();
    expect(onClose).toHaveBeenCalledOnce();
    const settings = host.querySelector<HTMLButtonElement>('[aria-label="设置中心"]')!;
    expect(settings.querySelector('svg')).not.toBeNull();
    expect(settings.querySelector('.titlebar-chip-label')?.textContent).toBe('设置中心');
    expect(settings.classList.contains('ui-btn')).toBe(true);
  });
});
