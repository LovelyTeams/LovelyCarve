import { describe, expect, it } from 'vitest';
import { durationText, isEmptyValue, recordValueText, timestampText } from './analysisValue';

describe('analysis record values', () => {
  it('reads RFC 3339 times with their zone and precision, and never labels a zoneless time UTC', () => {
    expect([
      '2023-11-02T08:41:17Z',
      '2026-10-19T13:30:32.854123400+00:00',
      '2024-04-11T00:00:00+08:00',
      '2024-04-11T00:00:00-0530',
      '2024-02-29T23:59:59',
      '2024-02-29 23:59',
      'yesterday',
    ].map(timestampText)).toEqual([
      '2023-11-02 08:41:17 UTC',
      '2026-10-19 13:30:32.854123400 UTC',
      '2024-04-11 00:00:00 UTC+08:00',
      '2024-04-11 00:00:00 UTC-05:30',
      '2024-02-29 23:59:59',
      '2024-02-29 23:59',
      'yesterday',
    ]);
  });

  it('formats a timestamp field through the same reading and keeps other kinds as declared', () => {
    expect(recordValueText('2024-01-02T03:04:05Z', 'timestamp')).toBe('2024-01-02 03:04:05 UTC');
    expect(recordValueText(1_704_164_645, 'timestamp')).toBe('1704164645');
    expect(recordValueText('2024-01-02T03:04:05Z', 'text')).toBe('2024-01-02T03:04:05Z');
    expect(recordValueText(2048, 'size', { detailed: true })).toBe('2.0 KB（2,048 字节）');
  });

  it('treats missing values alike and shows them as a dash', () => {
    expect([null, undefined, '', [], 0, false, 'x'].map(isEmptyValue)).toEqual([true, true, true, true, false, false, false]);
    expect(recordValueText([], 'list')).toBe('—');
    expect(recordValueText(false, 'bool')).toBe('否');
  });

  it('writes durations in seconds below a minute and as a clock above', () => {
    expect([11.742, 59.9999, 3723].map(durationText)).toEqual(['11.742 s', '60 s', '1:02:03']);
  });
});
