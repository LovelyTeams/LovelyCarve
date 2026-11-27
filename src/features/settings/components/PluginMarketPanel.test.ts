// @vitest-environment happy-dom
import { createApp, nextTick, type App } from 'vue';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import PluginMarketPanel from './PluginMarketPanel.vue';
import { IpcCommandError } from '../../../lib/ipc/client';
import type { PackageInspection } from '../../../lib/ipc/plugins';
import type { MarketCatalog, MarketDownload } from '../../../lib/ipc/pluginMarket';
import type { PluginInstallLink } from '../../../lib/ipc/evidenceLinks';
import { resetPluginCatalogForTests } from '../../tasks/composables/usePluginCatalog';
import { resetTaskCenterForTests } from '../../tasks/composables/useTaskCenter';

const ipc = vi.hoisted(() => ({
  list: vi.fn(), download: vi.fn(), release: vi.fn(), pluginList: vi.fn(), install: vi.fn(), invoke: vi.fn(),
}));
vi.mock('../../../lib/ipc/pluginMarket', () => ({
  pluginMarketList: ipc.list, pluginMarketDownload: ipc.download, pluginMarketRelease: ipc.release,
}));
vi.mock('../../../lib/ipc/plugins', async original => ({
  ...await original<typeof import('../../../lib/ipc/plugins')>(),
  pluginList: ipc.pluginList, pluginInstall: ipc.install,
}));
vi.mock('../../../lib/ipc/client', async original => ({
  ...await original<typeof import('../../../lib/ipc/client')>(),
  invokeCmd: ipc.invoke,
}));
vi.mock('../../../lib/ipc/events', () => ({ onEvent: async () => () => {}, emitEvent: vi.fn() }));

const SOURCE = 'https://lovelyteams.github.io/LovelyCarve-Plugin-Site/index.json';
const catalog = (status: MarketCatalog['packages'][number]['status'], installed: string | null = null): MarketCatalog => ({
  source: SOURCE, title: 'LovelyCarve', description: null, fetchedAt: '2026-11-25T08:00:00Z', platform: 'windows-x86_64',
  packages: [{
    id: 'org.example.demo', name: '示例', description: '示例插件', publisher: 'Example', license: 'MIT',
    locales: { en: { 'plugin.name': 'Demo' } }, latest: '1.2.0', installed, status,
    versions: [
      { version: '1.2.0', published: '2026-11-20T00:00:00Z', permissions: [], prerelease: false, size: 2048, sha256: 'a'.repeat(64) },
      { version: '1.1.0', published: '2026-10-01T00:00:00Z', permissions: [], prerelease: false, size: null, sha256: null },
    ],
  }],
});
const inspection: PackageInspection = {
  archive: 'C:\\data\\plugin-market\\downloads\\a.lcpkg', sha256: 'a'.repeat(64), size: 2048, pluginId: 'org.example.demo', name: '示例',
  version: '1.2.0', description: '示例插件', publisher: 'Example', license: 'MIT', permissions: [], memoryLimit: 1 << 30, analyzers: [],
  filesystems: [], recordTypes: 0, sdk: '0.2.0', schemaHash: null, locales: {}, library: 'demo.dll', librarySha256: 'b'.repeat(64),
  installed: null, change: 'new', blocked: null,
};
const download: MarketDownload = { source: SOURCE, archive: inspection.archive, sha256: inspection.sha256, inspection };

