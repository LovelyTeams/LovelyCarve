import { describe, expect, it } from 'vitest';
import type { KdbxEntry, KdbxGroup, KdbxHeader, KdbxTimes } from '../../../lib/ipc/kdbx';
import {
  changedFields, entryMatches, entryRows, groupRows, headerDetails, isExpired, isUnder, kdbxTimeText, orderedFields,
} from './kdbx';

const NO_TIMES: KdbxTimes = {
  creation: null, last_modification: null, last_access: null, expiry: null, expires: false, usage_count: null, location_changed: null,
};

function entry(title: string, fields: Record<string, string> = {}, overrides: Partial<KdbxEntry> = {}): KdbxEntry {
  return {
    uuid: `${title.toUpperCase().padEnd(32, '0')}`,
    icon_id: 0,
    custom_icon: null,
    foreground_color: null,
    background_color: null,
    override_url: null,
    quality_check: null,
    tags: [],
    previous_parent_group: null,
    times: NO_TIMES,
    fields: Object.entries({ Title: title, ...fields }).map(([key, value]) => ({ key, value, protected: key === 'Password', unreadable: null, resolved: null })),
    attachments: [],
    auto_type: null,
    custom_data: [],
    history: [],
    ...overrides,
  };
}

function group(name: string, uuid: string, entries: KdbxEntry[] = [], groups: KdbxGroup[] = []): KdbxGroup {
  return {
    uuid, name, notes: null, icon_id: 48, custom_icon: null, times: NO_TIMES, is_expanded: null,
    default_auto_type_sequence: null, enable_auto_type: null, enable_searching: null, last_top_visible_entry: null,
    previous_parent_group: null, tags: [], custom_data: [], entries, groups,
  };
}

const root = group('Root', 'R', [entry('Bank', { UserName: 'bob', Password: 'p1', PIN: '1234' })], [
  group('Email', 'E', [entry('Mail', { UserName: 'alice' })], [group('Archive', 'A', [entry('Old')])]),
  group('Recycle Bin', 'B', [entry('Gone')]),
]);

describe('kdbx model', () => {
  it('lists groups depth-first with paths, counts and the recycle bin', () => {
    const rows = groupRows(root, 'B');
    expect(rows.map(row => [row.key, row.path.join('/'), row.depth, row.entryCount, row.recycleBin])).toEqual([
      ['', '', 0, 4, false],
      ['0', 'Email', 1, 2, false],
      ['0/0', 'Email/Archive', 2, 1, false],
      ['1', 'Recycle Bin', 1, 1, true],
    ]);
    const entries = entryRows(rows);
    expect(entries.map(row => [row.entry.fields[0].value, row.groupKey, row.inRecycleBin])).toEqual([
      ['Bank', '', false], ['Mail', '0', false], ['Old', '0/0', false], ['Gone', '1', true],
    ]);
    expect(entries.filter(row => isUnder(row, '0')).length).toBe(2);
    expect(entries.filter(row => isUnder(row, '')).length).toBe(4);
    // "1" is not a prefix group of "10".
    expect(isUnder({ ...entries[0], groupKey: '10' }, '1')).toBe(false);
  });

  it('searches every field, tags and the group path', () => {
    const entries = entryRows(groupRows(root, null));
    expect(entries.filter(row => entryMatches(row, 'ALICE')).map(row => row.entry.fields[0].value)).toEqual(['Mail']);
    expect(entries.filter(row => entryMatches(row, '1234')).length).toBe(1);
    expect(entries.filter(row => entryMatches(row, 'archive')).length).toBe(1);
    expect(entries.filter(row => entryMatches(row, '  ')).length).toBe(4);
  });

  it('orders standard fields first and finds what each version changed', () => {
    const current = entry('Bank', { PIN: '1', Notes: 'n', Password: 'new', UserName: 'bob' });
    expect(orderedFields(current).map(field => field.key)).toEqual(['Title', 'UserName', 'Password', 'Notes', 'PIN']);
    const older = entry('Bank', { PIN: '1', Notes: 'n', Password: 'old', UserName: 'bob' });
    expect(changedFields(older, current)).toEqual(['Password']);
    const withFile = { ...current, attachments: [{ name: 'a.txt', binary_id: 0, size: 3, sha256: 'ab', protected: false, unreadable: null }] };
    expect(changedFields(current, withFile)).toEqual(['__attachments']);
  });

  it('shows instants in UTC, keeps zone-less text and tells expiry', () => {
    expect(kdbxTimeText({ kind: 'utc', seconds: 1_528_628_641, text: null }).text).toBe('2018-06-10 11:04:01 UTC');
    expect(kdbxTimeText({ kind: 'unzoned', seconds: null, text: '2018-06-10T11:04:01' })).toEqual({
      text: '2018-06-10 11:04:01', title: '2018-06-10T11:04:01', unzoned: true,
    });
    expect(kdbxTimeText(null).text).toBe('—');
    const expiring = entry('X', {}, { times: { ...NO_TIMES, expires: true, expiry: { kind: 'utc', seconds: 100, text: null } } });
    expect(isExpired(expiring, 101)).toBe(true);
    expect(isExpired(expiring, 99)).toBe(false);
    expect(isExpired({ ...expiring, times: { ...expiring.times, expires: false } }, 101)).toBe(false);
  });

  it('describes a KDBX 3 header without a key', () => {
    const header: KdbxHeader = {
      file_size: 5838, version: '3.1', pre_release: false, cipher: 'AES-256', cipher_uuid: '31c1f2e6bf714350be5805216afc5aff',
      compression: 'none',
      kdf: { name: 'AES-KDF', uuid: 'c9d9f39a628a4460bf740d08c18a4fea', rounds: 60000, salt: 'aa', memory: null, iterations: null, parallelism: null, version: null, secret_key: false, associated_data: false },
      master_seed: 'bb', encryption_iv: 'cc', inner_stream: 'Salsa20', protected_stream_key: 'dd', stream_start_bytes: 'ee',
      comment: null, public_custom_data: [], length: 222, sha256: 'ff', sha256_valid: null, fields: [], warnings: [],
    };
    const rows = headerDetails(header);
    expect(rows.map(row => [row.label, row.value])).toEqual([
      ['格式版本', 'KDBX 3.1'],
      ['加密算法', 'AES-256'],
      ['算法 UUID', '31c1f2e6bf714350be5805216afc5aff'],
      ['压缩', '无'],
      ['密钥派生', 'AES-KDF'],
      ['轮数', (60000).toLocaleString()],
      ['转换种子', 'aa'],
      ['主种子', 'bb'],
      ['加密 IV', 'cc'],
      ['内部流', 'Salsa20'],
      ['内部流密钥', 'dd'],
      ['流起始字节', 'ee'],
      ['头部长度', '222 B'],
      ['头部 SHA-256', 'ff'],
    ]);
  });
});
