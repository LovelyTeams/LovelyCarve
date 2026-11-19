import { invokeCmd } from './client';
import type { ImageInfo } from './image';
import type { ImageId } from './ids';

/** Upstream native observation. Unrated findings retain a null severity. */
export interface IosBackupFinding {
  code: string;
  note: string;
  severity: 'Info' | 'Low' | 'Medium' | 'High' | 'Critical' | null;
  category: string;
  source: { analyzer: string; scope: string; version: string | null };
  evidence: { field: string; value: string; location: unknown }[];
  subjects: unknown[];
  context: unknown;
}

export function iosBackupUnlock(args: { imageId: ImageId; password: string; taskId: string }): Promise<ImageInfo> {
  return invokeCmd<ImageInfo>('ios_backup_unlock', args);
}

export function iosBackupAudit(args: { imageId: ImageId; taskId: string }): Promise<IosBackupFinding[]> {
  return invokeCmd<IosBackupFinding[]>('ios_backup_audit', args);
}