let app: App;
let host: HTMLElement;
async function settle() { for (let index = 0; index < 8; index++) await nextTick(); }
function button(label: string, root: ParentNode = host) {
  return [...root.querySelectorAll<HTMLButtonElement>('button')].find(element => element.textContent?.trim() === label)!;
}
async function mount(installLink?: PluginInstallLink) {
  app = createApp(PluginMarketPanel, { installLink });
  app.mount(host);
  await settle();
}
async function dialog() {
  return vi.waitFor(() => {
    const element = document.body.querySelector<HTMLElement>('.install-modal');
    expect(element).not.toBeNull();
    return element!;
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  resetPluginCatalogForTests();
  resetTaskCenterForTests();
  ipc.list.mockResolvedValue(catalog('notInstalled'));
  ipc.release.mockResolvedValue(undefined);
  ipc.pluginList.mockResolvedValue({ packages: [], rejected: [], installDirectory: null });
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

describe('plugin market', () => {
  it('lists the official index and reads it again on request', async () => {
    await mount();
    expect(ipc.list).toHaveBeenCalledExactlyOnceWith(false);
    expect(host.querySelector('.market-meta')?.textContent).toContain('LovelyCarve');
    expect(host.querySelector('.source-form')).toBeNull();
    host.querySelector<HTMLButtonElement>('[aria-label="重新读取索引"]')!.click(); await settle();
    expect(ipc.list).toHaveBeenLastCalledWith(true);
  });

  it('opens an install link from the release page and installs only after confirmation', async () => {
    ipc.download.mockResolvedValueOnce(download);
    await mount({ pluginId: inspection.pluginId, version: inspection.version });
    const confirmation = await dialog();
    expect(ipc.download).toHaveBeenCalledExactlyOnceWith(inspection.pluginId, inspection.version);
    expect(ipc.install).not.toHaveBeenCalled();
    expect(confirmation.textContent).toContain(SOURCE);

    ipc.install.mockResolvedValueOnce({ pluginId: inspection.pluginId, version: inspection.version, replaced: null });
    button('安装', confirmation).click();
    await vi.waitFor(() => expect(host.querySelector('[role="status"]')?.textContent).toContain('已安装 org.example.demo 1.2.0'));
    expect(ipc.install).toHaveBeenCalledExactlyOnceWith(inspection.archive, inspection.sha256);
    expect(ipc.release).toHaveBeenCalledExactlyOnceWith(inspection.sha256);
  });

  it('downloads, confirms and installs a listed version, then releases the download', async () => {
    await mount();
    const card = host.querySelector<HTMLElement>('.market-card')!;
    expect(card.querySelector('.market-title strong')?.textContent).toBe('示例');
    expect(card.textContent).toContain('当前平台不可用');
    ipc.download.mockResolvedValueOnce(download);
    button('安装', card).click();
    const confirmation = await dialog();
    expect(ipc.download).toHaveBeenCalledWith('org.example.demo', '1.2.0');
    expect(confirmation.textContent).toContain(SOURCE);
    expect(confirmation.textContent).toContain('新安装');

    ipc.install.mockResolvedValueOnce({ pluginId: 'org.example.demo', version: '1.2.0', replaced: null });
    ipc.list.mockResolvedValueOnce(catalog('installed', '1.2.0'));
    button('安装', confirmation).click();
    await vi.waitFor(() => expect(host.querySelector('[role="status"]')?.textContent).toContain('已安装 org.example.demo 1.2.0'));
    expect(ipc.install).toHaveBeenCalledWith(inspection.archive, inspection.sha256);
    expect(ipc.release).toHaveBeenCalledWith(inspection.sha256);
    await vi.waitFor(() => expect(host.querySelector('.market-card .ui-badge')?.textContent).toBe('已安装'));
  });

  it('releases a download the user declined', async () => {
    await mount();
    ipc.download.mockResolvedValueOnce(download);
    button('安装', host.querySelector<HTMLElement>('.market-card')!).click();
    const confirmation = await dialog();
    button('取消', confirmation).click(); await settle();
    expect(ipc.install).not.toHaveBeenCalled();
    expect(ipc.release).toHaveBeenCalledWith(inspection.sha256);
    await closed();
  });

  it('names a download that does not match the index', async () => {
    await mount();
    ipc.download.mockRejectedValueOnce(new IpcCommandError({
      code: 'corruptData', message: 'the package from x does not match the index', context: { reason: 'integrity' }, retryable: false,
    }));
    button('安装', host.querySelector<HTMLElement>('.market-card')!).click(); await settle();
    expect(host.querySelector('[role="alert"]')?.textContent).toContain('下载的插件包与索引不符');
    await closed();
  });
});
