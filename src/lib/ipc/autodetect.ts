// 自动识别平台 IPC。Wire
// contracts mirror the Rust serde structs in `core::autodetect` (snake_case
// fields; enums serialise to the reference platform words). The backend runs
// the same detection as a background first task when evidence first joins a
// case. Every run, manual or first task, is an `autoDetect` backend task and
// ends with `AUTODETECT_FINISHED_EVENT` (the report) or
// `AUTODETECT_FAILED_EVENT` before its `task-lifecycle` registration goes.
import { invokeCmd, type BackendAppError } from './client';
import { onEvent, type UnlistenFn } from './events';

/** 检材平台词表,存储在 `evidence.platform`。 */
export type EvidencePlatform =
  | 'android'
  | 'ios'
  | 'windows'
  | 'macos'
  | 'linux'
  | 'harmony'
  | 'huaweihmos'
  | 'othermobile'
  | 'drone';

/** 检材走的识别路(§3.0 三路)。 */
export type DetectRoute = 'disk_image' | 'archive' | 'phone_image';

export interface MarkerCheck {
  path: string;
  present: boolean;
}

export interface PartitionPlatform {
  partition_id: number;
  label: string;
  kind: string;
  /** 平台识别使用的文件系统类型码;不在门限表内为 null。 */
  ftype: number | null;
  /** C++ 层全匹配结论(“系统分区”)。 */
  native_platform: EvidencePlatform | null;
  native_markers: MarkerCheck[];
  /** Go 层映射器结论。 */
  platform: EvidencePlatform | null;
  go_markers: MarkerCheck[];
  filesystem_opened: boolean;
  bitlocker_locked: boolean;
  error: string | null;
}

export interface BackupConclusion {
  /** 备份类型枚举词,回写 `evidence.type`。 */
  backup_type: string;
  tool: string;
  platform: EvidencePlatform;
  manufactor: string | null;
  features: string[];
  needs_password: boolean;
}

export interface AutoDetectReport {
  image_id: string;
  route: DetectRoute;
  platform: EvidencePlatform | null;
  platforms: EvidencePlatform[];
  evidence_type: string;
  is_backup: boolean;
  manufactor: string | null;
  display_platform_name: string;
  friend_platform_name: string;
  partitions: PartitionPlatform[];
  backup: BackupConclusion | null;
  warnings: string[];
}

/** 识别完成事件名;载荷为 `AutoDetectReport`。 */
export const AUTODETECT_FINISHED_EVENT = 'autodetect-finished';

/** 识别失败或取消事件名;载荷为 `AutoDetectFailure`。 */
export const AUTODETECT_FAILED_EVENT = 'autodetect-failed';

export interface AutoDetectFailure {
  image_id: string;
  /** 与命令错误同形;取消时 `code` 为 `cancelled`。 */
  error: BackendAppError;
}

/** 手动触发识别并(有案件时)回写检材行。 */
export function autodetectEvidence(args: { imageId: string }): Promise<AutoDetectReport> {
  return invokeCmd<AutoDetectReport>('autodetect_evidence', args);
}

export function onAutoDetectFinished(
  handler: Parameters<typeof onEvent<AutoDetectReport>>[1],
): Promise<UnlistenFn> {
  return onEvent<AutoDetectReport>(AUTODETECT_FINISHED_EVENT, handler);
}

export function onAutoDetectFailed(
  handler: Parameters<typeof onEvent<AutoDetectFailure>>[1],
): Promise<UnlistenFn> {
  return onEvent<AutoDetectFailure>(AUTODETECT_FAILED_EVENT, handler);
}
