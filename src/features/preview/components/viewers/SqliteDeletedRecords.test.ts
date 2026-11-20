// @vitest-environment happy-dom
import { createApp, h, nextTick, type App } from 'vue';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import SqliteDeletedRecords from './SqliteDeletedRecords.vue';
import type { SqliteRecoveredRecord, SqliteRecovery, SqliteTable } from '../../../../lib/ipc/sqlite';

const rpc = vi.hoisted(() => ({ deleted: vi.fn() }));
vi.mock('../../../../lib/ipc/sqlite', () => ({ fsSqliteDeletedRecords: rpc.deleted }));

let app: App;
let host: HTMLDivElement;
const counted = vi.fn();

const messages: SqliteTable = {
  name: 'messages',
  kind: 'table',
  columns: [
    { name: 'id', type_name: 'INTEGER', notnull: false, default_value: null, pk: 1 },
    { name: 'body', type_name: 'TEXT', notnull: true, default_value: null, pk: 0 },
  ],
  row_count: 2,
  sample_rows: [],
  error: null,
  create_sql: null,
};

function record(overrides: Partial<SqliteRecoveredRecord> = {}): SqliteRecoveredRecord {
  return {
    table: 'messages',
    link: 'known',
    origin: 'freelist_page',
    page: 7,
    offset: 0x1a0,
    rowid: 3,
    confidence: 0.92,
    wal_frame: null,
    overflow_pages: [],
    values: [{ kind: 'null' }, { kind: 'text', value: 'deleted hello' }],
    current_values: null,
    ...overrides,
  };
}

function recovery(overrides: Partial<SqliteRecovery> = {}): SqliteRecovery {
  return {
    page_size: 4096,
    page_count: 12,
    freelist_pages: 2,
    cipher: null,
    sidecars: [],
    notes: [],
    records: [],
    omitted: 0,
    schemas: [],
    ...overrides,
  };
}

async function settle() {
  for (let i = 0; i < 4; i += 1) await nextTick();
}

async function mount(props: Record<string, unknown> = {}) {
  app = createApp({
    render: () => h(SqliteDeletedRecords, {
      imageId: 'evidence',
      nodeId: 'ntfs:42',
      parentId: 'ntfs:5',
      sqliteKey: null,
      tables: [messages],
      onCounted: counted,
      ...props,
    }),
  });
  app.mount(host);
  await settle();
}

function bodyRows(): HTMLTableRowElement[] {
  return [...host.querySelectorAll<HTMLTableRowElement>('tbody tr')];
}

function valueColumns(): string[] {
  return [...host.querySelectorAll('thead th .col-text')].map((head) =>
    [...head.children].map((part) => part.textContent).join(' '));
}

async function openOptions(trigger: HTMLButtonElement): Promise<HTMLElement[]> {
  trigger.click();
  await settle();
  return [...document.querySelectorAll<HTMLElement>('[role=option]')];
}

async function choose(trigger: HTMLButtonElement, label: string) {
  (await openOptions(trigger)).find((option) => option.textContent === label)!.click();
  await settle();
}

beforeEach(() => {
  vi.clearAllMocks();
  host = document.createElement('div');
  document.body.append(host);
});
afterEach(() => {
  app.unmount();
  host.remove();
});

