// @vitest-environment happy-dom
import { createApp, h, nextTick, ref, type App } from 'vue';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import EncryptedVolumeUnlockDialog from './EncryptedVolumeUnlockDialog.vue';
import type { DiskPartition, ImageInfo } from '../../../lib/ipc/image';

const rpc = vi.hoisted(() => ({ start: vi.fn(), dismiss: vi.fn(), pick: vi.fn() }));
vi.mock('../composables/useEncryptedVolumeUnlocks', () => ({ startEncryptedVolumeUnlock: rpc.start }));
vi.mock('../composables/useFileSession', () => ({ dismissUnlockReminder: rpc.dismiss }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: rpc.pick }));

function partition(id: number, kind: string, label: string | null, start_lba: number): DiskPartition {
  return { id, kind, label, start_lba, length_lba: 2048 } as DiskPartition;
}
function image(partitions: DiskPartition[], size_bytes = 8192 * 512): ImageInfo {
  return { id: 'source', path: 'disk.dd', source_format: 'raw', size_bytes, sector_size: 512, table_scheme: 'gpt', partitions,
    disk_id: null, on_disk_bytes: null, modified_unix_ms: null, created_unix_ms: null, ewf: null } as ImageInfo;
}
const info = image([partition(1, 'ntfs', 'System', 2048), partition(2, 'luks', 'Linux', 4096), partition(3, 'unknown', null, 6144)]);
const VALID_RECOVERY = Array(8).fill('111111').join('-');
let app: App;
const source = ref(info);
const open = ref(true);
const partitionId = ref<number | null>(null);
const closed = vi.fn(() => { open.value = false; });

/** The newest dialog: one that just closed may still be leaving. */
function dialog(): HTMLElement {
  return [...document.querySelectorAll<HTMLElement>('[role=dialog]')].pop()!;
}
async function settle() {
  for (let i = 0; i < 4; i += 1) await nextTick();
}
async function value(selector: string, text: string) {
  const element = dialog().querySelector<HTMLInputElement>(selector)!;
  element.value = text;
  element.dispatchEvent(new Event('input', { bubbles: true }));
  await nextTick();
}
function comboboxes() {
  return dialog().querySelectorAll<HTMLButtonElement>('button[role=combobox]');
}
function selected(field: number): string | undefined {
  // The trigger also holds every other label (hidden) to keep its width.
  return comboboxes()[field]!.querySelector('.ui-select-value > :not(.ui-select-sizer)')?.textContent?.trim();
}
async function options(field: number): Promise<string[]> {
  comboboxes()[field]!.click();
  await settle();
  const labels = [...document.querySelectorAll<HTMLElement>('[role=option]')].map(option => option.textContent ?? '');
  comboboxes()[field]!.click();
  await settle();
  return labels;
}
async function choose(field: number, label: string) {
  comboboxes()[field]!.click();
  await settle();
  [...document.querySelectorAll<HTMLElement>('[role=option]')].find(option => option.textContent === label)!.click();
  await settle();
}
async function credentialTab(label: string) {
  [...dialog().querySelectorAll<HTMLButtonElement>('.ui-seg-item')].find(item => item.textContent === label)!.click();
  await settle();
}
function submitButton() {
  return dialog().querySelector<HTMLButtonElement>('button[type=submit]')!;
}
async function submit() {
  dialog().querySelector('form')!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
  await settle();
}
async function reopen(next: { info?: ImageInfo; partition?: number | null }) {
  open.value = false;
  await settle();
  if (next.info) source.value = next.info;
  if (next.partition !== undefined) partitionId.value = next.partition;
  open.value = true;
  await settle();
}
beforeEach(async () => {
  vi.clearAllMocks();
  rpc.start.mockReturnValue(true);
  source.value = info;
  open.value = true;
  partitionId.value = null;
  app = createApp({
    render: () => h(EncryptedVolumeUnlockDialog, {
      open: open.value, info: source.value, partitionId: partitionId.value, onClose: closed,
    }),
  });
  app.mount(document.createElement('div'));
  await settle();
});
afterEach(() => {
  app.unmount();
  // A dialog that was closing stays in its leave transition; do not let the
  // next test find it.
  document.body.innerHTML = '';
});

