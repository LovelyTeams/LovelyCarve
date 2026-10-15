import { sourceText, type Translator } from '../../../i18n/translate';
/**
 * BitLocker recovery passwords are 48 digits in eight dash-separated groups of
 * six. Every group is divisible by 11 and its quotient fits in 16 bits — the
 * rules the vendored decryptor (`recovery_key_hash`) enforces — so a typo can
 * be pointed out in the field instead of after an unlock round trip.
 */

const GROUP_COUNT = 8;
const GROUP_DIGITS = 6;
/** 65535 × 11: the largest group whose quotient still fits in 16 bits. */
const MAX_GROUP_VALUE = 720_885;

export interface RecoveryKeyCheck {
  /** Canonical `xxxxxx-…-xxxxxx` form to send to the backend. */
  normalized: string;
  /** Complete groups that pass the checksum. */
  validGroups: number;
  /** A finished group that can never be right (letters, checksum, range,
   *  extra group): worth showing while the user is still typing. */
  groupError: string | null;
  /** What is still missing (a short group, too few groups): show once the
   *  user submits or leaves the field. */
  formatError: string | null;
}

/** Pasted keys often arrive with spaces, en dashes or no separators at all.
 *  A run of digits without any separator is grouped six at a time. */
export function normalizeRecoveryKey(raw: string): string {
  const joined = raw.trim().replace(/[^0-9A-Za-z]+/g, '-').replace(/^-+|-+$/g, '');
  return /^\d+$/.test(joined) ? joined.replace(/(\d{6})(?=\d)/g, '$1-') : joined;
}

export function checkRecoveryKey(raw: string, tr: Translator = sourceText): RecoveryKeyCheck {
  const normalized = normalizeRecoveryKey(raw);
  const groups = normalized ? normalized.split('-') : [];
  let validGroups = 0;
  let groupError: string | null = null;
  let formatError: string | null = null;
  for (const [index, group] of groups.entries()) {
    const position = index + 1;
    if (!/^\d+$/.test(group)) {
      groupError = tr("只能输入数字，第 {0} 组含有其他字符", { 0: position });
      break;
    }
    if (index >= GROUP_COUNT) {
      groupError = tr("恢复密钥只有 {0} 组，请删除多出的第 {1} 组", { 0: GROUP_COUNT, 1: position });
      break;
    }
    if (group.length > GROUP_DIGITS) {
      groupError = tr("第 {0} 组超过 {1} 位，请检查分隔", { 0: position, 1: GROUP_DIGITS });
      break;
    }
    if (group.length < GROUP_DIGITS) {
      formatError ??= tr("第 {0} 组只有 {1} 位，应为 {2} 位", { 0: position, 1: group.length, 2: GROUP_DIGITS });
      continue;
    }
    const value = Number(group);
    if (value % 11 !== 0) {
      groupError = tr("第 {0} 组「{1}」校验失败（应能被 11 整除），请检查是否输错", { 0: position, 1: group });
      break;
    }
    if (value > MAX_GROUP_VALUE) {
      groupError = tr("第 {0} 组「{1}」超出有效范围（不大于 {2}）", { 0: position, 1: group, 2: MAX_GROUP_VALUE });
      break;
    }
    validGroups += 1;
  }
  if (groupError === null && formatError === null && groups.length < GROUP_COUNT) {
    formatError = tr("还差 {0} 组（共 {1} 组 {2} 位数字）", { 0: GROUP_COUNT - groups.length, 1: GROUP_COUNT, 2: GROUP_DIGITS });
  }
  return { normalized, validGroups, groupError, formatError };
}
