import { invokeCmd } from './client';
import type { ImageInfo } from './image';
import type { ImageId, VolumeKey } from './ids';

export interface EncryptedUnlockOptions {
  target?: 'newImage' | 'sourcePartition';
  scheme: 'luks' | 'veracrypt' | 'filevault';
  password?: string;
  masterKeyHex?: string;
  partitionId?: number;
  offset?: number;
  length?: number;
  hidden?: boolean;
  pim?: number;
  logicalVolumeIndex?: number;
  encryptedRootPlistPath?: string;
}

export interface EncryptedUnlockResult {
  volume_key?: VolumeKey;
  logical_volume_identifier?: string;
  logical_volume_index?: number;
  image: ImageInfo;
  scheme: 'luks' | 'veracrypt' | 'filevault';
  cipher: string;
  encryption_sector_size: number;
  source_offset: number;
  source_length: number;
  hidden: boolean;
}

export function encryptedVolumeUnlock(args: {
  imageId: ImageId;
  options: EncryptedUnlockOptions;
  taskId?: string;
}): Promise<EncryptedUnlockResult> {
  return invokeCmd<EncryptedUnlockResult>('encrypted_volume_unlock', args);
}

export interface FilevaultVolumeInfo {
  index: number; name: string | null; identifier: string | null;
  family_identifier: string; size: number; pbkdf2_iterations: number;
  protectors: string[]; conversion_status: string | null;
  findings: { code: string; severity: string; note: string }[];
  object_identifier: string; transaction: string;
}
export interface FilevaultInventory {
  volumes: FilevaultVolumeInfo[]; header_offset: number;
  source_offset: number; source_length: number;
  physical_volume_identifier: string; volume_group_identifier: string; volume_group_name: string | null;
  metadata_pointer_offset: number; encrypted_metadata_offset: number;
}
export function filevaultInspect(args: {
  imageId: ImageId; options: Pick<EncryptedUnlockOptions, 'partitionId' | 'offset' | 'length' | 'encryptedRootPlistPath'>;
  taskId?: string;
}): Promise<FilevaultInventory> {
  return invokeCmd<FilevaultInventory>('filevault_inspect', args);
}
