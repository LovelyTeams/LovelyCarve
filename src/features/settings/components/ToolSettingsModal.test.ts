// @vitest-environment happy-dom
import { createApp, h, nextTick, ref, type App } from 'vue';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import ToolSettingsModal from './ToolSettingsModal.vue';
import Titlebar from '../../../app/components/Titlebar.vue';
import { useTheme } from '../../../composables/useTheme';
import { useUiPreferences } from '../../../composables/useUiPreferences';
import { open } from '@tauri-apps/plugin-dialog';
import { setLocale } from '../../../composables/useLocale';

vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ minimize: vi.fn(), toggleMaximize: vi.fn(), close: vi.fn() }) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));
vi.mock('../../../composables/useAppVersion', () => ({ useAppVersion: () => ({ version: ref('2.3.4'), buildDate: '2026-10-12T01:02:03Z' }) }));
vi.mock('../../cases/api', () => ({ useCaseContext: () => ({ activeCase: ref(null) }) }));
vi.mock('./PluginSettingsPanel.vue', () => ({ default: { render: () => h('div', { 'data-plugins': true }, '插件清单') } }));
vi.mock('../../../app/components/Tabbar.vue', () => ({ default: { render: () => null } }));
const { setTheme } = useTheme();
const preferences = useUiPreferences();
let app: App;
let host: HTMLElement;
async function settle() { await nextTick(); await nextTick(); }
beforeEach(() => {
  vi.mocked(open).mockReset();
  localStorage.clear();
  setLocale('zh-CN');
  setTheme('light');
  preferences.motionPreference.value = 'system';
  preferences.mountTarget.value = 'drive';
  preferences.mountFolderRoot.value = '';
  preferences.autoMountCaseEvidence.value = false;
  host = document.createElement('div'); document.body.append(host);
});
afterEach(() => { app?.unmount(); host.remove(); setLocale('zh-CN'); });
function button(label: string) { return [...host.querySelectorAll<HTMLButtonElement>('button')].find(element => element.textContent?.trim() === label)!; }

describe('settings entry and preferences', () => {
  it('switches the open settings to English immediately and persists the choice', async () => {
    app = createApp(ToolSettingsModal, { visible: true }); app.mount(host);
    const language = host.querySelector<HTMLInputElement>('input[type="radio"][value="en"]')!;
    language.click(); await settle();
    expect(localStorage.getItem('lovelycarve-locale')).toBe('en');
    expect(host.querySelector('.ui-modal-title')?.textContent).toBe('Settings');
    expect(button('Dark')).toBeDefined();
    expect(host.querySelector('.settings-nav-item[aria-current="page"] strong')?.textContent).toBe('Appearance');
    expect(host.textContent).toContain('Changes saved automatically');
    preferences.mountTarget.value = 'folder'; await settle();
    expect(host.querySelector('.path-input')?.getAttribute('placeholder')).toBe('Leave empty to use the case folder');
    button('Restore page defaults').click(); await settle();
    expect(host.querySelector('.ui-modal-title')?.textContent).toBe('设置中心');
    expect(localStorage.getItem('lovelycarve-locale')).toBe('zh-CN');
  });
  it('provides logs and version information inside settings without a status bar', async () => {
    app = createApp(ToolSettingsModal, { visible: true }); app.mount(host);
    button('关于').click();
    await settle();
    expect(host.querySelector('[aria-label="软件信息"]')?.textContent).toContain('v2.3.4');
    expect(host.textContent).toContain('构建时间（本地）');
    expect(button('恢复当前页默认')).toBeUndefined();
    button('日志').click();
    await settle();
    expect(host.querySelector('[aria-label="搜索日志"]')).not.toBeNull();
    expect(host.textContent).toContain('最近 5,000 条');
    expect(host.querySelector('.statusbar')).toBeNull();
    button('界面').click();
    await settle();
    expect(host.querySelector('.theme-options')).not.toBeNull();
  });

  it('opens interface settings directly from the titlebar gear', async () => {
    const visible = ref(false);
    app = createApp({ render: () => [h(Titlebar, { onOpenSettings: () => { visible.value = true; } }), h(ToolSettingsModal, { visible: visible.value, onClose: () => { visible.value = false; } })] });
    app.mount(host);
    expect(host.querySelector('[role="dialog"]')).toBeNull();
    host.querySelector<HTMLButtonElement>('[aria-label="设置中心"]')!.click();
    await settle();
    expect(host.querySelector('[role="dialog"]')).not.toBeNull();
    expect(host.querySelector('.settings-nav-item[aria-current="page"] strong')?.textContent).toBe('界面');
    expect(host.querySelector('.theme-options')).not.toBeNull();
    expect(host.querySelector('img[alt*="头像"]')).toBeNull();
    expect(host.textContent).not.toContain('用户资料');
    button('完成').click();
    await settle();
    expect(visible.value).toBe(false);
  });

  it('persists theme changes and restores all interface preferences together', async () => {
    app = createApp(ToolSettingsModal, { visible: true }); app.mount(host);
    button('深色').click();
    await settle();
    expect(document.documentElement.dataset.theme).toBe('dark');
    expect(localStorage.getItem('lovelycarve-theme')).toBe('dark');
    expect(button('深色').getAttribute('aria-pressed')).toBe('true');
    preferences.motionPreference.value = 'off';
    preferences.mountTarget.value = 'folder';
    preferences.mountFolderRoot.value = 'D:\\挂载';
    preferences.autoMountCaseEvidence.value = true;
    await settle();
    button('恢复当前页默认').click();
    await settle();
    expect(document.documentElement.dataset.theme).toBe('light');
    expect([preferences.motionPreference.value, preferences.mountTarget.value, preferences.mountFolderRoot.value, preferences.autoMountCaseEvidence.value]).toEqual(['system', 'drive', '', false]);
  });

  it('supports opening plugin settings and switching back to interface settings', async () => {
    app = createApp(ToolSettingsModal, { visible: true, initialSection: 'plugins' }); app.mount(host);
    expect(host.querySelector('[data-plugins]')).not.toBeNull();
    expect(button('恢复当前页默认')).toBeUndefined();
    [...host.querySelectorAll<HTMLButtonElement>('.settings-nav-item')].find(element => element.querySelector('strong')?.textContent === '界面')!.click();
    await settle();
    expect(host.querySelector('.theme-options')).not.toBeNull();
    expect(button('恢复当前页默认').disabled).toBe(false);
  });

  it('labels the mount path, groups mount choices and reports picker failures inline', async () => {
    preferences.mountTarget.value = 'folder';
    vi.mocked(open).mockRejectedValue(new Error('目录选择器不可用'));
    app = createApp(ToolSettingsModal, { visible: true }); app.mount(host);
    const radios = [...host.querySelectorAll<HTMLInputElement>('input[type="radio"]')].filter(input => ['drive', 'folder'].includes(input.value));
    expect(radios[0].name).toBeTruthy();
    expect(radios[0].name).toBe(radios[1].name);
    const pathInput = host.querySelector<HTMLInputElement>('.path-input')!;
    expect(host.querySelector(`label[for="${pathInput.id}"]`)?.textContent).toBe('默认根目录');
    button('选择').click(); await settle();
    expect(host.querySelector('[role="alert"]')?.textContent).toContain('目录选择器不可用');
    expect(button('选择').disabled).toBe(false);
  });
});
