import { sourceText, type Translator } from '../../../i18n/translate';

/** 「时间」着色的分档：距卷内最新写入多久。 */
export const AGE_BUCKETS: readonly number[] = [7, 30, 182, 365, Number.POSITIVE_INFINITY];

export function ageBucketLabel(bucket: number, tr: Translator = sourceText): string {
  switch (bucket) {
    case 0: return tr('一周内');
    case 1: return tr('一月内');
    case 2: return tr('半年内');
    case 3: return tr('一年内');
    default: return tr('更早');
  }
}

/** `newest`（Unix 秒）落在哪一档，以卷内最新写入 `reference` 为「现在」；任一未知时为 null。 */
export function ageBucketOf(newest: number | null, reference: number | null): number | null {
  if (newest === null || reference === null) return null;
  const days = (reference - newest) / 86_400;
  const bucket = AGE_BUCKETS.findIndex(limit => days <= limit);
  return bucket < 0 ? AGE_BUCKETS.length - 1 : bucket;
}

/** Unix 秒 → 与列表一致的 `YYYY-MM-DD HH:MM:SS`（UTC）。 */
export function formatUnixSeconds(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds)) return '—';
  const date = new Date(seconds * 1000);
  if (Number.isNaN(date.getTime())) return '—';
  return date.toISOString().replace('T', ' ').slice(0, 19);
}
