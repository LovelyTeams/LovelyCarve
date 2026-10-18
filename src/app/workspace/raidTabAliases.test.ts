import { describe, expect, it } from 'vitest';
import type { EvidenceRef } from '../../lib/ipc/cases';
import type { RaidArray, RaidStack } from '../../lib/ipc/raid';
import { raidMemberTabAliases, raidTabPathKey } from './raidTabAliases';

const array: RaidArray = { set_uuid: 'set', set_name: 'set', level: 5, layout: 2, chunksize_sectors: 1024,
  raid_disks: 2, data_offset_sectors: 0, assembled_bytes: 1024, assembled_image_id: 'assembled', heuristic: false,
  metadata_version: '1.2', degraded: false, degradable: false,
  members: [{ dev_number: 0, slot: 0, device_uuid: '', data_size_sectors: 1, reconstructed: false, state: 'active', events: 1,
    origin: { kind: 'whole', image_id: 'member', partition_id: 0, byte_offset: 0, byte_length: 512 } }] };
const evidence: EvidenceRef[] = [{ image_id: 'assembled', path: 'assembled://set', source_format: 'assembled', size_bytes: 1024, added_at: '' },
  { image_id: 'member', path: 'D:\\NAS\\sdb.E01', source_format: 'ewf', size_bytes: 512, added_at: '' }];
const stack: RaidStack = { images: [], arrays: [array], volume_groups: [], filesystems: [], diagnostics: [] };

describe('RAID tab aliases', () => {
  it('groups only successfully assembled evidence and keeps platform path identity', () => {
    expect([...raidMemberTabAliases(stack, evidence)]).toEqual([['d:/nas/sdb.e01', 'assembled://set']]);
    expect(raidTabPathKey('D:/NAS/SDB.E01')).toBe('d:/nas/sdb.e01');
    expect(raidTabPathKey('/NAS/SDB.E01')).toBe('/NAS/SDB.E01');
    expect(raidMemberTabAliases({ ...stack, arrays: [{ ...array, assembled_image_id: null }] }, evidence).size).toBe(0);
    expect(raidMemberTabAliases(stack, evidence.slice(1)).size).toBe(0);
    expect(raidMemberTabAliases(null, evidence).size).toBe(0);
  });
  it('sends a disk with members of several arrays to the largest one', () => {
    const system = { ...array, set_uuid: 'system', assembled_image_id: 'system-image', assembled_bytes: 512 };
    const systemEvidence = { ...evidence[0], image_id: 'system-image', path: 'assembled://system' };
    expect([...raidMemberTabAliases({ ...stack, arrays: [system, array] }, [...evidence, systemEvidence])])
      .toEqual([['d:/nas/sdb.e01', 'assembled://set']]);
  });
  it('groups images that host the backing physical volumes of an LV member', () => {
    const logical = { ...array, members: [{ ...array.members[0], origin: { kind: 'lv' as const, vg_uuid: 'vg', vg_name: 'pool', lv_name: 'data', size_bytes: 512 } }] };
    const backing = { vg_uuid: 'vg', vg_name: 'pool', extent_size_sectors: 1, logical_volumes: [], members: [{ image_id: 'member', partition_id: 0, pv_local_id: '', pv_uuid: '', pe_start_sectors: 0, partition_byte_offset: 0, partition_byte_length: 512 }] };
    expect([...raidMemberTabAliases({ ...stack, arrays: [logical], volume_groups: [backing] }, evidence)]).toEqual([['d:/nas/sdb.e01', 'assembled://set']]);
  });
});
