// @vitest-environment happy-dom
import { createApp, h, nextTick, type App } from 'vue';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { DEFAULT_LOCALE, initLocale, LOCALE_KEY, localizeOptions, setLocale, tr, useLocale } from './useLocale';
import { buildImageNavGroups } from '../app/workspace/navigation';
import { checkRecoveryKey } from '../features/evidence/model/bitlockerRecoveryKey';
import { evidenceDisplayName, partitionTitle } from '../features/evidence/model/evidenceOverview';
import { BOOL_FILTER_OPTIONS } from '../features/files/model/model';
import WelcomeScreen from '../app/components/WelcomeScreen.vue';

let stop: (() => void) | undefined;
let app: App | undefined;
let host: HTMLElement | undefined;
beforeEach(() => {
  localStorage.clear();
  setLocale(DEFAULT_LOCALE);
});
afterEach(() => {
  app?.unmount(); app = undefined;
  host?.remove(); host = undefined;
  stop?.(); stop = undefined;
  vi.restoreAllMocks();
  setLocale(DEFAULT_LOCALE);
  localStorage.clear();
});

describe('application language', () => {
  it('restores the saved language before mounting and maintains the document language', () => {
    localStorage.setItem(LOCALE_KEY, 'en');
    stop = initLocale();
    expect(useLocale().locale.value).toBe('en');
    expect(document.documentElement.lang).toBe('en');
    setLocale('zh-CN');
    expect(localStorage.getItem(LOCALE_KEY)).toBe('zh-CN');
    expect(document.documentElement.lang).toBe('zh-CN');
  });

  it('synchronizes detached windows and cleans up its subscription', () => {
    stop = initLocale();
    expect(initLocale()).toBe(stop);
    const update = (key: string | null, newValue: string | null, storageArea: Storage = localStorage) =>
      window.dispatchEvent(new StorageEvent('storage', { key, newValue, storageArea }));
    update(LOCALE_KEY, 'en', sessionStorage);
    update('other-setting', 'en');
    expect(useLocale().locale.value).toBe('zh-CN');
    update(LOCALE_KEY, 'en');
    expect(tr('设置中心')).toBe('Settings');
    expect(document.documentElement.lang).toBe('en');
    update(null, null);
    expect(useLocale().locale.value).toBe(DEFAULT_LOCALE);
    update(LOCALE_KEY, 'invalid');
    expect(useLocale().locale.value).toBe(DEFAULT_LOCALE);
    stop(); stop = undefined;
    update(LOCALE_KEY, 'en');
    expect(useLocale().locale.value).toBe(DEFAULT_LOCALE);
  });

  it('remains usable when storage is unavailable or contains an invalid preference', () => {
    localStorage.setItem(LOCALE_KEY, 'invalid');
    stop = initLocale();
    expect(useLocale().locale.value).toBe(DEFAULT_LOCALE);
    stop(); stop = undefined;
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => { throw new Error('blocked'); });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('blocked'); });
    stop = initLocale();
    setLocale('en');
    expect(tr('设置中心')).toBe('Settings');
    expect(document.documentElement.lang).toBe('en');
  });

  it('updates existing navigation and option labels without changing stable values', async () => {
    const groups = buildImageNavGroups();
    const reference = groups[0].items;
    host = document.createElement('div'); document.body.append(host);
    app = createApp({ render: () => h('span', groups[0].groupLabel + groups[0].items[1].label) });
    app.mount(host);
    expect(host.textContent).toBe('镜像文件列表');
    setLocale('en'); await nextTick();
    expect(host.textContent).toBe('ImageFiles');
    expect(groups[0].items).toBe(reference);
    expect(localizeOptions(BOOL_FILTER_OPTIONS)).toEqual([
      { value: '', label: 'All' }, { value: '是', label: 'Yes' }, { value: '否', label: 'No' },
    ]);
    expect(BOOL_FILTER_OPTIONS[1].label).toBe('是');
  });

  it('translates validation messages and generated headings while preserving evidence names', () => {
    setLocale('en');
    expect(checkRecoveryKey('12345x', tr).groupError).toContain('Digits only');
    expect(partitionTitle({ id: 3, kind: 'ntfs', start_lba: 1, label: null }, tr)).toBe('Partition 3');
    expect(partitionTitle({ id: 3, kind: 'ntfs', start_lba: 1, label: '文件列表' }, tr)).toBe('文件列表');
    expect(evidenceDisplayName({ source_format: 'raw', path: 'D:/证据/文件列表' }, tr)).toBe('文件列表');
  });

  it('renders the welcome page in either language without translating case names or paths', async () => {
    host = document.createElement('div'); document.body.append(host);
    app = createApp(WelcomeScreen, {
      busy: false, dragging: false, loadLabel: '',
      recentCases: [{ name: '文件列表', dir: 'D:/证据/文件列表', openedAt: '2026-10-15T00:00:00Z' }],
    });
    app.mount(host);
    setLocale('en'); await nextTick();
    expect(host.querySelector('[aria-label="Start page"]')).not.toBeNull();
    expect(host.textContent).toContain('Choose files');
    expect(host.querySelector('.entry-name')?.textContent).toBe('文件列表');
    expect(host.querySelector('.recent-entry')?.getAttribute('title')).toBe('D:/证据/文件列表');
    setLocale('zh-CN'); await nextTick();
    expect(host.textContent).toContain('选择文件');
  });
});
