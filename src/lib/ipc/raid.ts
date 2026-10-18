import { invokeCmd } from './client';



export interface RaidStack {
  images: RaidImage[];
  volume_groups: RaidVolumeGroup[];
  arrays: RaidArray[];
  filesystems: RaidFilesystem[];
  diagnostics: string[];
}

export interface RaidImage {
  image_id: string;
  path: string;
  size_bytes: number;
  partition_count: number;
  lvm_pv_count: number;
}

export interface RaidVolumeGroup {
  vg_uuid: string;
  vg_name: string;
  extent_size_sectors: number;
  members: RaidVgMember[];
  logical_volumes: RaidLogicalVolume[];
}

export interface RaidVgMember {
  image_id: string;
  partition_id: number;
  pv_local_id: string;
  pv_uuid: string;
  pe_start_sectors: number;
  partition_byte_offset: number;
  partition_byte_length: number;
}

export interface RaidLogicalVolume {
  name: string;
  size_bytes: number;
  segments: RaidLvSegment[];
  /** Synthetic image of an LV spanning several physical volumes, once
   *  registered; single-PV LVs are browsed through their PV's partition. */
  assembled_image_id: string | null;
}

export interface RaidLvSegment {
  pv_local_id: string;
  image_id: string;
  partition_id: number;
  byte_offset: number;
  byte_length: number;
}

export interface RaidArray {
  set_uuid: string;
  set_name: string;
  /** md personality: `-1` linear, `0`, `1`, `4`, `5`, `6` or `10`. */
  level: number;
  /** Raw md `layout` field: the RAID5/6 parity rotation, the RAID10 copy
   *  layout (`near | far << 8 | offset << 16`) or the RAID0 zone layout. */
  layout: number;
  chunksize_sectors: number;
  raid_disks: number;
  data_offset_sectors: number;
  assembled_bytes: number;
  members: RaidArrayMember[];
  /** Set once the backend has registered the assembled array as a synthetic
   *  `DiskImage`; null for incomplete sets, unsupported levels, or degraded
   *  sets awaiting the examiner's decision. */
  assembled_image_id: string | null;
  /** `true` when this array was reconstructed by the metadata-less heuristic
   *  (no mdadm superblock / LVM label) — its level, chunk, layout and disk
   *  order were *recovered* by brute force and verified against the
   *  filesystem, not read from on-disk metadata. */
  heuristic: boolean;
  /** md metadata version (`0.90`, `1.0`, `1.1`, `1.2`); empty for
   *  metadata-less arrays. */
  metadata_version: string;
  /** The assembled device lacks members: their data comes from parity or the
   *  remaining mirrors. */
  degraded: boolean;
  /** Not assembled because members the metadata still expects are absent,
   *  though the present ones suffice; `raidAutoDetect([set_uuid])` assembles
   *  it degraded. */
  degradable: boolean;
}

/** What a member contributes to its array. */
export type RaidMemberState = 'active' | 'spare' | 'faulty' | 'journal' | 'stale' | 'rebuilding' | 'duplicate';

export interface RaidArrayMember {
  dev_number: number;
  /** Slot within the array (0..raid_disks-1); null for spare, faulty and
   *  journal devices. */
  slot: number | null;
  device_uuid: string;
  data_size_sectors: number;
  /** `true` when this column's payload is rebuilt from parity rather than read
   *  off the disk — a stale or still-rebuilding member of a parity array.
   *  Drives the dropped-disk styling in the layout diagram. */
  reconstructed: boolean;
  state: RaidMemberState;
  /** md event counter; null for metadata-less members. */
  events: number | null;
  origin: RaidMemberOrigin;
}

export type RaidMemberOrigin =
  | { kind: 'lv'; vg_uuid: string; vg_name: string; lv_name: string; size_bytes: number }
  | { kind: 'whole'; image_id: string; partition_id: number; byte_offset: number; byte_length: number };

export interface RaidFilesystem {
  set_uuid: string;
  kind: string;
  fsid: string | null;
  total_bytes: number | null;
  bytes_used: number | null;
  num_devices: number | null;
  sector_size: number | null;
  node_size: number | null;
}

/** Run LVM → md → filesystem detection over every loaded image. Idempotent:
 *  assembled arrays and cross-PV logical volumes are registered as synthetic
 *  images once and reused on re-runs. `degradedSets` names md sets to assemble
 *  without the members their metadata still expects. Heavy-ish (reads member
 *  superblocks) but bounded. */
export function raidAutoDetect(degradedSets: string[] = []): Promise<RaidStack> {
  return invokeCmd<RaidStack>('raid_auto_detect', { degradedSets });
}

/** Explicit geometry for a manual RAID reconstruction (the builder fallback).
 *  `member_image_ids` are loaded-image ids in column (slot) order; an empty
 *  string stands for a column whose disk is not available. */
export interface ManualRaidParams {
  member_image_ids: string[];
  /** md level: `-1` linear, `0`, `1`, `4`, `5`, `6` or `10`. */
  level: number;
  /** RAID5/6 rotation, or the RAID10 copy layout
   *  (`near | far << 8 | offset << 16`); ignored for other levels. */
  layout: number;
  /** Stripe/chunk size in 512-byte sectors (ignored by RAID1). */
  chunk_sectors: number;
  /** Per-member reserved-header size in 512-byte sectors. */
  data_offset_sectors: number;
  /** Columns to rebuild from parity or mirrors instead of reading them. */
  missing_slots: number[];
}

export interface ManualRaidPartition {
  kind: string;
  start_lba: number;
  length_lba: number;
  label: string | null;
}

/** Result of `raid_manual_assemble`: the registered synthetic image id, its
 *  logical size, and a preview of the partitions the geometry yields (empty
 *  when the parameters produce no recognisable table). */
export interface ManualRaidResult {
  image_id: string;
  size_bytes: number;
  partitions: ManualRaidPartition[];
}

/** Assemble a RAID device from an explicit, examiner-supplied geometry and
 *  register it as a synthetic image. Idempotent on the geometry. */
export function raidManualAssemble(params: ManualRaidParams): Promise<ManualRaidResult> {
  return invokeCmd<ManualRaidResult>('raid_manual_assemble', { params });
}
