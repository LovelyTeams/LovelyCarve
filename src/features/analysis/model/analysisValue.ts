import { joinList, sourceText, type Translator } from '../../../i18n/translate';
import type { RecordFieldKind, RecordSeverity } from '../../../lib/ipc/analysisResults';
import { formatBytes } from '../../../utils/format';

/** Seconds as `42 s` or `1:02:03`. */
export function durationText(seconds: number): string {
  if (seconds < 60) return `${Number(seconds.toFixed(3))} s`;
  const total = Math.floor(seconds);
  const parts = [Math.floor(total / 3600), Math.floor(total / 60) % 60, total % 60];
  return parts.map((part, index) => index === 0 ? String(part) : String(part).padStart(2, '0')).join(':');
}

const TIMESTAMP = /^(\d{4}-\d{2}-\d{2})[T ](\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?)(Z|[+-]\d{2}:?\d{2})?$/i;

/** An RFC 3339 time as `2024-01-02 03:04:05 UTC` or `… UTC+08:00`, keeping
 *  its precision. A time recorded without a zone stays without one, so it
 *  never reads as UTC; anything else is shown as stored. */
export function timestampText(value: string): string {
  const match = TIMESTAMP.exec(value.trim());
  if (!match) return value;
  const [, date, time, zone] = match;
  if (!zone) return `${date} ${time}`;
  if (zone.toUpperCase() === 'Z' || /^[+-]00:?00$/.test(zone)) return `${date} ${time} UTC`;
  return `${date} ${time} UTC${zone.includes(':') ? zone : `${zone.slice(0, 3)}:${zone.slice(3)}`}`;
}

/** A value the record does not carry (shown as a dash). */
export function isEmptyValue(value: unknown): boolean {
  return value === null || value === undefined || value === '' || (Array.isArray(value) && value.length === 0);
}

/** A record value as the table or detail shows it, by the field's kind.
 *  `choice` names an enum value; `detailed` adds exact byte counts. */
export function recordValueText(
  value: unknown,
  kind: RecordFieldKind | null,
  options: { choice?: (id: string) => string; detailed?: boolean; tr?: Translator } = {},
): string {
  const tr = options.tr ?? sourceText;
  if (isEmptyValue(value)) return '—';
  switch (kind) {
    case 'bool':
      return value === true ? tr("是") : value === false ? tr("否") : String(value);
    case 'size':
      if (typeof value !== 'number') return String(value);
      return options.detailed ? tr("{0}（{1} 字节）", { 0: formatBytes(value), 1: value.toLocaleString() }) : formatBytes(value);
    case 'duration':
      return typeof value === 'number' ? durationText(value) : String(value);
    case 'enum':
      return typeof value === 'string' && options.choice ? options.choice(value) : String(value);
    case 'list':
      return Array.isArray(value) ? joinList(value.map(item => typeof item === 'string' ? item : JSON.stringify(item)), tr) : String(value);
    case 'json':
      return typeof value === 'string' ? value : JSON.stringify(value);
    case 'timestamp':
      if (typeof value === 'string') return timestampText(value);
      return typeof value === 'number' ? String(value) : JSON.stringify(value);
    case 'text':
    case 'int':
    case 'float':
    case 'path':
    case 'hash':
    case 'ip':
    case null:
      return typeof value === 'string' ? value : typeof value === 'number' || typeof value === 'boolean' ? String(value) : JSON.stringify(value);
  }
}

/** Long cell text, cut for the table. */
export function truncateText(text: string, maxLength: number): string {
  return text.length > maxLength ? `${text.slice(0, maxLength - 1)}…` : text;
}

/** A record value in a CSV export: the stored value, not its display form. */
export function recordCsvValue(value: unknown): string | number | boolean {
  if (value === null || value === undefined) return '';
  if (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') return value;
  if (Array.isArray(value) && value.every(item => typeof item === 'string')) return value.join('; ');
  return JSON.stringify(value);
}

/** Badge tone of an analyzer's rating. */
export function severityTone(severity: RecordSeverity): 'danger' | 'warn' | 'info' {
  if (severity === 'critical' || severity === 'high') return 'danger';
  return severity === 'medium' ? 'warn' : 'info';
}

export function severityLabel(severity: RecordSeverity, tr: Translator = sourceText): string {
  switch (severity) {
    case 'critical': return tr("严重");
    case 'high': return tr("高");
    case 'medium': return tr("中");
    case 'low': return tr("低");
    case 'info': return tr("提示");
  }
}
