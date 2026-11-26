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

/** 导出一个文件，作为任务 `taskId` 运行：可用 `task_cancel` 取消，取消后不写出目标文件。 */
export function fsExport(args: { imageId: ImageId; nodeId: NodeId; destination: string; taskId: string }): Promise<FsExportResult> {
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
