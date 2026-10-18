import { describe, expect, it } from 'vitest';
import type { EwfAcquiryInfo, ImageInfo } from '../../../lib/ipc/image';
import { translate, type Translator } from '../../../i18n/translate';
import { ACQUISITION_HIGHLIGHTS, acquisitionRows, evidenceDisplayName, evidenceFacts, evidenceHashVerdict, evidenceLayout, formatBytes, partColor, partitionGeometry, storedHashRows } from './evidenceOverview';

const info: ImageInfo = { id: 'a', path: 'a', source_format: 'raw', size_bytes: 1000, sector_size: 100, table_scheme: 'gpt', disk_id: null, on_disk_bytes: null, created_unix_ms: null, modified_unix_ms: null, ewf: null, partitions: [
  { id: 1, kind: 'ntfs', label: '末尾', start_lba: 8, length_lba: 8 },
  { id: 2, kind: 'ext4', label: null, start_lba: 0, length_lba: 2 },
  { id: 3, kind: 'unknown', label: null, start_lba: 20, length_lba: 2 },
] };

describe('evidence overview projections', () => {
  it('shows local acquisition dates without shifting them and keeps raw dates in the disclosure', () => {
    const evidence = { ...info, ewf: {
      acquiry_date: '2026 9 21 9 13 5',
      acquiry_date_parsed: { display: '2026-09-21 09:13:05', time_basis: 'local' },
      system_date: '1142163845',
      system_date_parsed: { display: '2006-03-12 11:44:05', time_basis: 'unix' },
    } as EwfAcquiryInfo };
    const rows = acquisitionRows(evidence);
    expect(rows.find(row => row.label === '采集时间')?.value).toBe('2026-09-21 09:13:05（采集机本地时间，时区未记录）');
    expect(rows.find(row => row.label === '系统时间')?.value).toBe('2006-03-12 11:44:05（Unix 时间，按 UTC 显示）');
    const details = rows.filter(row => !ACQUISITION_HIGHLIGHTS.has(row.acquisitionKey!));
    expect(details.find(row => row.label === '采集时间原始值')?.value).toBe('2026 9 21 9 13 5');
    expect(details.find(row => row.label === '系统时间原始值')?.value).toBe('1142163845');
    const english: Translator = (message, params) => translate(message, 'en', params);
    const highlights = acquisitionRows(evidence, english).filter(row => ACQUISITION_HIGHLIGHTS.has(row.acquisitionKey!));
    expect(highlights.find(row => row.label === 'Acquisition time')?.value).toBe('2026-09-21 09:13:05 (acquisition machine local time, timezone not recorded)');
  });
  it('falls back to unchanged raw dates for invalid values and older IPC payloads', () => {
    const evidence = { ...info, ewf: { acquiry_date: '  invalid date  ', system_date: '2026 9 21 9 13 5' } as EwfAcquiryInfo };
    expect(acquisitionRows(evidence).find(row => row.label === '采集时间')?.value).toBe('  invalid date  ');
    expect(acquisitionRows(evidence).find(row => row.label === '系统时间')?.value).toBe('2026 9 21 9 13 5');
    expect(acquisitionRows({ ...info, ewf: { acquiry_date: null, system_date: null } as EwfAcquiryInfo })).toEqual([]);
    expect(acquisitionRows(null)).toEqual([]);
  });
  it('clips the disk layout to image bytes while preserving the original partition colours', () => {
    expect(evidenceLayout(info)).toEqual([
      { key: 'part-2', leftPct: 0, widthPct: 20, lenBytes: 200, color: partColor(1), tooltip: '分区 2 · Ext4 · 200 B', partition: info.partitions[1] },
      { key: 'part-1', leftPct: 80, widthPct: 20, lenBytes: 200, color: partColor(0), tooltip: '末尾 · NTFS · 200 B', partition: info.partitions[0] },
    ]);
    expect(evidenceLayout({ ...info, size_bytes: 0 })).toEqual([]);
  });
  it('retains binary capacity precision and inclusive sector geometry', () => {
    expect(formatBytes(1024 * 1024)).toBe('1.00 MiB'); expect(formatBytes(1023)).toBe('1023 B');
    expect(partitionGeometry(info.partitions[0]!, 100, '扇区')).toBe('扇区 8 – 15，共 8 扇区\n字节偏移 0x320');
  });
  it('names an automatic reconstruction without exposing its identity as the heading', () => {
    const evidence = { ...info, source_format: 'assembled', path: 'assembled://md:auto-raid5-59494dc69af24768b6a690819d35420e' };
    expect(evidenceDisplayName(evidence)).toBe('RAID 5 重组镜像');
    expect(evidenceFacts(evidence)).toContainEqual({ label: '重组镜像标识', value: evidence.path, mono: true, copyable: true });
    expect(evidenceDisplayName({ ...info, path: 'D:\\evidence\\disk.E01' })).toBe('disk.E01');
    expect(evidenceDisplayName({ ...evidence, path: 'assembled://md:custom-array' })).toBe('RAID 重组镜像');
    expect(evidenceDisplayName({ ...evidence, path: 'assembled://lvm:Ab12-Cd34:vg1000/lv' })).toBe('逻辑卷 vg1000/lv');
  });
  it('normalizes recorded hashes and judges only algorithms with a computed result', () => {
    const evidence = { ...info, ewf: { stored_md5: ' AA BB ', stored_sha1: 'ccdd' } as NonNullable<ImageInfo['ewf']> };
    const rows = storedHashRows(evidence, { md5: 'aabb', sha1: null });
    expect(rows).toEqual([{ label: 'MD5', value: ' AA BB ', verdict: 'match' }, { label: 'SHA1', value: 'ccdd', verdict: 'pending' }]);
    expect(evidenceHashVerdict(rows)).toBe('match');
    expect(evidenceHashVerdict(storedHashRows(evidence, { md5: 'aabb', sha1: 'wrong' }))).toBe('mismatch');
    expect(evidenceHashVerdict(storedHashRows(evidence, undefined))).toBeNull();
  });
});
