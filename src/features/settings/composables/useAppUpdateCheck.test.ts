// @vitest-environment happy-dom
import { createApp, h, nextTick, ref, type App } from 'vue';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useAppUpdateCheck, type AppUpdateCheck } from './useAppUpdateCheck';
import AppAboutPanel from '../components/AppAboutPanel.vue';
import { setLocale } from '../../../composables/useLocale';

vi.mock('../../../composables/useAppVersion', () => ({ useAppVersion: () => ({ version: ref('1.2.0'), buildDate: '' }) }));
vi.mock('../../../composables/useToast', () => ({ notifyInfo: vi.fn(), notifyError: vi.fn() }));

let app: App | undefined;
let host: HTMLElement;
let updates: AppUpdateCheck;
const fetchMock = vi.fn();
const preferenceKey = 'lovelycarve-auto-update-check';
async function settle() {
  for (let index = 0; index < 8; index++) await nextTick();
}
function mount() {
  app = createApp({ setup() { updates = useAppUpdateCheck(); return () => h(AppAboutPanel, { updates }); } });
  app.mount(host);
}
beforeEach(() => {
  localStorage.clear();
  setLocale('zh-CN');
  fetchMock.mockReset();
  vi.stubGlobal('fetch', fetchMock);
  host = document.createElement('div');
  document.body.append(host);
});
afterEach(() => {
  app?.unmount(); app = undefined;
  host.remove();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
  setLocale('zh-CN');
});

describe('app update checks', () => {
  it('makes no startup request by default, but allows a manual check and displays the latest tag', async () => {
    let resolveCheck: (response: Response) => void = () => {};
    fetchMock.mockImplementation(() => new Promise<Response>(resolve => { resolveCheck = resolve; }));
    mount(); await settle();
    expect(updates.automatic.value).toBe(false);
    expect(fetchMock).not.toHaveBeenCalled();
    host.querySelector<HTMLButtonElement>('button')!.click();
    await nextTick();
    expect(host.querySelector('button')?.disabled).toBe(true);
    host.querySelector<HTMLButtonElement>('button')!.click();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    resolveCheck(Response.json([{ name: 'v1.10.0' }, { name: 'v2.0.0-beta.1' }]));
    await settle();
    expect(updates.status.value).toBe('available');
    expect(host.textContent).toContain('发现新版本 v1.10.0');
    expect(updates.tagUrl.value).toBe('https://github.com/LovelyTeams/LovelyCarve/tree/v1.10.0');
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.spyOn(navigator.clipboard, 'writeText').mockImplementation(writeText);
    host.querySelectorAll<HTMLButtonElement>('button')[1]!.click();
    await settle();
    expect(writeText).toHaveBeenCalledWith(updates.tagUrl.value);
    expect(host.textContent).toContain('已复制');
    expect(updates.automatic.value).toBe(false);
  });

  it('persists explicit opt-in and checks automatically on the next startup', async () => {
    fetchMock.mockImplementation(async () => Response.json([{ name: 'v1.2.0' }]));
    mount();
    host.querySelector<HTMLInputElement>('[role="switch"]')!.click();
    await settle();
    expect(localStorage.getItem(preferenceKey)).toBe('true');
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(updates.status.value).toBe('current');
    app!.unmount(); app = undefined;
    mount(); await settle();
    expect(updates.automatic.value).toBe(true);
    expect(fetchMock).toHaveBeenCalledTimes(2);
    setLocale('en'); await settle();
    expect(host.textContent).toContain('No newer version tags found.');
  });

  it('ignores malformed opt-in values and tolerates an inaccessible preference store', async () => {
    localStorage.setItem(preferenceKey, '"true"');
    mount(); await settle();
    expect(fetchMock).not.toHaveBeenCalled();
    app!.unmount(); app = undefined;
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => { throw new Error('blocked'); });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('blocked'); });
    mount(); await settle();
    expect(updates.automatic.value).toBe(false);
    fetchMock.mockResolvedValue(Response.json([]));
    updates.automatic.value = true;
    await settle();
    expect(updates.status.value).toBe('current');
  });

  it('cancels on opt-out and rejects late results from an older check', async () => {
    let resolveOld: (response: Response) => void = () => {};
    fetchMock.mockImplementationOnce(() => new Promise<Response>(resolve => { resolveOld = resolve; }))
      .mockResolvedValueOnce(Response.json([{ name: 'v1.2.0' }]));
    mount();
    updates.automatic.value = true;
    const signal = fetchMock.mock.calls[0]![1].signal as AbortSignal;
    updates.automatic.value = false;
    expect(signal.aborted).toBe(true);
    await updates.checkNow();
    resolveOld(Response.json([{ name: 'v9.0.0' }]));
    await settle();
    expect(updates.status.value).toBe('current');
    expect(updates.latestTag.value).toBeNull();
  });

  it('shows rate-limit and network errors and allows retry', async () => {
    fetchMock.mockResolvedValueOnce(new Response('', { status: 403 })).mockRejectedValueOnce(new TypeError('offline')).mockResolvedValueOnce(Response.json([]));
    mount();
    await updates.checkNow();
    expect(updates.errorMessage.value).toContain('请求受限');
    await updates.checkNow();
    expect(updates.errorMessage.value).toContain('无法连接');
    await updates.checkNow();
    expect(updates.status.value).toBe('current');
  });

  it('times out pending network work and aborts it when the owner is disposed', async () => {
    vi.useFakeTimers();
    fetchMock.mockImplementation((_url, options) => new Promise((_resolve, reject) => {
      options.signal.addEventListener('abort', () => reject(new DOMException('Aborted', 'AbortError')));
    }));
    mount();
    const pending = updates.checkNow();
    await vi.advanceTimersByTimeAsync(15_000);
    await pending;
    expect(updates.errorMessage.value).toContain('超时');
    const second = updates.checkNow();
    const signal = fetchMock.mock.calls[1]![1].signal as AbortSignal;
    app!.unmount(); app = undefined;
    expect(signal.aborted).toBe(true);
    await second;
    expect(updates.status.value).toBe('idle');
  });
});
