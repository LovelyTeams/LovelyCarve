// Pure presentation model for the KDBX viewer: group and entry rows with their
// paths, search, field order, history differences, time text and the header
// and metadata rows.
import type { KdbxDatabase, KdbxEntry, KdbxField, KdbxGroup, KdbxHeader, KdbxKdf, KdbxTime } from '../../../lib/ipc/kdbx';
import { joinList, sourceText, type Translator } from '../../../i18n/translate';
import { formatBinaryBytes, formatEpochSeconds } from '../../../utils/format';

/** The fields KeePass shows in its own columns, in its order. */
export const STANDARD_FIELDS = ['Title', 'UserName', 'Password', 'URL', 'Notes'] as const;

export interface KdbxGroupRow {
  group: KdbxGroup;
  /** Position path ("" for the root, "0/2" …); stable for one report. */
  key: string;
  depth: number;
  /** Group names below the root. */
  path: string[];
  /** Entries in this group and every group under it. */
  entryCount: number;
  recycleBin: boolean;
  inRecycleBin: boolean;
}

export interface KdbxEntryRow {
  entry: KdbxEntry;
  /** Group key + entry position. */
  key: string;
  groupKey: string;
  path: string[];
  inRecycleBin: boolean;
}

function countEntries(group: KdbxGroup): number {
  return group.entries.length + group.groups.reduce((total, child) => total + countEntries(child), 0);
}

/** Every group, the root first, depth-first in document order. */
export function groupRows(root: KdbxGroup, recycleBin: string | null): KdbxGroupRow[] {
  const rows: KdbxGroupRow[] = [];
  const visit = (group: KdbxGroup, key: string, depth: number, path: string[], inRecycleBin: boolean) => {
    const recycle = recycleBin !== null && group.uuid === recycleBin;
    rows.push({ group, key, depth, path, entryCount: countEntries(group), recycleBin: recycle, inRecycleBin: inRecycleBin || recycle });
    group.groups.forEach((child, index) => {
      visit(child, key === '' ? String(index) : `${key}/${index}`, depth + 1, [...path, child.name], inRecycleBin || recycle);
    });
  };
  visit(root, '', 0, [], false);
  return rows;
}

/** Every current entry (no history versions) with its group path. */
export function entryRows(groups: readonly KdbxGroupRow[]): KdbxEntryRow[] {
  return groups.flatMap(row => row.group.entries.map((entry, index) => ({
    entry,
    key: `${row.key}#${index}`,
    groupKey: row.key,
    path: row.path,
    inRecycleBin: row.inRecycleBin,
  })));
}

/** Whether `row` is in the group `groupKey` or a group under it. */
export function isUnder(row: KdbxEntryRow, groupKey: string): boolean {
  return groupKey === '' || row.groupKey === groupKey || row.groupKey.startsWith(`${groupKey}/`);
}

export function fieldValue(entry: KdbxEntry, key: string): string | null {
  return entry.fields.find(field => field.key === key)?.value ?? null;
}

export function fieldLabel(key: string, tr: Translator = sourceText): string {
  switch (key) {
    case 'Title': return tr('标题');
    case 'UserName': return tr('用户名');
    case 'Password': return tr('密码');
    case 'URL': return tr('网址');
    case 'Notes': return tr('备注');
    default: return key;
  }
}

/** Standard fields first in KeePass's order, then the others as stored. */
export function orderedFields(entry: KdbxEntry): KdbxField[] {
  const standard = STANDARD_FIELDS.flatMap(key => entry.fields.filter(field => field.key === key));
  const custom = entry.fields.filter(field => !(STANDARD_FIELDS as readonly string[]).includes(field.key));
  return [...standard, ...custom];
}

/** Case-insensitive substring search over every field, tags and the group path. */
export function entryMatches(row: KdbxEntryRow, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  const haystacks = [
    ...row.entry.fields.map(field => field.value),
    ...row.entry.tags,
    row.path.join('/'),
    row.entry.uuid ?? '',
  ];
  return haystacks.some(text => text.toLowerCase().includes(needle));
}

/** Field keys whose value differs between two versions of an entry. */
export function changedFields(older: KdbxEntry, newer: KdbxEntry): string[] {
  const keys = new Set([...older.fields.map(field => field.key), ...newer.fields.map(field => field.key)]);
  const changed = [...keys].filter(key => fieldValue(older, key) !== fieldValue(newer, key));
  const attachments = (entry: KdbxEntry) => entry.attachments.map(attachment => `${attachment.name}:${attachment.sha256 ?? ''}`).join('\n');
  if (attachments(older) !== attachments(newer)) changed.push('__attachments');
  return changed;
}

