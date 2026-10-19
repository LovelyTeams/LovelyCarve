import { invokeCmd } from './client';
import { onEvent, type UnlistenFn } from './events';
import type { ReadFaultReport } from './fs/export';
import type { FsNode } from './fs/types';
import type { DiskPartition } from './image';

export type CarveScope = 'whole' | 'unallocated' | 'partition' | 'range';
export type CarveSessionStatus = 'queued' | 'running' | 'paused' | 'completed' | 'cancelled' | 'failed';
export type CarveCategory = 'image' | 'document' | 'archive' | 'database' | 'audio' | 'video';
/** A recovery scan runs filesystem recovery first, then signature carving. */
export type CarveScanPhase = 'filesystem' | 'signature';

/** Filesystem families to recover; every family off skips that phase. */
export interface RecoveryFilesystems {
  fat: boolean;
  ntfs: boolean;
  exfat: boolean;
  /** Also list live files of known volumes; lost volumes always list every file. */
  include_existing: boolean;
}

export interface CarveFormatInfo {
  id: string;
  extension: string;
  label: string;
  category: CarveCategory;
  description: string;
  signatures: string[];
}

export interface CarveStartOptions {
  format_ids: string[];
  scope: CarveScope;
  partition_id: number | null;
  /** `range` scope only. */
  start_sector?: number | null;
  sector_count?: number | null;
  max_file_size: number;
  max_results: number;
  filesystems?: RecoveryFilesystems;
  /** Raw-signature carving; defaults to true. */
  signatures?: boolean;
}

export interface CarveStartResult {
  scan_id: string;
  task_id: string;
}

export interface CarveArtifact {
  id: string;
  name: string;
  format_id: string;
  extension: string;
  category: CarveCategory;
  offset: number;
  length: number;
  confidence: number;
  region: string;
  read_fault: boolean;
}

export interface FilesystemScanSummary {
  options: RecoveryFilesystems;
  /** Boot-sector bytes while `total` is known, then enumerated records. */
  done: number;
  total: number | null;
  finished: boolean;
  volumes: number;
  directories: number;
  files: number;
  truncated: boolean;
  read_faults_detected: boolean;
  error: string | null;
}

export interface CarveScanSnapshot {
  scan_id: string;
  task_id: string;
  image_id: string;
  created_unix_ms: number;
  /** Set once the scan ended. */
  finished_unix_ms: number | null;
  status: CarveSessionStatus;
  phase: CarveScanPhase;
  /** Whether the session runs signature carving. */
  signatures: boolean;
  /** Present when the session recovers filesystem records. */
  filesystem: FilesystemScanSummary | null;
  scope_label: string;
  bytes_scanned: number;
  bytes_total: number;
  signatures_found: number;
  artifacts_found: number;
  current_offset: number;
  truncated: boolean;
  error: string | null;
}

export interface CarveResultFilter {
  format_id?: string | null;
  category?: CarveCategory | null;
  min_confidence?: number | null;
}

export interface CarveResultsPage {
  items: CarveArtifact[];
  next_cursor: string | null;
  /** Last stable result key returned even while a background scan is growing. */
  resume_cursor: string | null;
  total: number;
  truncated: boolean;
  scan_finished: boolean;
}

export interface RecoveryVolume {
  partition: DiskPartition;
  lost: boolean;
  reconstruction: { mft_offset: number; cluster_size: number; record_size: number; sector_size: number } | null;
  error: string | null;
}

export interface RecoveryCandidate {
  id: string;
  volume_index: number;
  path: string;
  node: FsNode;
  /** A readable mapping can be exported; it does not prove the content was not overwritten. */
  readable: boolean;
  initialized_size: number | null;
  /** Recycle Bin restores keep the physical record here. */
  source_path?: string | null;
  source_node?: FsNode | null;
}

export interface RecoveryDirectory {
  volume_index: number;
  path: string;
  node: FsNode;
  source_path?: string | null;
  source_node?: FsNode | null;
}

