// @vitest-environment happy-dom
import { createApp, h, nextTick, type App } from 'vue';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { KdbxDatabase, KdbxHeader, KdbxTimes } from '../../../../lib/ipc/kdbx';
import { IpcCommandError } from '../../../../lib/ipc/client';
import KdbxViewer from './KdbxViewer.vue';

const rpc = vi.hoisted(() => ({ header: vi.fn(), open: vi.fn(), attachment: vi.fn(), children: vi.fn() }));
vi.mock('../../../../lib/ipc/kdbx', () => ({ fsKdbxHeader: rpc.header, fsKdbxOpen: rpc.open, fsKdbxAttachment: rpc.attachment }));
vi.mock('../../../../lib/ipc/fs/query', () => ({ fsChildrenPage: rpc.children }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

const TIMES: KdbxTimes = {
  creation: { kind: 'utc', seconds: 1_600_000_000, text: null },
  last_modification: { kind: 'utc', seconds: 1_600_000_060, text: null },
  last_access: null, expiry: null, expires: false, usage_count: 2, location_changed: null,
};

const HEADER: KdbxHeader = {
  file_size: 2048, version: '4.0', pre_release: false, cipher: 'ChaCha20', cipher_uuid: 'd6038a2b8b6f4cb5a524339a31dbb59a',
  compression: 'GZip',
  kdf: { name: 'Argon2d', uuid: 'ef636ddf8c29444b91f7a9a403e30a0c', rounds: null, salt: '00', memory: 64 << 20, iterations: 2, parallelism: 2, version: 0x13, secret_key: false, associated_data: false },
  master_seed: '11', encryption_iv: '22', inner_stream: null, protected_stream_key: null, stream_start_bytes: null,
  comment: null, public_custom_data: [], length: 250, sha256: '33', sha256_valid: true, fields: [], warnings: [],
};

function database(): KdbxDatabase {
  const field = (key: string, value: string, isProtected = false) => ({ key, value, protected: isProtected, unreadable: null, resolved: null });
  const group = (name: string, uuid: string) => ({
    uuid, name, notes: null, icon_id: 48, custom_icon: null, times: TIMES, is_expanded: null, default_auto_type_sequence: null,
    enable_auto_type: null, enable_searching: null, last_top_visible_entry: null, previous_parent_group: null, tags: [], custom_data: [],
    entries: [], groups: [],
  });
  return {
    header: HEADER,
    key: { password: 'set', keyfile: null, transformed_key: false },
    transformed_key: 'ab'.repeat(32),
    header_hash: 'matches',
    inner_stream: 'ChaCha20',
    meta: {
      generator: 'KeePassXC', settings_changed: null, database_name: 'Case 7', database_name_changed: null, database_description: null,
      database_description_changed: null, default_username: null, default_username_changed: null, maintenance_history_days: 365,
      color: null, master_key_changed: null, master_key_change_rec: -1, master_key_change_force: -1, master_key_change_force_once: null,
      memory_protection: { title: false, username: false, password: true, url: false, notes: false }, custom_icons: [],
      recycle_bin_enabled: true, recycle_bin_uuid: null, recycle_bin_changed: null, entry_templates_group: null,
      entry_templates_group_changed: null, history_max_items: 10, history_max_size: 6 << 20, last_selected_group: null,
      last_top_visible_group: null, custom_data: [],
    },
    root: {
      ...group('Root', 'R'),
      entries: [{
        uuid: 'E'.repeat(32), icon_id: 0, custom_icon: null, foreground_color: null, background_color: null, override_url: null,
        quality_check: null, tags: ['bank'], previous_parent_group: null, times: TIMES,
        fields: [field('Title', 'Bank'), field('UserName', 'bob'), field('Password', 's3cret', true), field('URL', 'https://bank.example')],
        attachments: [], auto_type: null, custom_data: [], history: [],
      }],
      groups: [group('Email', 'M')],
    },
    recycle_bin: null,
    deleted_objects: [],
    binaries: [],
    counts: { groups: 2, entries: 1, history: 0, attachments: 0 },
    warnings: [],
  };
}

let app: App;
let host: HTMLDivElement;

async function settle() {
  for (let index = 0; index < 6; index += 1) await nextTick();
  await new Promise(resolve => setTimeout(resolve, 0));
  for (let index = 0; index < 4; index += 1) await nextTick();
}

async function mount() {
  app = createApp({ render: () => h(KdbxViewer, { imageId: 'evidence', nodeId: 'ntfs:42', parentId: 'ntfs:5' }) });
  app.mount(host);
  await settle();
}

function passwordInput(): HTMLInputElement {
  return host.querySelector<HTMLInputElement>('input[type="password"], input[type="text"].ui-input')!;
}

async function submit(password: string) {
  const input = passwordInput();
  input.value = password;
  input.dispatchEvent(new Event('input'));
  host.querySelector<HTMLFormElement>('form')!.dispatchEvent(new Event('submit', { cancelable: true }));
  await settle();
}

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  rpc.header.mockResolvedValue(HEADER);
  rpc.children.mockResolvedValue({ items: [
    { id: 'ntfs:43', name: 'vault.keyx', parent_id: 'ntfs:5', kind: 'file', size: 240, deleted: false, inode: null, timestamps: { modified: null, accessed: null, created: null, mft_changed: null, birth: null } },
  ], nextCursor: null, truncated: false });
});

afterEach(() => {
  app.unmount();
  host.remove();
  vi.clearAllMocks();
});

describe('KdbxViewer', () => {
  it('shows the header without a key and flags wrong credentials on the password field', async () => {
    rpc.open.mockRejectedValueOnce(new IpcCommandError({ code: 'invalidInput', message: '密码、密钥文件或转换密钥不正确', context: null, retryable: false }));
    await mount();
    expect(rpc.header).toHaveBeenCalledWith({ imageId: 'evidence', nodeId: 'ntfs:42' });
    expect(host.textContent).toContain('KDBX 4.0');
    expect(host.textContent).toContain('Argon2d');
    expect(rpc.children).toHaveBeenCalledWith({ imageId: 'evidence', parentId: 'ntfs:5', pageSize: 500 });

    await submit('wrong');
    expect(rpc.open).toHaveBeenCalledWith({ imageId: 'evidence', nodeId: 'ntfs:42', credentials: { password: 'wrong' } });
    expect(passwordInput().getAttribute('aria-invalid')).toBe('true');
    expect(host.querySelector('.ui-field-error')?.textContent).toContain('密码、密钥文件或转换密钥不正确');
  });

  it('opens the database, lists entries and keeps protected values masked until shown', async () => {
    rpc.open.mockResolvedValueOnce(database());
    await mount();
    await submit('s3cret');
    const rows = [...host.querySelectorAll<HTMLTableRowElement>('tbody tr')];
    expect(rows.map(row => row.textContent?.includes('Bank'))).toContain(true);
    expect(host.textContent).not.toContain('s3cret');

    rows.find(row => row.textContent?.includes('Bank'))!.click();
    await settle();
    expect(host.textContent).toContain('••••••••');
    expect(host.textContent).not.toContain('s3cret');
    host.querySelector<HTMLButtonElement>('button[aria-label="显示"]')!.click();
    await settle();
    expect(host.textContent).toContain('s3cret');

    // Locking forgets the database and returns to the form.
    host.querySelector<HTMLButtonElement>('button[aria-label="锁定"]')!.click();
    await settle();
    expect(host.querySelector('form')).not.toBeNull();
    expect(host.textContent).not.toContain('s3cret');
  });
});
