import { describe, expect, it } from 'vitest';
import { translate } from '../../../i18n/translate';
import type { GlobalSearchHit } from '../../../lib/ipc/search';
import {
  buildSearchContextPreview,
  formatHitOffset,
  formatSearchFileSize,
  highlightSearchPattern,
  parseSearchPatterns,
  sourceKindLabel,
  taskDisplayTitle,
} from './searchResults';

const hit: GlobalSearchHit = {
  hit_seq: 3,
  node_id: 'ntfs:part:0:1234',
  volume_key: 'part:0',
  file_path: 'part:0:/Users/Alice/accounts.txt',
  file_name: 'accounts.txt',
  file_size: 2048,
  file_offset: 0x2a1,
  match_len: 8,
  encoding: 'UTF-8',
  pattern: 'password',
  pattern_kind: 'keyword',
  match_count: 2,
  context_preview: 'Password=password',
  context_full: '',
  md5: '',
  sha256: '',
  source_kind: 'file',
};

describe('global search presentation helpers', () => {
  it('formats offsets as uppercase hex with the h suffix', () => {
    expect(formatHitOffset(hit)).toBe('2A1h');
    expect(formatHitOffset({ ...hit, file_offset: 0 })).toBe('0h');
    expect(formatHitOffset({ ...hit, file_offset: 123456789 })).toBe('75BCD15h');
  });

  it('formats file sizes across units', () => {
    expect(formatSearchFileSize(512)).toBe('512 B');
    expect(formatSearchFileSize(2048)).toBe('2.0 KB');
    expect(formatSearchFileSize(5 * 1024 * 1024)).toBe('5.0 MB');
    expect(formatSearchFileSize(3 * 1024 * 1024 * 1024)).toBe('3.0 GB');
  });

  it('labels source kinds', () => {
    expect(sourceKindLabel('file')).toBe('文件');
    expect(sourceKindLabel('unallocated')).toBe('未分配');
    expect(sourceKindLabel('diskgap')).toBe('分区间隙');
  });

  it('builds task titles with keywords first and regex tail', () => {
    expect(taskDisplayTitle(['password'], [])).toBe('password');
    expect(taskDisplayTitle(['a', 'b'], ['\\d{4}'])).toBe('a、b + /\\d{4}');
    expect(taskDisplayTitle([], ['\\d+', '[x'])).toBe('/\\d+ 等 2 条');
    expect(taskDisplayTitle(['a', 'b'], ['\\d+', '[x'], (message, params) => translate(message, 'en', params))).toBe('a, b + /\\d+ and 2 more');
  });

  it('parses newline and comma separated patterns while preserving order', () => {
    expect(parseSearchPatterns('password\n token，secret, password')).toEqual([
      'password', 'token', 'secret',
    ]);
  });

  it('centers a long context excerpt around the matching pattern', () => {
    const preview = buildSearchContextPreview(
      `${'a'.repeat(80)}password${'b'.repeat(80)}`,
      'password',
      40,
    );
    expect(preview).toContain('password');
    expect(preview.startsWith('…')).toBe(true);
    expect(preview.endsWith('…')).toBe(true);
    expect(buildSearchContextPreview('short password', 'password', 40)).toBe('short password');
  });

  it('highlights every pattern match without changing its original casing', () => {
    expect(highlightSearchPattern('Password=password', 'password')).toEqual([
      { text: 'Password', matched: true },
      { text: '=', matched: false },
      { text: 'password', matched: true },
    ]);
  });
});