/** Filesystem recovery result of a scan session. */
export interface RecoveryScan {
  scan_id: string;
  image_id: string;
  options: RecoveryFilesystems;
  ranges: { start_sector: number; sector_count: number }[];
  volumes: RecoveryVolume[];
  candidates: RecoveryCandidate[];
  directories: RecoveryDirectory[];
  truncated: boolean;
  warnings: string[];
  read_faults: ReadFaultReport;
}

export interface RecoveryExport {
  directory: string;
  recovered: number;
  failed: number;
  bytes_written: number;
  errors: string[];
  report_path: string;
  read_faults: ReadFaultReport;
}

/** Top-level counts describe carved artifacts; `files` describes filesystem records. */
export interface CarveRecoveryResult {
  recovered: number;
  failed: number;
  bytes_written: number;
  synthetic_artifacts: number;
  report_path: string | null;
  report_error: string | null;
  errors: string[];
  errors_truncated: boolean;
  files: RecoveryExport | null;
}

export const CARVE_SCAN_PROGRESS_EVENT = 'carve-scan-progress';

export function carveFormats(): Promise<CarveFormatInfo[]> {
  return invokeCmd<CarveFormatInfo[]>('carve_formats');
}

export function carveStart(args: {
  imageId: string;
  options: CarveStartOptions;
}): Promise<CarveStartResult> {
  return invokeCmd<CarveStartResult>('carve_start', args);
}

export function carveScanStatus(args: { scanId: string }): Promise<CarveScanSnapshot> {
  return invokeCmd<CarveScanSnapshot>('carve_scan_status', args);
}

export function carveScanPause(args: { scanId: string }): Promise<CarveScanSnapshot> {
  return invokeCmd<CarveScanSnapshot>('carve_scan_pause', args);
}

export function carveScanResume(args: { scanId: string }): Promise<CarveScanSnapshot> {
  return invokeCmd<CarveScanSnapshot>('carve_scan_resume', args);
}

export function carveLatestScan(args: { imageId: string }): Promise<CarveScanSnapshot | null> {
  return invokeCmd<CarveScanSnapshot | null>('carve_latest_scan', args);
}

export function carveResultsPage(args: {
  scanId: string;
  cursor?: string | null;
  limit?: number;
  filter?: CarveResultFilter | null;
}): Promise<CarveResultsPage> {
  return invokeCmd<CarveResultsPage>('carve_results_page', args);
}

/** Read a bounded carved-file prefix as a Tauri raw binary response. */
export function carvePreview(args: {
  scanId: string;
  artifactId: string;
  maxBytes?: number;
}): Promise<ArrayBuffer> {
  return invokeCmd<ArrayBuffer>('carve_preview', args);
}

/** Recovered volumes, directories and files; `null` until the filesystem phase finishes. */
export function carveFilesystemResults(args: { scanId: string }): Promise<RecoveryScan | null> {
  return invokeCmd<RecoveryScan | null>('carve_filesystem_results', args);
}

export function carveRecover(args: {
  scanId: string;
  options: {
    /** `null` recovers every carved result; `[]` none when `file_ids` is set. */
    artifact_ids: string[] | null;
    file_ids?: string[];
    destination: string;
    task_id: string;
  };
}): Promise<CarveRecoveryResult> {
  return invokeCmd<CarveRecoveryResult>('carve_recover', args);
}

export function onCarveScanProgress(
  handler: Parameters<typeof onEvent<CarveScanSnapshot>>[1],
): Promise<UnlistenFn> {
  return onEvent<CarveScanSnapshot>(CARVE_SCAN_PROGRESS_EVENT, handler);
}

/** Retained carving sessions for the current evidence, newest first. */
export function carveScanList(args: { imageId: string }): Promise<CarveScanSnapshot[]> {
  return invokeCmd<CarveScanSnapshot[]>('carve_scan_list', args);
}

export function carveScanRemove(args: { imageId: string; scanId: string }): Promise<void> {
  return invokeCmd<void>('carve_scan_remove', args);
}