describe('sqlite deleted records', () => {
  it('scans with the directory and key and lists where each row came from', async () => {
    rpc.deleted.mockResolvedValue(recovery({
      sidecars: ['chat.db-wal'],
      omitted: 5,
      records: [
        record(),
        record({ table: null, link: 'unattributed', origin: 'wal_frame', wal_frame: 4, offset: null, rowid: null, confidence: null }),
        record({ link: 'inferred', origin: 'freeblock', overflow_pages: [9, 10] }),
      ],
    }));
    const key = { passphrase: 'evidence-key' };
    await mount({ sqliteKey: key });

    expect(rpc.deleted).toHaveBeenCalledWith({ imageId: 'evidence', nodeId: 'ntfs:42', parentId: 'ntfs:5', key });
    expect(counted).toHaveBeenCalledWith(8);
    expect(host.textContent).toContain('页大小 4096 · 12 页 · 空闲页 2 · 已结合 chat.db-wal');
    expect(host.textContent).toContain('另有 5 条未列出');

    const [carved, wal, rebuilt] = bodyRows();
    expect(carved.textContent).toContain('空闲页');
    expect(carved.textContent).toContain('页 7 · 0x1A0');
    expect(carved.textContent).toContain('92%');
    expect(wal.textContent).toContain('WAL 帧 4 · 页 7');
    expect(wal.textContent).toContain('未归属');
    expect(rebuilt.textContent).toContain('推断');
    expect(rebuilt.textContent).toContain('溢出 2 页');
    // Mixed tables: generic value columns.
    expect(valueColumns()).toEqual(['列 1', '列 2']);
  });

  it('filters by table with its column names, by origin, and clears an empty filter', async () => {
    rpc.deleted.mockResolvedValue(recovery({
      records: [
        record(),
        record({ table: 'contacts', origin: 'journal_deleted', values: [{ kind: 'integer', value: 1 }] }),
        record({ table: null, link: 'unattributed', origin: 'dropped_table' }),
      ],
    }));
    await mount();
    const [tableSelect, originSelect] = [...host.querySelectorAll<HTMLButtonElement>('button[role=combobox]')];
    const tableOptions = await openOptions(tableSelect);
    expect(tableOptions.map((option) => option.textContent)).toEqual(['全部表', 'contacts', 'messages', '未归属']);

    tableOptions.find((option) => option.textContent === 'messages')!.click();
    await settle();
    expect(bodyRows()).toHaveLength(1);
    expect(valueColumns()).toEqual(['id INTEGER', 'body TEXT']);

    await choose(originSelect, '日志 · 已删除');
    expect(host.textContent).toContain('没有符合筛选条件的记录');
    [...host.querySelectorAll('button')].find((button) => button.textContent?.includes('清除筛选'))!.click();
    await settle();
    expect(bodyRows()).toHaveLength(3);
  });

  it('marks the cells a journaled update changed', async () => {
    rpc.deleted.mockResolvedValue(recovery({
      records: [record({
        origin: 'journal_prior',
        values: [{ kind: 'integer', value: 3 }, { kind: 'text', value: 'before' }],
        current_values: [{ kind: 'integer', value: 3 }, { kind: 'text', value: 'after' }],
      })],
    }));
    await mount();
    const changed = [...host.querySelectorAll<HTMLTableCellElement>('td.cell-changed')];
    expect(changed.map((cell) => cell.textContent)).toEqual(['before']);
    expect(changed[0].title).toBe('before\n当前值：after');
  });

  it('shows notes, dropped schemas and the empty state', async () => {
    rpc.deleted.mockResolvedValue(recovery({
      notes: ['chat.db-wal 中 2 个页面无法用该密钥解密，未参与恢复'],
      schemas: [{ object_type: 'table', name: 'secrets', table_name: 'secrets', root_page: 4, sql: 'CREATE TABLE secrets(x)' }],
    }));
    await mount();
    expect(host.querySelector('.ui-alert.warn')?.textContent).toContain('未参与恢复');
    expect(host.querySelector('.schema-summary')?.textContent).toContain('已删除的表结构');
    expect(host.querySelector('.schema-sql')?.textContent).toBe('CREATE TABLE secrets(x)');
    expect(host.textContent).toContain('未发现已删除记录');
    expect(counted).toHaveBeenCalledWith(0);
  });

  it('reports a failed scan without the key prefix and retries', async () => {
    // Backend errors stringify to their message (IpcCommandError#toString).
    rpc.deleted
      .mockRejectedValueOnce('SQLITE_LOCKED: 密钥或兼容参数无法按页解密该数据库')
      .mockResolvedValueOnce(recovery({ records: [record()] }));
    await mount();
    const alert = host.querySelector('[role=alert]')!;
    expect(alert.textContent).toContain('扫描已删除记录失败：密钥或兼容参数无法按页解密该数据库');
    alert.querySelector('button')!.click();
    await settle();
    expect(rpc.deleted).toHaveBeenCalledTimes(2);
    expect(bodyRows()).toHaveLength(1);
  });
});
