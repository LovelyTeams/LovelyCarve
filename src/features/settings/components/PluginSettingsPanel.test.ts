// @vitest-environment happy-dom
import { createApp, nextTick, type App } from 'vue';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import PluginSettingsPanel from './PluginSettingsPanel.vue';
import type { PluginListResponse, PluginPackageInfo } from '../../../lib/ipc/plugins';
import { resetPluginCatalogForTests } from '../../tasks/composables/usePluginCatalog';
import { resetTaskCenterForTests } from '../../tasks/composables/useTaskCenter';

const ipc = vi.hoisted(() => ({ list: vi.fn(), restart: vi.fn(), install: vi.fn(), uninstall: vi.fn(), open: vi.fn(), confirm: vi.fn(), invoke: vi.fn() }));
vi.mock('../../../lib/ipc/plugins', async original => ({
  ...await original<typeof import('../../../lib/ipc/plugins')>(),
  pluginList: ipc.list, pluginRestart: ipc.restart, pluginInstall: ipc.install, pluginUninstall: ipc.uninstall,
}));
vi.mock('../../../lib/ipc/client', () => ({ invokeCmd: ipc.invoke }));
vi.mock('../../../lib/ipc/events', () => ({ onEvent: async () => () => {}, emitEvent: vi.fn() }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: ipc.open }));
vi.mock('../../../composables/useConfirm', () => ({ confirm: ipc.confirm }));

const plugin: PluginPackageInfo = {
  id: 'org.test.demo', name: '示例插件', version: '1.0', directory: 'D:\\plugins\\demo', library: 'D:\\plugins\\demo\\demo.dll',
  libraryPresent: true, removable: false, memoryLimit: 1 << 30, fingerprint: '123456', state: 'configured', error: null, permissions: [],
  sdk: '0.2.0', schemaHash: 'fnv1a64:1', analyzers: [{ id: 'scan', name: '扫描', scope: 'system', platforms: ['linux'], category: 'baseinfo', trigger: 'manually', provides: [], requires: [] }],
  filesystems: ['demofs'], records: [{ recordType: 'demo.item', label: '条目', fields: [] }],
  locales: { en: { 'plugin.name': 'Demo plugin', 'record.demo.item': 'Items' } },
};
const installed: PluginPackageInfo = { ...plugin, id: 'org.test.extra', name: '附加插件', removable: true, filesystems: [], locales: {} };
const listing = (packages: PluginPackageInfo[]): PluginListResponse => ({
  packages, rejected: [{ directory: 'D:\\plugins\\broken', error: 'plugin.toml: protocol 1 is no longer supported' }], installDirectory: 'C:\\AppData\\plugins',
});
let app: App;
let host: HTMLElement;
async function settle() { for (let index = 0; index < 8; index++) await nextTick(); }
beforeEach(() => {
  vi.clearAllMocks();
  resetPluginCatalogForTests();
  resetTaskCenterForTests();
  ipc.list.mockResolvedValue(listing([plugin, installed]));
  ipc.restart.mockResolvedValue(undefined);
  ipc.invoke.mockResolvedValue([]);
  host = document.createElement('div'); document.body.append(host);
});
afterEach(() => { app?.unmount(); host.remove(); });
async function mount() { app = createApp(PluginSettingsPanel); app.mount(host); await settle(); }
function button(label: string, root: ParentNode = host) { return [...root.querySelectorAll<HTMLButtonElement>('button')].find(element => element.textContent?.trim() === label)!; }

describe('plugin package settings', () => {
  it('lists bundled and installed packages, packages that failed to load and the install directory', async () => {
    await mount();
    const cards = [...host.querySelectorAll<HTMLElement>('.plugin-card')];
    expect(cards.map(card => card.querySelector('.plugin-title strong')?.textContent)).toEqual(['示例插件', '附加插件']);
    expect(cards[0].textContent).toContain('内置');
    expect(cards[1].textContent).toContain('已安装');
    expect(button('卸载', cards[0])).toBeUndefined();
    expect(button('重置', cards[1])).toBeUndefined();
    expect(host.querySelector('.rejected')?.textContent).toContain('protocol 1');
    expect(host.querySelector('.plugin-hint')?.textContent).toContain('C:\\AppData\\plugins');
    expect(host.querySelector('details.plugin-disclosure')?.textContent).toContain('条目');
  });

  it('shows reset progress and its outcome, then failures inline', async () => {
    let finish!: () => void;
    ipc.restart.mockReturnValueOnce(new Promise<void>(resolve => { finish = resolve; }));
    await mount();
    button('重置').click(); await settle();
    expect(button('安装插件包…').disabled).toBe(true);
    finish(); await settle();
    await vi.waitFor(() => expect(host.querySelector('[role="status"]')?.textContent).toContain('下次使用时启动'));
    ipc.restart.mockRejectedValueOnce(new Error('插件仍在执行'));
    button('重置').click(); await settle();
    expect(host.querySelector('[role="alert"]')?.textContent).toContain('插件仍在执行');
  });

  it('installs a chosen package and reports the replaced version', async () => {
    await mount();
    ipc.open.mockResolvedValueOnce('D:\\Downloads\\extra.lcpkg');
    ipc.install.mockResolvedValueOnce({ pluginId: 'org.test.extra', version: '1.1', replaced: '1.0' });
    button('安装插件包…').click();
    await vi.waitFor(() => expect(host.querySelector('[role="status"]')?.textContent).toContain('1.0 → 1.1'));
    expect(ipc.open).toHaveBeenCalledWith(expect.objectContaining({ filters: [expect.objectContaining({ extensions: ['lcpkg'] })] }));
    expect(ipc.install).toHaveBeenCalledWith('D:\\Downloads\\extra.lcpkg');
    expect(ipc.list).toHaveBeenCalledTimes(2);
    expect(ipc.invoke).toHaveBeenCalledWith('sched_task_functions');

    ipc.open.mockResolvedValueOnce(null);
    button('安装插件包…').click(); await settle();
    expect(ipc.install).toHaveBeenCalledOnce();
  });

  it('removes an installed package only after confirmation', async () => {
    await mount();
    const card = host.querySelectorAll<HTMLElement>('.plugin-card')[1];
    ipc.confirm.mockResolvedValueOnce(false);
    button('卸载', card).click(); await settle();
    expect(ipc.uninstall).not.toHaveBeenCalled();
    ipc.confirm.mockResolvedValueOnce(true);
    ipc.uninstall.mockRejectedValueOnce(new Error('任务仍在运行'));
    button('卸载', card).click(); await settle();
    expect(ipc.uninstall).toHaveBeenCalledWith('org.test.extra');
    expect(host.querySelector('[role="alert"]')?.textContent).toContain('任务仍在运行');
  });

  it('keeps the listed packages when a refresh fails', async () => {
    await mount();
    ipc.list.mockRejectedValueOnce(new Error('目录不可读'));
    host.querySelector<HTMLButtonElement>('.plugin-toolbar-actions .ui-icon-btn')!.click(); await settle();
    expect(host.querySelector('.plugin-card')?.textContent).toContain('示例插件');
    expect(host.querySelector('[role="alert"]')?.textContent).toContain('目录不可读');
  });
});
