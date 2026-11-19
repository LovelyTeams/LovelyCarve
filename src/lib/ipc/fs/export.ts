import { invokeCmd } from '../client';
import type { ImageId, NodeId } from '../ids';



export interface BadRegion {
  offset: number;
  len: number;
}

export interface ReadFaultReport {
  bad_regions: BadRegion[];
  synthetic_bytes: number;
  read_faults_detected: boolean;
}

export interface FsExportResult extends ReadFaultReport {
  bytes_written: number;
}

export interface FsExportDirSummary {
  file_count: number;
  dir_count: number;
  bytes_written: number;
  skipped_count: number;
  skipped_deleted: number;
  truncated: boolean;
  cancelled: boolean;
  bad_regions: BadRegion[];
  synthetic_bytes: number;
  read_faults_detected: boolean;
}

export interface FsExportDirProgress {
  task_id: string;
  files_done: number;
  dirs_done: number;
  bytes_written: number;
  skipped: number;
  skipped_deleted: number;
  current_path: string | null;
}

export type ExportDirMode = 'tree' | 'zip';

export function fsExport(args: { imageId: ImageId; nodeId: NodeId; destination: string }): Promise<FsExportResult> {
  return invokeCmd<FsExportResult>('fs_export', args);
}

export function fsExportDir(args: {
  imageId: ImageId;
  nodeId: NodeId;
  destination: string;
  taskId: string;
  mode: ExportDirMode;
}): Promise<FsExportDirSummary> {
  return invokeCmd<FsExportDirSummary>('fs_export_dir', args);
}
