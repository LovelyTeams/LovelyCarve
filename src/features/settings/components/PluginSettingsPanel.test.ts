// @vitest-environment happy-dom
import { createApp, nextTick, type App } from 'vue';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import PluginSettingsPanel from './PluginSettingsPanel.vue';
import type { PackageInspection, PluginListResponse, PluginPackageInfo } from '../../../lib/ipc/plugins';
import { IpcCommandError } from '../../../lib/ipc/client';
import { resetPluginCatalogForTests } from '../../tasks/composables/usePluginCatalog';
import { resetTaskCenterForTests } from '../../tasks/composables/useTaskCenter';

const ipc = vi.hoisted(() => ({
  list: vi.fn(), restart: vi.fn(), inspect: vi.fn(), install: vi.fn(), uninstall: vi.fn(), readme: vi.fn(),
  open: vi.fn(), confirm: vi.fn(), invoke: vi.fn(),
}));
vi.mock('../../../lib/ipc/plugins', async original => ({
  ...await original<typeof import('../../../lib/ipc/plugins')>(),
  pluginList: ipc.list, pluginRestart: ipc.restart, pluginInspect: ipc.inspect, pluginInstall: ipc.install,
  pluginUninstall: ipc.uninstall, pluginReadme: ipc.readme,
}));
vi.mock('../../../lib/ipc/client', async original => ({
  ...await original<typeof import('../../../lib/ipc/client')>(),
  invokeCmd: ipc.invoke,
}));
vi.mock('../../../lib/ipc/events', () => ({ onEvent: async () => () => {}, emitEvent: vi.fn() }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: ipc.open }));
vi.mock('../../../composables/useConfirm', () => ({ confirm: ipc.confirm }));

