export function truncateText(value: string, maxLength: number): string {
  return value.length > maxLength ? `${value.slice(0, maxLength)}...` : value;
}

export function formatBytes(value: number, fractionDigits = 1): string {
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(fractionDigits)} KB`;
  if (value < 1024 * 1024 * 1024) return `${(value / (1024 * 1024)).toFixed(fractionDigits)} MB`;
  return `${(value / (1024 * 1024 * 1024)).toFixed(fractionDigits)} GB`;
}

/** Binary capacity for evidence geometry; IEC units match the 1024 divisor. */
export function formatBinaryBytes(value: number, fractionDigits = 2): string {
  if (value < 1024) return `${value} B`;
  const units = ['KiB', 'MiB', 'GiB', 'TiB', 'PiB'];
  let current = value / 1024;
  let index = 0;
  while (current >= 1024 && index < units.length - 1) {
    current /= 1024;
    index += 1;
  }
  return `${current.toFixed(fractionDigits)} ${units[index]}`;
}

export type TimestampTimezone = 'local' | 'utc' | 'utc8';

export interface FormatEpochSecondsOptions {
  timezone?: TimestampTimezone;
  precision?: 'seconds' | 'milliseconds';
  fallback?: string;
  zeroAsMissing?: boolean;
  suffix?: boolean;
}

export interface FormatCaptureTimestampOptions extends FormatEpochSecondsOptions {
  epochThreshold?: number;
  relativePrecision?: number;
}

export function formatEpochSeconds(
  value: number | null | undefined,
  options: FormatEpochSecondsOptions = {},
): string {
  const {
    timezone = 'local',
    precision = 'milliseconds',
    fallback = '-',
    zeroAsMissing = true,
    suffix = false,
  } = options;

  if (value == null || !Number.isFinite(value) || (zeroAsMissing && value === 0)) {
    return fallback;
  }

  const date = new Date(Math.round(value * 1000));
  if (Number.isNaN(date.getTime())) return fallback;

  if (timezone === 'utc') {
    return `${formatDateParts(date, true, precision)}${suffix ? ' UTC' : ''}`;
  }

  if (timezone === 'utc8') {
    const shifted = new Date(date.getTime() + 8 * 60 * 60 * 1000);
    return `${formatDateParts(shifted, true, precision)}${suffix ? ' +08' : ''}`;
  }

  return formatDateParts(date, false, precision);
}

export function formatCaptureTimestamp(
  value: number | null | undefined,
  options: FormatCaptureTimestampOptions = {},
): string {
  const {
    epochThreshold = 946684800,
    relativePrecision = 6,
    fallback = '-',
    ...epochOptions
  } = options;

  if (value == null || !Number.isFinite(value)) return fallback;
  if (value > epochThreshold) return formatEpochSeconds(value, { fallback, ...epochOptions });
  return `${value.toFixed(relativePrecision)}s`;
}

function formatDateParts(
  date: Date,
  utc: boolean,
  precision: 'seconds' | 'milliseconds',
): string {
  const pad = (n: number, width = 2) => String(n).padStart(width, '0');
  const year = utc ? date.getUTCFullYear() : date.getFullYear();
  const month = (utc ? date.getUTCMonth() : date.getMonth()) + 1;
  const day = utc ? date.getUTCDate() : date.getDate();
  const hours = utc ? date.getUTCHours() : date.getHours();
  const minutes = utc ? date.getUTCMinutes() : date.getMinutes();
  const seconds = utc ? date.getUTCSeconds() : date.getSeconds();
  const base = `${year}-${pad(month)}-${pad(day)} ${pad(hours)}:${pad(minutes)}:${pad(seconds)}`;

  if (precision === 'seconds') return base;

  const millis = utc ? date.getUTCMilliseconds() : date.getMilliseconds();
  return `${base}.${pad(millis, 3)}`;
}