describe('encrypted volume unlock dialog', () => {
  it('normalizes a LUKS master key, submits only that credential, and clears it on close', async () => {
    expect(rpc.dismiss).toHaveBeenCalledWith('source');
    await choose(1, '整个镜像 / 容器');
    await credentialTab('主密钥');
    await value('input[type=password]', '00 11 aa FF');
    await submit();
    expect(rpc.start).toHaveBeenCalledWith(info, { scheme: 'luks', credential: { kind: 'masterKey', keyHex: '0011aaFF' } });
    await reopen({});
    expect(dialog().querySelector<HTMLInputElement>('input[type=password]')!.value).toBe('');
  });

  it('marks malformed master keys and clears them when switching to VeraCrypt', async () => {
    await credentialTab('主密钥');
    await value('input[type=password]', '001x');
    const input = dialog().querySelector<HTMLInputElement>('input[type=password]')!;
    expect(input.getAttribute('aria-invalid')).toBe('true');
    expect(dialog().querySelector('.ui-field-error')?.id).toBe(input.getAttribute('aria-describedby'));
    expect(submitButton().disabled).toBe(true);
    await submit();
    expect(rpc.start).not.toHaveBeenCalled();
    await choose(0, 'VeraCrypt');
    expect(dialog().querySelector('.ui-seg')).toBeNull();
    expect(dialog().querySelector<HTMLInputElement>('input[type=password]')!.value).toBe('');
  });

  it('offers VeraCrypt only undetected whole-sector regions and forwards its options', async () => {
    let sent: unknown;
    rpc.start.mockImplementation((image, unlockOptions) => { sent = structuredClone({ imageId: image.id, options: unlockOptions }); return true; });
    await choose(0, 'VeraCrypt');
    // The LUKS header of partition 2 names its own scheme.
    expect(await options(1)).toEqual(['整个镜像 / 容器', '分区 1 · System', '分区 3 · unknown']);
    await choose(1, '分区 3 · unknown');
    await value('input[type=password]', 'secret');
    await value('input[type=number]', '1');
    await choose(2, '隐藏卷');
    await choose(3, 'SHA-512');
    await submit();
    expect(sent).toEqual({ imageId: 'source', options: { scheme: 'veracrypt', credential: { kind: 'password', password: 'secret' }, partitionId: 3, pim: 1, header: 'hidden', prf: 'sha512' } });
    expect(closed).toHaveBeenCalled();
    await reopen({});
    expect(dialog().querySelector<HTMLInputElement>('input[type=password]')!.value).toBe('');
  });

  it('opens a VeraCrypt volume with keyfiles alone and forgets them on close', async () => {
    rpc.pick.mockResolvedValueOnce(['C:\\keys\\photo.jpg', 'C:\\keys\\notes.txt']).mockResolvedValueOnce('C:\\keys\\photo.jpg');
    await choose(0, 'VeraCrypt');
    await choose(1, '整个镜像 / 容器');
    expect(submitButton().disabled).toBe(true);
    const [addFiles] = [...dialog().querySelectorAll<HTMLButtonElement>('.keyfiles button')];
    addFiles!.click();
    await settle();
    addFiles!.click();
    await settle();
    expect([...dialog().querySelectorAll('.keyfile-list .key-path')].map(item => item.textContent)).toEqual(['C:\\keys\\photo.jpg', 'C:\\keys\\notes.txt']);
    dialog().querySelector<HTMLButtonElement>('.keyfile-list button')!.click();
    await settle();
    await submit();
    expect(rpc.start).toHaveBeenCalledWith(info, { scheme: 'veracrypt', credential: { kind: 'password', password: '' }, pim: 0, keyfiles: ['C:\\keys\\notes.txt'] });
    await reopen({});
    await choose(0, 'VeraCrypt');
    expect(dialog().querySelector('.keyfile-list')).toBeNull();
  });

  it('reads a system-encrypted drive as new evidence and keeps its PIM in range', async () => {
    await choose(0, 'VeraCrypt');
    await choose(1, '整个镜像 / 容器');
    await choose(2, '系统加密');
    expect(await options(3)).not.toContain('Argon2id');
    await value('input[type=password]', 'secret');
    await value('input[type=number]', '70000');
    const pim = dialog().querySelector<HTMLInputElement>('input[type=number]')!;
    expect(pim.getAttribute('aria-invalid')).toBe('true');
    expect(submitButton().disabled).toBe(true);
    await value('input[type=number]', '5');
    await submit();
    expect(rpc.start).toHaveBeenCalledWith(info, { scheme: 'veracrypt', credential: { kind: 'password', password: 'secret' }, pim: 5, header: 'system', target: 'newImage' });
  });

  it('does not offer VeraCrypt for a container whose size is not a whole number of sectors', async () => {
    await reopen({ info: image([], 196_708) });
    expect(await options(0)).toEqual(['LUKS1 / LUKS2', 'FileVault 2 / CoreStorage']);
    await reopen({ info: image([], 196_608) });
    expect(await options(0)).toEqual(['LUKS1 / LUKS2', 'VeraCrypt', 'FileVault 2 / CoreStorage']);
  });

  it('keeps the dialog open when an existing background job rejects submission and clears credentials', async () => {
    rpc.start.mockReturnValue(false);
    await value('input[type=password]', 'wrong');
    await submit();
    expect(closed).not.toHaveBeenCalled();
    expect(dialog().querySelector<HTMLInputElement>('input[type=password]')!.value).toBe('');
  });

  it('starts from the LUKS partition it was opened for and forgets the password on close', async () => {
    await reopen({ partition: 2 });
    expect(selected(0)).toBe('LUKS1 / LUKS2');
    expect(comboboxes()).toHaveLength(1);
    expect(dialog().querySelector('.ui-modal-title')?.textContent).toBe('解锁分区 2');
    await value('input[type=password]', 'typed');

    dialog().dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    await settle();
    expect(closed).toHaveBeenCalled();
    open.value = true;
    await settle();
    expect(dialog().querySelector<HTMLInputElement>('input[type=password]')!.value).toBe('');
  });

  it('submits the fixed partition without exposing a region chooser', async () => {
    await reopen({ partition: 2 });
    await value('input[type=password]', 'secret');
    await submit();
    expect(rpc.start).toHaveBeenCalledWith(info, { scheme: 'luks', credential: { kind: 'password', password: 'secret' }, partitionId: 2 });
  });

  it('submits LUKS for the whole image and leaves cancellation to the task center', async () => {
    expect(selected(1)).toBe('分区 2 · Linux');
    await choose(1, '整个镜像 / 容器');
    await value('input[type=password]', 'secret');
    await submit();
    expect(rpc.start).toHaveBeenCalledWith(info, { scheme: 'luks', credential: { kind: 'password', password: 'secret' } });
    expect(closed).toHaveBeenCalledOnce();
  });

  it('unlocks BitLocker in the same form and routes the credential to the chosen partition', async () => {
    const bitlocker = image([partition(1, 'ntfs', 'System', 2048), partition(3, 'bitlocker', 'Data', 4096),
      partition(4, 'bitlocker', 'Backup', 6144), partition(2, 'luks', 'Linux', 8192)], 16384 * 512);
    await reopen({ info: bitlocker });
    expect(selected(0)).toBe('BitLocker');
    expect(selected(1)).toBe('分区 3 · Data');
    expect([...dialog().querySelectorAll('.ui-seg-item')].map(item => item.textContent)).toEqual(['密码', '恢复密钥', '内存镜像', '其他密钥']);
    await choose(0, 'VeraCrypt');
    expect(selected(1)).toBe('整个镜像 / 容器');
    await choose(0, 'BitLocker');
    expect(selected(1)).toBe('分区 3 · Data');
    await choose(1, '分区 4 · Backup');

    await value('input[type=password]', 'secret');
    dialog().querySelector('input[type=password]')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, isComposing: true }));
    expect(closed).not.toHaveBeenCalled();
    await submit();
    expect(rpc.start).toHaveBeenCalledWith(bitlocker, { scheme: 'bitlocker', credential: { kind: 'password', password: 'secret' }, partitionId: 4 });
    expect(closed).toHaveBeenCalledOnce();
  });

  it('checks a BitLocker recovery password before it can be submitted', async () => {
    await reopen({ info: image([partition(3, 'bitlocker', 'Data', 2048)]), partition: 3 });
    await credentialTab('恢复密钥');
    await value('input[type=text]', '123456-' + VALID_RECOVERY.slice(7));
    expect(dialog().querySelector('.ui-field-error')?.textContent).toContain('校验失败');
    expect(submitButton().disabled).toBe(true);
    await value('input[type=text]', VALID_RECOVERY.replaceAll('-', ' '));
    await submit();
    expect(rpc.start).toHaveBeenCalledWith(source.value, { scheme: 'bitlocker', credential: { kind: 'recoveryPassword', recoveryPassword: VALID_RECOVERY }, partitionId: 3 });
  });

  it('sends BitLocker other keys through the same submission', async () => {
    await reopen({ info: image([partition(3, 'bitlocker', 'Data', 2048)]), partition: 3 });
    await credentialTab('其他密钥');
    // A startup key needs its file first.
    expect(submitButton().disabled).toBe(true);
    await choose(1, '明文密钥');
    await submit();
    expect(rpc.start).toHaveBeenCalledWith(source.value, { scheme: 'bitlocker', credential: { kind: 'clearKey' }, partitionId: 3 });
    await reopen({});
    await credentialTab('其他密钥');
    await choose(1, 'FVEK / TWEAK');
    const [key, tweak] = dialog().querySelectorAll<HTMLInputElement>('.key-fields input');
    key!.value = '01'.repeat(32); key!.dispatchEvent(new Event('input', { bubbles: true }));
    tweak!.value = '41'.repeat(32); tweak!.dispatchEvent(new Event('input', { bubbles: true }));
    await settle();
    await submit();
    expect(rpc.start).toHaveBeenLastCalledWith(source.value, { scheme: 'bitlocker', credential: { kind: 'masterKey', keyHex: '01'.repeat(32), tweakHex: '41'.repeat(32) }, partitionId: 3 });
  });

  it('says so when nothing is left to unlock', async () => {
    await reopen({ info: { ...image([partition(0, 'fat12', null, 0)], 2048 * 512), unlocked_partitions: [{ id: 0, scheme: 'veracrypt' }] } });
    expect(dialog().querySelector('.ui-alert')?.textContent).toContain('没有可解锁的加密区域');
    expect(submitButton().disabled).toBe(true);
  });
});
