import { describe, expect, it } from 'vitest';
import type { RaidArray, RaidArrayMember, RaidStack } from '../../../lib/ipc/raid';
import {
  activeMembers, arrayStatus, diskLabelsFor, exportBaseName, layoutLabel, levelShortLabel, missingSlotsFor,
  orderedMembers, relevantRaidArrays, relevantRaidVolumeGroups, slotlessMembers,
} from './stackView';

const member: RaidArrayMember = { dev_number: 0, slot: 0, device_uuid: 'member-a', data_size_sectors: 1000, reconstructed: false, state: 'active', events: 12, origin: { kind: 'whole', image_id: 'a', partition_id: 1, byte_offset: 0, byte_length: 512000 } };
const array: RaidArray = { set_uuid: 'array-a', set_name: 'array/a', level: 5, layout: 0, chunksize_sectors: 128, raid_disks: 3, data_offset_sectors: 0, assembled_bytes: 1024000, members: [member], assembled_image_id: null, heuristic: false, metadata_version: '1.2', degraded: false, degradable: false };
const stack: RaidStack = { images: [], volume_groups: [{ vg_uuid: 'vg-a', vg_name: 'vg', extent_size_sectors: 10, members: [{ image_id: 'a', partition_id: 1, pv_local_id: 'pv-a', pv_uuid: 'pv', pe_start_sectors: 0, partition_byte_offset: 0, partition_byte_length: 512000 }], logical_volumes: [{ name: 'data', size_bytes: 4096, segments: [], assembled_image_id: 'lv-image' }] }], arrays: [array, { ...array, set_uuid: 'lv-array', members: [{ ...member, origin: { kind: 'lv', vg_uuid: 'vg-a', vg_name: 'vg', lv_name: 'lv', size_bytes: 512000 } }] }, { ...array, set_uuid: 'assembled', assembled_image_id: 'synthetic', members: [] }], filesystems: [], diagnostics: [] };

describe('RAID view projections', () => {
  it('includes direct members, PV-backed LV arrays and the assembled image itself', () => {
    expect(relevantRaidArrays(stack, 'a').map(item => item.set_uuid)).toEqual(['array-a', 'lv-array']);
    expect(relevantRaidArrays(stack, 'synthetic').map(item => item.set_uuid)).toEqual(['assembled']);
    expect(relevantRaidArrays(stack, 'unrelated')).toEqual([]); expect(relevantRaidVolumeGroups(stack, 'a')).toEqual(stack.volume_groups);
    expect(relevantRaidVolumeGroups(stack, 'lv-image')).toEqual(stack.volume_groups);
  });
  it('orders only assigned columns and keeps reconstructed and missing slots in the diagram', () => {
    const reconstructed = { ...member, device_uuid: 'rebuilt', slot: 2, reconstructed: true, state: 'stale' as const };
    const spare = { ...member, device_uuid: 'spare', slot: null, state: 'spare' as const };
    const arranged = { ...array, members: [reconstructed, spare, member] };
    expect(orderedMembers(arranged)).toEqual([member, reconstructed]); expect(missingSlotsFor(arranged)).toEqual([2]);
    expect(slotlessMembers(arranged)).toEqual([spare]);
    expect(diskLabelsFor(arranged, new Map([['a', 'D:/member.dd']]))).toEqual(['member.dd', null, 'member.dd']);
    expect(activeMembers(arranged)).toBe(1);
    // Once assembled, slots without a read member are drawn as rebuilt too.
    expect(missingSlotsFor({ ...arranged, assembled_image_id: 'x', degraded: true })).toEqual([1, 2]);
  });
  it('distinguishes assembled, degraded, incomplete, unsupported and failed geometry', () => {
    expect(arrayStatus(array)).toEqual({ label: '成员不全 1/3', sev: 'warn' });
    expect(arrayStatus({ ...array, assembled_image_id: 'assembled' })).toEqual({ label: '已重组', sev: 'success' });
    expect(arrayStatus({ ...array, assembled_image_id: 'assembled', degraded: true })).toEqual({ label: '已降级重组', sev: 'warn' });
    expect(arrayStatus({ ...array, degradable: true })).toEqual({ label: '缺 2 块，可降级重组', sev: 'warn' });
    expect(arrayStatus({ ...array, raid_disks: 1, level: 7 })).toEqual({ label: 'RAID7 暂不支持重组', sev: 'info' });
    expect(arrayStatus({ ...array, raid_disks: 1, layout: 9 })).toEqual({ label: 'RAID5 布局 9 暂不支持重组', sev: 'info' });
    expect(arrayStatus({ ...array, raid_disks: 1, level: 6, layout: 18 })).toEqual({ label: '未能重组，见诊断', sev: 'warn' });
    expect(arrayStatus({ ...array, raid_disks: 1 })).toEqual({ label: '未能重组，见诊断', sev: 'warn' });
    expect(exportBaseName(array)).toBe('array_a_RAID5');
    expect(exportBaseName({ ...array, level: -1 })).toBe('array_a_linear');
  });
  it('names levels and layouts', () => {
    expect(levelShortLabel(-1)).toBe('Linear');
    expect(levelShortLabel(10)).toBe('RAID 10');
    expect(layoutLabel({ ...array, level: 6, layout: 18 })).toBe('左同步（Q 固定末盘）');
    expect(layoutLabel({ ...array, level: 10, layout: 0x102 })).toBe('近端 2 份');
    expect(layoutLabel({ ...array, level: 10, layout: 0x10201 })).toBe('偏移 2 份');
  });
});