const plugin: PluginPackageInfo = {
  id: 'org.test.demo', name: '示例插件', version: '1.0.0', description: '示例说明', publisher: 'LovelyTeams', license: 'MIT', hasReadme: true,
  directory: 'D:\\plugins\\demo', library: 'D:\\plugins\\demo\\demo.dll',
  libraryPresent: true, removable: false, memoryLimit: 1 << 30, fingerprint: '123456', state: 'configured', error: null, permissions: [],
  sdk: '0.2.0', schemaHash: 'fnv1a64:1', analyzers: [{ id: 'scan', name: '扫描', scope: 'system', platforms: ['linux'], category: 'baseinfo', trigger: 'manually', provides: [], requires: [] }],
  filesystems: ['demofs'], records: [{ recordType: 'demo.item', label: '条目', fields: [] }],
  locales: { en: { 'plugin.name': 'Demo plugin', 'record.demo.item': 'Items' } },
};
const installed: PluginPackageInfo = {
  ...plugin, id: 'org.test.extra', name: '附加插件', removable: true, filesystems: [], locales: {}, description: null, hasReadme: false,
};
const inspection = (overrides: Partial<PackageInspection> = {}): PackageInspection => ({
  archive: 'D:\\Downloads\\extra.lcpkg', sha256: 'a'.repeat(64), size: 2048, pluginId: 'org.test.extra', name: '附加插件', version: '1.1.0',
  description: null, publisher: null, license: null, permissions: ['network'], memoryLimit: 1 << 30,
  analyzers: [{ id: 'scan', name: '扫描', scope: 'system' }], filesystems: [], recordTypes: 1, sdk: '0.2.0', schemaHash: 'fnv1a64:1',
  locales: {}, library: 'extra.dll', librarySha256: 'b'.repeat(64),
  installed: { version: '1.0.0', bundled: false, librarySha256: 'c'.repeat(64) }, change: 'upgrade', blocked: null,
  ...overrides,
});
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
afterEach(() => {
  app?.unmount(); host.remove();
  // A dialog still leaving through its transition when the test ended.
  document.body.querySelectorAll('.ui-modal-overlay').forEach(element => element.remove());
});
async function closed() {
  await vi.waitFor(() => expect(document.body.querySelector('.install-modal')).toBeNull());
}
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

  it('inspects a chosen package and installs the inspected bytes only after confirmation', async () => {
    await mount();
    ipc.open.mockResolvedValueOnce('D:\\Downloads\\extra.lcpkg');
    ipc.inspect.mockResolvedValueOnce(inspection());
    button('安装插件包…').click();
    const dialog = await vi.waitFor(() => {
      const element = document.body.querySelector<HTMLElement>('.install-modal');
      expect(element).not.toBeNull();
      return element!;
    });
    expect(ipc.open).toHaveBeenCalledWith(expect.objectContaining({ filters: [expect.objectContaining({ extensions: ['lcpkg'] })] }));
    expect(ipc.inspect).toHaveBeenCalledWith('D:\\Downloads\\extra.lcpkg');
    expect(dialog.textContent).toContain('升级自 1.0.0');
    expect(dialog.textContent).toContain('网络访问');
    expect(dialog.textContent).toContain('a'.repeat(64));
    expect(ipc.install).not.toHaveBeenCalled();

    ipc.install.mockResolvedValueOnce({ pluginId: 'org.test.extra', version: '1.1.0', replaced: '1.0.0' });
    button('安装', dialog).click();
    await vi.waitFor(() => expect(host.querySelector('[role="status"]')?.textContent).toContain('1.0.0 → 1.1.0'));
    expect(ipc.install).toHaveBeenCalledWith('D:\\Downloads\\extra.lcpkg', 'a'.repeat(64));
    expect(ipc.list).toHaveBeenCalledTimes(2);
    expect(ipc.invoke).toHaveBeenCalledWith('sched_task_functions');
    await closed();

    ipc.open.mockResolvedValueOnce(null);
    button('安装插件包…').click(); await settle();
    expect(ipc.inspect).toHaveBeenCalledOnce();
  });

  it('names why a package cannot be installed and keeps the dialog on a failed install', async () => {
    await mount();
    ipc.open.mockResolvedValueOnce('D:\\Downloads\\bundled.lcpkg');
    ipc.inspect.mockResolvedValueOnce(inspection({ blocked: { reason: 'bundled', message: 'org.test.demo is a bundled plugin and cannot be replaced' } }));
    button('安装插件包…').click();
    const blocked = await vi.waitFor(() => {
      const element = document.body.querySelector<HTMLElement>('.install-modal');
      expect(element).not.toBeNull();
      return element!;
    });
    expect(blocked.textContent).toContain('内置插件不能替换');
    expect(button('安装', blocked).disabled).toBe(true);
    button('取消', blocked).click(); await settle();
    await closed();

    ipc.open.mockResolvedValueOnce('D:\\Downloads\\extra.lcpkg');
    ipc.inspect.mockResolvedValueOnce(inspection());
    button('安装插件包…').click();
    const dialog = await vi.waitFor(() => {
      const element = document.body.querySelector<HTMLElement>('.install-modal');
      expect(element).not.toBeNull();
      return element!;
    });
    ipc.install.mockRejectedValueOnce(new IpcCommandError({
      code: 'ffiUnavailable', message: 'the library speaks LPP 3', context: { reason: 'libraryRejected' }, retryable: false,
    }));
    button('安装', dialog).click(); await settle();
    expect(dialog.querySelector('[role="alert"]')?.textContent).toContain('插件库未通过启动校验');
    expect(dialog.querySelector('[role="alert"]')?.textContent).toContain('the library speaks LPP 3');
    button('取消', dialog).click(); await settle();
  });

  it('shows the publisher, license and README of a package', async () => {
    ipc.readme.mockResolvedValueOnce('# 示例\n\n- 第一项');
    await mount();
    const card = host.querySelector<HTMLElement>('.plugin-card')!;
    expect(card.querySelector('.plugin-description')?.textContent).toBe('示例说明');
    expect(card.textContent).toContain('LovelyTeams');
    const readme = card.querySelector<HTMLDetailsElement>('details.readme-disclosure')!;
    readme.open = true;
    readme.dispatchEvent(new Event('toggle'));
    await vi.waitFor(() => expect(card.querySelector('.readme h1')?.textContent).toBe('示例'));
    expect(ipc.readme).toHaveBeenCalledWith('org.test.demo');
    expect(host.querySelectorAll('.plugin-card')[1].querySelector('details.readme-disclosure')).toBeNull();
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
