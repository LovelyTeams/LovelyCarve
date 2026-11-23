import { invokeCmd } from './client';
import type { ImageInfo } from './image';
import type { ImageId, VolumeKey } from './ids';

/** Encryption formats the one unlock command opens. */
export type EncryptionScheme = 'bitlocker' | 'luks' | 'veracrypt' | 'filevault';

/** What opens a volume. The backend checks that the kind fits the scheme:
 *  BitLocker takes every kind, LUKS and FileVault a password or master key,
 *  VeraCrypt a password. */
export type UnlockCredential =
  | { kind: 'password'; password: string }
  | { kind: 'recoveryPassword'; recoveryPassword: string }
  | { kind: 'startupKey'; path: string }
  /** LUKS master key, FileVault VMK / XTS key pair, or BitLocker FVEK with its TWEAK. */
  | { kind: 'masterKey'; keyHex: string; tweakHex?: string }
  | { kind: 'vmk'; keyHex: string }
  | { kind: 'clearKey' };

export interface EncryptedUnlockOptions {
  target?: 'newImage' | 'sourcePartition';
  scheme: EncryptionScheme;
  credential: UnlockCredential;
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
  scheme: EncryptionScheme;
  cipher: string;
  encryption_sector_size: number;
  source_offset: number;
  source_length: number;
  hidden: boolean;
  /** The BitLocker recovery password a VMK yielded. */
  recovery_password?: string;
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

export interface BitlockerVmkCandidate {
  offset: number;
  type_flag: number;
  key_type: number;
  vmk_hex: string;
  recovery_key: string | null;
  already_tried: boolean;
}

export interface BitlockerVmkScanResult {
  partition_id: number;
  candidates: BitlockerVmkCandidate[];
}

/** Progress of a memory-image scan, keyed by its task id. */
export const BITLOCKER_SCAN_PROGRESS_EVENT = 'bitlocker-scan-progress';
export interface BitlockerScanProgress {
  task_id: string;
  bytes_done: number;
  bytes_total: number;
}

/** Find BitLocker VMK candidates in a memory image, each with the recovery
 *  password it derives against the partition. Emits
 *  {@link BITLOCKER_SCAN_PROGRESS_EVENT}. */
export function bitlockerScanMemory(args: {
  imageId: ImageId;
  partitionId: number;
  memoryPath: string;
  taskId: string;
}): Promise<BitlockerVmkScanResult> {
  return invokeCmd<BitlockerVmkScanResult>('bitlocker_scan_memory', args);
}
