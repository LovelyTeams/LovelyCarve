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
  sources: vi.fn(), setSources: vi.fn(), list: vi.fn(), download: vi.fn(), downloadOfficial: vi.fn(), release: vi.fn(),
  pluginList: vi.fn(), install: vi.fn(), invoke: vi.fn(), open: vi.fn(),
}));
vi.mock('../../../lib/ipc/pluginMarket', () => ({
  pluginMarketSources: ipc.sources, pluginMarketSetSources: ipc.setSources, pluginMarketList: ipc.list,
  pluginMarketDownload: ipc.download, pluginMarketRelease: ipc.release,
  pluginMarketDownloadOfficial: ipc.downloadOfficial,
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
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: ipc.open }));

const SOURCE = 'https://plugins.example.com/lab/index.json';
const catalog = (status: MarketCatalog['packages'][number]['status'], installed: string | null = null): MarketCatalog => ({
  source: SOURCE, title: '实验室', description: null, fetchedAt: '2026-11-25T08:00:00Z', platform: 'windows-x86_64',
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
  ipc.sources.mockResolvedValue({ sources: [SOURCE] });
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
  it('uses the official index without adding a source and installs only after confirmation', async () => {
    const source = 'https://lovelyteams.github.io/LovelyCarve-Plugin-Site/index.json';
    ipc.sources.mockResolvedValueOnce({ sources: [] });
    ipc.downloadOfficial.mockResolvedValueOnce({ ...download, source });
    await mount({ pluginId: inspection.pluginId, version: inspection.version });
    const confirmation = await dialog();
    expect(ipc.downloadOfficial).toHaveBeenCalledExactlyOnceWith(inspection.pluginId, inspection.version);
    expect(ipc.setSources).not.toHaveBeenCalled();
    expect(ipc.install).not.toHaveBeenCalled();
    expect(confirmation.textContent).toContain(source);

    ipc.install.mockResolvedValueOnce({ pluginId: inspection.pluginId, version: inspection.version, replaced: null });
    button('安装', confirmation).click();
    await vi.waitFor(() => expect(host.querySelector('[role="status"]')?.textContent).toContain('已安装 org.example.demo 1.2.0'));
    expect(ipc.install).toHaveBeenCalledExactlyOnceWith(inspection.archive, inspection.sha256);
    expect(ipc.release).toHaveBeenCalledExactlyOnceWith(inspection.sha256);
  });

  it('adds a source and states a rejected one at its field', async () => {
    ipc.sources.mockResolvedValueOnce({ sources: [] });
    await mount();
    expect(host.textContent).toContain('未配置索引源');
    const input = host.querySelector<HTMLInputElement>('.source-input')!;
    input.value = 'http://plugins.example.com/index.json'; input.dispatchEvent(new Event('input'));
    ipc.setSources.mockRejectedValueOnce(new IpcCommandError({
      code: 'invalidInput', message: 'only https:// addresses and local directories are accepted', context: { reason: 'invalidSource' }, retryable: false,
    }));
    host.querySelector<HTMLFormElement>('.source-form')!.dispatchEvent(new Event('submit'));
    await settle();
    expect(input.getAttribute('aria-invalid')).toBe('true');
    expect(host.querySelector('.ui-field-error')?.textContent).toContain('索引源无效');

    input.value = 'https://plugins.example.com/lab/'; input.dispatchEvent(new Event('input'));
    ipc.setSources.mockResolvedValueOnce({ sources: [SOURCE] });
    host.querySelector<HTMLFormElement>('.source-form')!.dispatchEvent(new Event('submit'));
    await vi.waitFor(() => expect(host.querySelector('.market-card')).not.toBeNull());
    expect(ipc.setSources).toHaveBeenLastCalledWith(['https://plugins.example.com/lab/']);
    expect(ipc.list).toHaveBeenCalledWith(SOURCE, false);
    expect(input.value).toBe('');
  });

  it('downloads, confirms and installs a listed version, then releases the download', async () => {
    await mount();
    const card = host.querySelector<HTMLElement>('.market-card')!;
    expect(card.querySelector('.market-title strong')?.textContent).toBe('示例');
    expect(card.textContent).toContain('当前平台不可用');
    ipc.download.mockResolvedValueOnce(download);
    button('安装', card).click();
    const confirmation = await dialog();
    expect(ipc.download).toHaveBeenCalledWith(SOURCE, 'org.example.demo', '1.2.0');
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
