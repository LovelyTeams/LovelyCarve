// Case + volume-snapshot IPC. Typed wrappers over the `case_*` / `snapshot_*`
// backend commands. Interfaces mirror the Rust serde structs in
// `core::case` / `commands::case` / `commands::case_snapshot` (snake_case
// field names on the wire).
//
// 案件以台账 `case.db` 为中心(见 docs/ARCHITECTURE.md「案件数据库」):
// `case_create` 建目录与台账,`case_open` 加进程锁并自愈,`case_close` 记审计
// 后释放,`case_lock`/`case_unlock` 翻转逻辑锁,`case_remove` 只删除案件自己
// 的文件,`case_list` 读本地登记簿。
import { invokeCmd } from './client';
import { onEvent } from './events';

/** Disk-image source format (matches Rust `ImageFormat`, serde snake_case). */
export type ImageFormat =
  | 'raw'
  | 'ewf'
  | 'vmdk'
  | 'qcow2'
  | 'phdi'
  | 'vdi'
  | 'vhd'
  | 'vhdx'
  | 'dmg'
  | 'pmfx'
  | 'xiaomi_firmware'
  | 'ldplayer'
  | 'docker'
  | 'tar'
  | 'zip'
  | 'sevenz'
  | 'ad1'
  | 'physical_disk'
  | 'folder'
  | 'assembled';

export interface EvidenceRef {
  image_id: string;
  path: string;
  source_format: ImageFormat;
  size_bytes: number;
  added_at: string;
  /** 自动识别回写的平台(未识别为 null)。 */
  platform?: string | null;
  /** 自动识别回写的检材类型(`disk` / `compressfile` / 备份枚举词)。 */
  evidence_type?: string | null;
}

/** One published volume of an evidence: its directory snapshot and whether a
 *  file index currently serves it. */
export interface SnapshotRef {
  volume_key: string;
  image_id: string;
  entry_count: number;
  truncated: boolean;
  built_at: string;
  source_volume_size: number;
  /** A file index is available for this volume. */
  indexed: boolean;
}

/** A `SnapshotRef` plus a computed freshness flag (backend flattens these). */
export interface SnapshotInfo extends SnapshotRef {
  stale: boolean;
}

/** The active case: ledger profile + evidence + published volumes. */
export interface CaseInfo {
  /** Case uid, generated at creation; stays the same when the directory moves. */
  id: string;
  case_number: string;
  name: string;
  dir: string;
  examiner: string;
  description: string;
  /** Logical lock: editing is refused while set. */
  locked: boolean;
  /** Case ledger structure version. */
  schema_version: number;
  created_at: string;
  updated_at: string;
  evidence: EvidenceRef[];
  snapshots: SnapshotRef[];
}

/** One entry of the local case registry, enriched with the ledger profile.
 *  `missing` marks a directory/ledger that can no longer be read. */
export interface CaseListItem {
  dir: string;
  /** Case uid; empty when the ledger cannot be read. */
  id: string;
  case_number: string;
  name: string;
  examiner: string;
  locked: boolean;
  created_at: string;
  updated_at: string;
  opened_at: string;
  missing: boolean;
}

/** One case-level setting. */
export interface CaseOption {
  key: string;
  value: string;
}

/** Payload of the `snapshot-build-progress` event. */
export interface SnapshotProgress {
  task_id: string;
  image_id: string;
  volume_key: string;
  /** started | reused | walking | indexing | done | cancelled | error */
  phase: 'started' | 'reused' | 'walking' | 'indexing' | 'done' | 'cancelled' | 'error';
  entries: number;
  truncated: boolean;
  message: string | null;
}

/** Event name carrying snapshot-build progress (matches the Rust constant). */
export const SNAPSHOT_PROGRESS_EVENT = 'snapshot-build-progress';

export function caseCreate(args: {
  dir: string;
  name: string;
  caseNumber?: string;
  description?: string;
  examiner?: string;
}): Promise<CaseInfo> {
  return invokeCmd<CaseInfo>('case_create', args);
}

export function caseOpen(args: { dir: string }): Promise<CaseInfo> {
  return invokeCmd<CaseInfo>('case_open', args);
}

/** Exit the active case: flush, release the process lock, unload its images. */
export function caseClose(): Promise<void> {
  return invokeCmd<void>('case_close');
}

export function caseActive(): Promise<CaseInfo | null> {
  return invokeCmd<CaseInfo | null>('case_active');
}

/** Locally registered cases, most recently opened first. */
export function caseList(): Promise<CaseListItem[]> {
  return invokeCmd<CaseListItem[]>('case_list');
}

/** Delete a case's own files (ledger, evidence and staging directories) and
 *  drop it from the registry; other files in the directory are kept. Closes
 *  the case first when it is the active one. */
export function caseRemove(args: { dir: string }): Promise<void> {
  return invokeCmd<void>('case_remove', args);
}

/** Set the logical lock on the active case. Returns the refreshed case info. */
export function caseLock(): Promise<CaseInfo> {
  return invokeCmd<CaseInfo>('case_lock');
}

export function caseUnlock(): Promise<CaseInfo> {
  return invokeCmd<CaseInfo>('case_unlock');
}

export function caseRename(args: { name: string }): Promise<CaseInfo> {
  return invokeCmd<CaseInfo>('case_rename', args);
}

export function caseOptionQuery(args: { prefix?: string } = {}): Promise<CaseOption[]> {
  return invokeCmd<CaseOption[]>('case_option_query', args);
}

export function caseOptionSave(args: { entries: CaseOption[] }): Promise<void> {
  return invokeCmd<void>('case_option_save', args);
}

export function caseEvidenceList(): Promise<EvidenceRef[]> {
  return invokeCmd<EvidenceRef[]>('case_evidence_list');
}

/** Remove an evidence image from the active case (cascades to its volumes,
 *  tags, tasks and analysis data). Returns the refreshed case info. */
export function caseEvidenceRemove(args: { imageId: string }): Promise<CaseInfo> {
  return invokeCmd<CaseInfo>('case_evidence_remove', args);
}

/** Start (or force-rebuild) a volume snapshot. `taskId` doubles as the
 *  cancellation token id — pass it to `task_cancel` to abort. */
export function snapshotBuild(args: {
  imageId: string;
  volumeKey: string;
  taskId: string;
}): Promise<void> {
  return invokeCmd<void>('snapshot_build', args);
}

export function snapshotList(): Promise<SnapshotInfo[]> {
  return invokeCmd<SnapshotInfo[]>('snapshot_list');
}

export function snapshotDrop(args: { imageId: string; volumeKey: string }): Promise<void> {
  return invokeCmd<void>('snapshot_drop', args);
}

/** Deterministic cancellation-token id for an auto-build (mirrors the Rust
 *  `format!("snapshot:{image_id}:{volume_key}")`). */
export function autoSnapshotTaskId(imageId: string, volumeKey: string): string {
  return `snapshot:${imageId}:${volumeKey}`;
}

export function onSnapshotProgress(handler: Parameters<typeof onEvent<SnapshotProgress>>[1]) {
  return onEvent<SnapshotProgress>(SNAPSHOT_PROGRESS_EVENT, handler);
}