/** A field key as a short label, attachments included. */
export function changedLabel(key: string, tr: Translator = sourceText): string {
  return key === '__attachments' ? tr('附件') : fieldLabel(key, tr);
}

export function isExpired(entry: KdbxEntry, nowSeconds: number): boolean {
  const expiry = entry.times.expiry;
  return entry.times.expires && expiry?.kind === 'utc' && expiry.seconds !== null && expiry.seconds < nowSeconds;
}

export interface KdbxTimeText {
  text: string;
  /** Local time for an instant, the stored text otherwise. */
  title: string;
  /** A KDBX 3 time without zone designator. */
  unzoned: boolean;
}

/** Instants in UTC with the local time as the tooltip; zone-less text as stored. */
export function kdbxTimeText(time: KdbxTime | null | undefined): KdbxTimeText {
  if (!time) return { text: '—', title: '', unzoned: false };
  if (time.kind === 'utc' && time.seconds !== null) {
    return {
      text: formatEpochSeconds(time.seconds, { timezone: 'utc', precision: 'seconds', suffix: true, zeroAsMissing: false }),
      title: formatEpochSeconds(time.seconds, { precision: 'seconds', zeroAsMissing: false }),
      unzoned: false,
    };
  }
  const text = time.text ?? '';
  return { text: time.kind === 'unzoned' ? text.replace('T', ' ') : text, title: text, unzoned: time.kind === 'unzoned' };
}

/** The masked form of a protected value; its length is not shown. */
export const MASKED_VALUE = '••••••••';

/** One label / value row (the shape `FileDetailFields` renders). */
export interface KdbxDetail {
  key: string;
  label: string;
  value: string | number | null;
  mono?: boolean;
  copyable?: boolean;
  warning?: boolean;
}

/** The key derivation parameters, one row each. */
export function kdfDetails(kdf: KdbxKdf, tr: Translator = sourceText): KdbxDetail[] {
  const rows: KdbxDetail[] = [{ key: 'kdf', label: tr('密钥派生'), value: kdf.name }];
  if (kdf.rounds !== null) rows.push({ key: 'rounds', label: tr('轮数'), value: kdf.rounds.toLocaleString(), mono: true });
  if (kdf.memory !== null) rows.push({ key: 'memory', label: tr('内存'), value: formatBinaryBytes(kdf.memory, 0), mono: true });
  if (kdf.iterations !== null) rows.push({ key: 'iterations', label: tr('迭代次数'), value: kdf.iterations.toLocaleString(), mono: true });
  if (kdf.parallelism !== null) rows.push({ key: 'parallelism', label: tr('并行度'), value: kdf.parallelism, mono: true });
  if (kdf.version !== null) {
    rows.push({ key: 'argonVersion', label: tr('算法版本'), value: `${kdf.version >> 4}.${kdf.version & 0xf}`, mono: true });
  }
  if (kdf.secret_key) rows.push({ key: 'secretKey', label: tr('密钥参数'), value: tr('有') });
  if (kdf.associated_data) rows.push({ key: 'associatedData', label: tr('关联数据'), value: tr('有') });
  rows.push({
    key: 'salt',
    label: kdf.rounds !== null ? tr('转换种子') : tr('盐'),
    value: kdf.salt,
    mono: true,
    copyable: true,
  });
  return rows;
}

function compressionLabel(compression: string, tr: Translator): string {
  return compression === 'none' ? tr('无') : compression;
}

/** The outer header, as far as it can be read without a key. */
export function headerDetails(header: KdbxHeader, tr: Translator = sourceText): KdbxDetail[] {
  const rows: KdbxDetail[] = [
    { key: 'version', label: tr('格式版本'), value: `KDBX ${header.version}`, mono: true },
    { key: 'cipher', label: tr('加密算法'), value: header.cipher },
    { key: 'cipherUuid', label: tr('算法 UUID'), value: header.cipher_uuid, mono: true, copyable: true },
    { key: 'compression', label: tr('压缩'), value: compressionLabel(header.compression, tr) },
    ...kdfDetails(header.kdf, tr),
    { key: 'masterSeed', label: tr('主种子'), value: header.master_seed, mono: true, copyable: true },
    { key: 'iv', label: tr('加密 IV'), value: header.encryption_iv, mono: true, copyable: true },
  ];
  if (header.inner_stream !== null) rows.push({ key: 'innerStream', label: tr('内部流'), value: header.inner_stream });
  if (header.protected_stream_key !== null) {
    rows.push({ key: 'streamKey', label: tr('内部流密钥'), value: header.protected_stream_key, mono: true, copyable: true });
  }
  if (header.stream_start_bytes !== null) {
    rows.push({ key: 'streamStart', label: tr('流起始字节'), value: header.stream_start_bytes, mono: true, copyable: true });
  }
  if (header.pre_release) rows.push({ key: 'preRelease', label: tr('签名'), value: tr('KeePass 2 预发布版') });
  if (header.comment) rows.push({ key: 'comment', label: tr('注释'), value: header.comment });
  rows.push(
    { key: 'length', label: tr('头部长度'), value: `${header.length.toLocaleString()} B`, mono: true },
    {
      key: 'sha256',
      label: tr('头部 SHA-256'),
      value: header.sha256,
      mono: true,
      copyable: true,
      warning: header.sha256_valid === false,
    },
  );
  return rows;
}

