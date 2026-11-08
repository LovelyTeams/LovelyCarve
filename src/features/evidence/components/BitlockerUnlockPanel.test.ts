// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, h, nextTick, ref, type App } from 'vue';
import type { BitlockerScanJob, BitlockerUnlockJob, BitlockerVmkScanResult } from '../model/bitlocker';
import BitlockerUnlockPanel from './BitlockerUnlockPanel.vue';

const calls = vi.hoisted(() => ({ metadata: vi.fn(), reminder: vi.fn(), password: vi.fn(), recovery: vi.fn(), scan: vi.fn(), vmk: vi.fn(), key: vi.fn(), cancelUnlock: vi.fn(), cancelScan: vi.fn(), open: vi.fn() }));
const job = ref<BitlockerUnlockJob>(); const scanJob = ref<BitlockerScanJob>();
vi.mock('../composables/useFileSession', () => ({ dismissBitlockerReminder: calls.reminder }));
vi.mock('../composables/useBitlockerUnlocks', () => ({ useBitlockerJob: () => job, useBitlockerScanJob: () => scanJob, startBitlockerPasswordUnlock: calls.password, startBitlockerRecoveryUnlock: calls.recovery, startBitlockerMemoryScan: calls.scan, startBitlockerVmkUnlock: calls.vmk, startBitlockerKeyUnlock: calls.key, cancelBitlockerUnlock: calls.cancelUnlock, cancelBitlockerMemoryScan: calls.cancelScan }));
vi.mock('../../../lib/ipc/bitlocker', () => ({ bitlockerMetadata: calls.metadata }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: calls.open }));

let app: App; let host: HTMLDivElement;
const imageId = ref('a');
const active = ref(false);
const close = vi.fn(() => { active.value = false; });
const key = '031174-056914-397793-502348-055847-196306-284306-262174';
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { resolve, promise }; }
async function settle() { for (let index = 0; index < 10; index++) await nextTick(); }
async function mount() {
  host = document.createElement('div'); document.body.append(host);
  app = createApp({ render: () => h(BitlockerUnlockPanel, {
    imageId: imageId.value, partitionId: 1, active: active.value, unlocked: false, onClose: close,
  }) });
  app.mount(host); await settle();
}
async function open() {
  active.value = true; await settle();
  expect(host.querySelector('.bitlocker-panel')).not.toBeNull();
}
function popup() { return host.querySelector<HTMLElement>('.bitlocker-panel')!; }
function button(label: string) { return [...popup().querySelectorAll<HTMLButtonElement>('button')].find(candidate => candidate.textContent?.trim() === label)!; }
async function click(label: string) {
  // Give newly rendered Vue listeners a later native event timestamp before bubbling.
  await new Promise(resolve => setTimeout(resolve, 1));
  button(label).click(); await settle();
}
function input(value: string) { const field = popup().querySelector<HTMLInputElement>('input')!; field.value = value; field.dispatchEvent(new Event('input', { bubbles: true })); return field; }
beforeEach(() => { vi.resetAllMocks(); imageId.value = 'a'; job.value = undefined; scanJob.value = undefined; active.value = false; });
afterEach(() => { app?.unmount(); host?.remove(); vi.unstubAllGlobals(); });

describe('BitLocker unlock panel', () => {
  it('focuses the recovery field, links validation feedback and submits the normalized key', async () => {
    await mount(); await open(); const field = input('031174-0569'); await settle();
    expect(field.getAttribute('aria-invalid')).toBeNull();
    field.dispatchEvent(new Event('blur')); await settle();
    expect(field.getAttribute('aria-invalid')).toBe('true');
    expect(document.getElementById(field.getAttribute('aria-describedby')!)?.textContent).toContain('只有 4 位');
    input(key.replaceAll('-', ' ')); await settle(); calls.recovery.mockResolvedValue(true);
    await click('解锁'); await settle();
    expect(calls.recovery).toHaveBeenCalledWith('a', 1, key); expect(host.querySelector('.bitlocker-panel')).toBeNull();
  });
  it('focuses the password field when switching credential mode and submits it', async () => {
    calls.password.mockResolvedValue(true); await mount(); await open(); await click('密码'); await settle();
    const field = input('secret'); await settle(); expect(document.activeElement).toBe(field);
    field.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); await settle();
    expect(calls.password).toHaveBeenCalledWith('a', 1, 'secret'); expect(close).toHaveBeenCalledOnce();
  });
  it('keeps memory scanning alive after closing the popup and displays the returned candidates on reopen', async () => {
    const scan = deferred<BitlockerVmkScanResult>(); calls.open.mockResolvedValue('D:/memory.raw'); calls.scan.mockReturnValue(scan.promise);
    await mount(); await open(); await click('内存镜像'); await settle(); await click('选择内存镜像'); await settle(); await click('开始扫描');
    scanJob.value = { imageId: 'a', partitionId: 1, taskId: 'scan', bytesDone: 50, bytesTotal: 100, cancelling: false }; await settle();
    expect(popup().textContent).toContain('50.0%');
    active.value = false; await settle();
    expect(host.querySelector('.bitlocker-panel')).toBeNull(); expect(calls.cancelScan).not.toHaveBeenCalled();
    scan.resolve({ partition_id: 1, candidates: [{ offset: 1, type_flag: 0, key_type: 0, vmk_hex: 'AA', recovery_key: key, already_tried: true }] }); scanJob.value = undefined; await settle(); await open();
    expect(popup().textContent).toContain(key); expect(popup().querySelector<HTMLButtonElement>('.bde-mem-row')?.disabled).toBe(true);
  });
  it('does not publish a late file selection after the panel is closed and reopened', async () => {
    const selected = deferred<string>(); calls.open.mockReturnValue(selected.promise);
    await mount(); await open(); await click('内存镜像'); await click('选择内存镜像');
    active.value = false; await settle(); await open();
    selected.resolve('D:/old.raw'); await settle();
    expect(popup().querySelector('.bde-file-path')).toBeNull();
  });
  it('does not close a different view on an old unlock completion', async () => {
    const unlock = deferred<boolean>(); calls.password.mockReturnValue(unlock.promise);
    await mount(); await open(); await click('密码'); await settle(); input('old-password'); await settle(); await click('解锁');
    active.value = false; await settle(); await open();
    const newField = input('new-password'); await settle();
    unlock.resolve(true); await settle();
    expect(calls.password).toHaveBeenCalledWith('a', 1, 'old-password');
    expect(newField.value).toBe('new-password'); expect(close).not.toHaveBeenCalled();
  });
  it('ignores a memory scan response after the control is destroyed', async () => {
    const scan = deferred<BitlockerVmkScanResult>(); calls.open.mockResolvedValue('D:/memory.raw'); calls.scan.mockReturnValue(scan.promise);
    await mount(); await open(); await click('内存镜像'); await settle(); await click('选择内存镜像'); await settle(); await click('开始扫描');
    app.unmount(); scan.resolve({ partition_id: 1, candidates: [] }); await settle();
    expect(host.querySelector('.bitlocker-panel')).toBeNull(); expect(calls.cancelScan).not.toHaveBeenCalled();
  });
});
