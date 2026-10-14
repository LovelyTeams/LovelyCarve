// @vitest-environment happy-dom
import { createApp, nextTick, type App } from 'vue';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import PluginSettingsPanel from './PluginSettingsPanel.vue';
import type { PluginInfo } from '../../../lib/ipc/plugins';

const ipc = vi.hoisted(() => ({ list: vi.fn(), restart: vi.fn(), rescan: vi.fn(), report: vi.fn(), subscribe: vi.fn() }));
vi.mock('../../../lib/ipc/plugins', () => ({ pluginList: ipc.list, pluginRestart: ipc.restart, pluginRescan: ipc.rescan, pluginScanReport: ipc.report, onPluginCatalogChanged: ipc.subscribe }));
const plugin: PluginInfo = { id: 'demo', name: '示例插件', version: '1.0', directory: 'D:\\plugins\\demo', library: 'demo.dll', isolation: 'process', fingerprint: '123456', state: 'configured', error: null, permissions: [], capabilities: [] };
let app: App;
let host: HTMLElement;
async function settle() { for (let index = 0; index < 6; index++) await nextTick(); }
beforeEach(() => { vi.clearAllMocks(); ipc.list.mockResolvedValue([plugin]); ipc.report.mockResolvedValue({ generation: 1, roots: ['D:/plugins'], entries: [] }); ipc.rescan.mockResolvedValue({ generation: 2, roots: ['D:/plugins'], entries: [] }); ipc.subscribe.mockResolvedValue(() => {}); ipc.restart.mockResolvedValue(undefined); host = document.createElement('div'); document.body.append(host); });
afterEach(() => { app?.unmount(); host.remove(); });
async function mount() { app = createApp(PluginSettingsPanel); app.mount(host); await settle(); }
function button(label: string) { return [...host.querySelectorAll<HTMLButtonElement>('button')].find(element => element.textContent?.trim() === label)!; }

describe('compact plugin settings', () => {
  it('keeps technical details folded and shows reset progress and success in the card', async () => {
    let finish!: () => void;
    ipc.restart.mockReturnValueOnce(new Promise<void>(resolve => { finish = resolve; }));
    await mount(); expect(host.querySelector('details')?.open).toBe(false);
    button('重置').click(); await settle();
    expect(button('重置中').disabled).toBe(true); expect(button('检查插件目录').disabled).toBe(true);
    finish(); await settle();
    expect(host.querySelector('[role="status"]')?.textContent).toContain('下次使用时启动');
    expect(button('重置').disabled).toBe(false);
  });

  it('shows reset failure inline and keeps the installed list available after a refresh failure', async () => {
    await mount(); ipc.restart.mockRejectedValueOnce(new Error('插件仍在执行'));
    button('重置').click(); await settle();
    expect(host.querySelector('[role="alert"]')?.textContent).toContain('插件仍在执行');
    ipc.list.mockRejectedValueOnce(new Error('目录不可读')); button('检查插件目录').click(); await settle();
    expect(host.querySelector('.plugin-card')?.textContent).toContain('示例插件');
    expect([...host.querySelectorAll('[role="alert"]')].some(element => element.textContent?.includes('目录不可读'))).toBe(true);
  });

  it('does not reload plugins after a pending reset resolves in an unmounted panel', async () => {
    let finish!: () => void;
    ipc.restart.mockReturnValueOnce(new Promise<void>(resolve => { finish = resolve; }));
    await mount(); button('重置').click(); await settle(); app.unmount();
    finish(); await settle(); expect(ipc.list).toHaveBeenCalledOnce();
  });
});


describe('manual plugin directory inspection', () => {
  it('does not scan on mount and publishes manual results with diagnostics', async () => {
    await mount();
    expect(ipc.rescan).not.toHaveBeenCalled();
    const report = { generation: 2, roots: ['D:/plugins'], entries: [{ directory: 'D:/plugins/new', pluginId: 'new', version: '2', status: 'updatePending', message: '更新待重启' }] };
    ipc.rescan.mockResolvedValueOnce(report);
    ipc.report.mockResolvedValueOnce(report);
    button('检查插件目录').click(); await settle();
    expect(ipc.rescan).toHaveBeenCalledOnce();
    expect(host.textContent).toContain('检查完成');
    expect(host.textContent).toContain('扫描版本 2');
  });

  it('releases a late subscription and ignores a late scan after unmount', async () => {
    let subscribe!: (stop: () => void) => void;
    ipc.subscribe.mockReturnValueOnce(new Promise(resolve => { subscribe = resolve; }));
    app = createApp(PluginSettingsPanel); app.mount(host); app.unmount();
    const stop = vi.fn(); subscribe(stop); await settle();
    expect(stop).toHaveBeenCalledOnce();
    expect(ipc.list).not.toHaveBeenCalled();
  });

  it('reloads the list on catalog events without rescanning disk', async () => {
    await mount();
    ipc.list.mockResolvedValueOnce([{ ...plugin, id: 'new', name: '新插件' }]);
    ipc.subscribe.mock.calls[0][0](2); await settle();
    expect(host.textContent).toContain('新插件');
    expect(ipc.rescan).not.toHaveBeenCalled();
  });
});