/** How the database was opened. */
export function keyDetails(database: KdbxDatabase, tr: Translator = sourceText): KdbxDetail[] {
  const { key } = database;
  const password = key.transformed_key ? '—'
    : key.password === 'set' ? tr('已输入')
      : key.password === 'empty' ? tr('空密码')
        : tr('无');
  return [
    { key: 'password', label: tr('主密码'), value: password },
    { key: 'keyfile', label: tr('密钥文件'), value: key.keyfile ?? tr('无') },
    { key: 'transformedKey', label: tr('转换密钥'), value: database.transformed_key, mono: true, copyable: true },
    { key: 'innerStream', label: tr('内部流'), value: database.inner_stream },
  ];
}

function timeValue(time: KdbxTime | null): string | null {
  const text = kdbxTimeText(time);
  return time ? text.text : null;
}

/** `Meta` rows; `groupName` names a group UUID. */
export function metaDetails(
  database: KdbxDatabase,
  groupName: (uuid: string | null) => string | null,
  tr: Translator = sourceText,
): KdbxDetail[] {
  const meta = database.meta;
  const days = (value: number | null) => (value === null ? null : value < 0 ? tr('从不') : tr('{0} 天', { 0: value }));
  const protectedFields = (['title', 'username', 'password', 'url', 'notes'] as const)
    .filter(field => meta.memory_protection[field])
    .map(field => fieldLabel({ title: 'Title', username: 'UserName', password: 'Password', url: 'URL', notes: 'Notes' }[field], tr));
  const rows: KdbxDetail[] = [
    { key: 'name', label: tr('数据库名称'), value: meta.database_name || null },
    { key: 'nameChanged', label: tr('名称修改时间'), value: timeValue(meta.database_name_changed) },
    { key: 'description', label: tr('描述'), value: meta.database_description || null },
    { key: 'defaultUser', label: tr('默认用户名'), value: meta.default_username || null },
    { key: 'generator', label: tr('生成程序'), value: meta.generator || null },
    { key: 'color', label: tr('颜色'), value: meta.color || null, mono: true },
    { key: 'masterKeyChanged', label: tr('主密钥修改时间'), value: timeValue(meta.master_key_changed) },
    { key: 'masterKeyRec', label: tr('建议更换主密钥'), value: days(meta.master_key_change_rec) },
    { key: 'masterKeyForce', label: tr('强制更换主密钥'), value: days(meta.master_key_change_force) },
    { key: 'settingsChanged', label: tr('设置修改时间'), value: timeValue(meta.settings_changed) },
    {
      key: 'recycleBin',
      label: tr('回收站'),
      value: meta.recycle_bin_enabled === false ? tr('已停用') : groupName(database.recycle_bin) ?? tr('无'),
    },
    { key: 'recycleBinChanged', label: tr('回收站修改时间'), value: timeValue(meta.recycle_bin_changed) },
    { key: 'templates', label: tr('模板群组'), value: groupName(meta.entry_templates_group) },
    {
      key: 'historyItems',
      label: tr('历史版本上限'),
      value: meta.history_max_items === null ? null : meta.history_max_items < 0 ? tr('不限') : meta.history_max_items.toLocaleString(),
    },
    {
      key: 'historySize',
      label: tr('历史大小上限'),
      value: meta.history_max_size === null ? null : meta.history_max_size < 0 ? tr('不限') : formatBinaryBytes(meta.history_max_size),
    },
    { key: 'maintenance', label: tr('维护天数'), value: days(meta.maintenance_history_days) },
    { key: 'memoryProtection', label: tr('内存保护'), value: protectedFields.length ? joinList(protectedFields, tr) : tr('无') },
    { key: 'lastGroup', label: tr('最后选中的群组'), value: groupName(meta.last_selected_group) },
  ];
  return rows.filter(row => row.value !== null && row.value !== '');
}
