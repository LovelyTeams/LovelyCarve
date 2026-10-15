import { invokeCmd } from './client';
import { type HashAlgo } from './fs/derived';
// Wire types for a loaded evidence image, shared by the image-load, RAID and
// encrypted-volume commands. They live in the IPC layer so domain modules do
// not import from composables; consumers use this transport model directly.

export interface DiskPartition {
  id: number;
  kind: string;
  start_lba: number;
  length_lba: number;
  label: string | null;
}

/** Optional display interpretation; raw acquisition fields remain unchanged. */
export interface EwfHeaderDate {
  display: string;
  /** Local wall time has no recorded offset; Unix seconds use the UTC epoch basis. */
  time_basis: 'local' | 'unix';
}

/** EWF (E01/Ex01) 采集元数据，字段缺失时为 null。 */
export interface EwfAcquiryInfo {
  case_number: string | null;
  evidence_number: string | null;
  examiner_name: string | null;
  description: string | null;
  notes: string | null;
  acquiry_date: string | null;
  system_date: string | null;
  acquiry_date_parsed?: EwfHeaderDate | null;
  system_date_parsed?: EwfHeaderDate | null;
  acquiry_operating_system: string | null;
  acquiry_software_version: string | null;
  model: string | null;
  serial_number: string | null;
  stored_md5: string | null;
  stored_sha1: string | null;
  format: string | null;
  media_type: string | null;
  compression: string | null;
  segment_count: number | null;
}

/** 卷影副本会话镜像的来源（`ImageInfo.shadow_copy`）。 */
export interface ShadowCopyOrigin {
  parent_image_id: string;
  partition_id: number;
  /** 按创建时间排序的序号，最早为 0。 */
  index: number;
  store_id: string;
  shadow_copy_id: string | null;
  created_unix_ms: number | null;
}

export interface ImageInfo {
  unlocked_partitions?: number[];
  /** Source unavailable; the session serves published case metadata indexes. */
  index_only?: boolean;
  id: string;
  path: string;
  source_format: string;
  size_bytes: number;
  sector_size: number;
  partitions: DiskPartition[];
  /** 分区表类型(gpt/mbr/apm/bsd/luks/mdadm/lvm2/volume),未识别为 null。 */
  table_scheme: string | null;
  /** GPT 磁盘 GUID 或 MBR 磁盘签名(0xXXXXXXXX)。 */
  disk_id: string | null;
  /** 源文件占用大小(E01 为各分段之和),无法获取为 null。 */
  on_disk_bytes: number | null;
  modified_unix_ms: number | null;
  created_unix_ms: number | null;
  /** EWF 采集元数据；非 EWF 镜像为 null。 */
  ewf: EwfAcquiryInfo | null;
  /** 卷影副本会话镜像的来源；其他镜像为 null 或缺省。 */
  shadow_copy?: ShadowCopyOrigin | null;
  ios_backup?: IosBackupInfo | null;
}

export interface IosBackupInfo {
  locked: boolean;
  declared_encrypted: boolean;
  observed_encrypted: boolean | null;
  device_name: string | null;
  product_type: string | null;
  product_version: string | null;
  serial_number: string | null;
  unique_identifier: string | null;
  snapshot_state: string | null;
  file_count: number | null;
  unreadable_rows: number | null;
  unresolved_sizes: number;
}


export function imageHashCompute<T>(args: {
  imageId: string;
  algorithms: HashAlgo[];
  taskId: string;
  readFaultPolicy?: 'strict' | 'zeroFillAndReport';
}): Promise<T> {
  return invokeCmd<T>('image_hash_compute', args);
}

/** Minimal view of a loaded image for the manual-builder member picker. */
export interface LoadedImage {
  id: string;
  /** `assembled://<label>` for synthetic RAID images (filtered out as members). */
  path: string;
  size_bytes: number;
}

/** List every loaded image (real + synthetic). The manual builder uses this to
 *  offer member candidates; `image_list` returns the full `ImageInfo`, of which
 *  only these fields are read. */
export function listLoadedImages(): Promise<LoadedImage[]> {
  return invokeCmd<LoadedImage[]>('image_list');
}

/** The slice of `ImageInfo` this domain needs when resolving an assembled
 *  array's synthetic image (full shape is declared above). */
export interface LoadedImageSummary {
  id: string;
  /** `assembled://<label>` for synthetic RAID images. */
  path: string;
  source_format: string;
  size_bytes: number;
}

/** Look up an already-loaded image (incl. synthetic assembled ones) by id. */
export function imageInfoById(args: { imageId: string }): Promise<LoadedImageSummary> {
  return invokeCmd<LoadedImageSummary>('image_info', args);
}

/** Legacy QCOW credentials stay in the invocation only; no case persistence. */
export type ImageLoadArgs = {
  path: string;
  qcowPassword?: string;
  qcowKeyHex?: string;
};

/** Load an image / folder by path WITHOUT touching the active file session —
 *  registers it in the backend (and the active case, if any) so a subsequent
 *  `raid_auto_detect` can see it. Used to background-load RAID member disks
 *  before reconstruction, so the members never open as their own tabs. */
export function imageLoad(args: ImageLoadArgs): Promise<LoadedImageSummary> {
  return invokeCmd<LoadedImageSummary>('image_load', args);
}

export function imageUnload(args: { imageId: string }): Promise<void> {
  return invokeCmd<void>('image_unload', args);
}

/** One attached physical disk, mirroring the Rust `core::physical_disk::PhysicalDisk`
 *  serde struct (snake_case on the wire). `path` is the OS-native device path
 *  (`\\.\PhysicalDriveN`) that loads through `imageLoad` / `image_load`. */
export interface PhysicalDiskInfo {
  path: string;
  display: string;
  size_bytes: number;
  sector_size: number;
  model: string | null;
  is_removable: boolean;
}

/** Enumerate the physical disks attached to the host (best-effort — disks the
 *  process can't open without elevation are silently skipped, so an unelevated
 *  app may get an empty/partial list). Backed by `physical_disks_list`. */
export function physicalDisksList(): Promise<PhysicalDiskInfo[]> {
  return invokeCmd<PhysicalDiskInfo[]>('physical_disks_list');
}

/** One partition as reported by `image_load` (subset of the backend `ImageInfo`
 *  — only the fields the auto-mount flow reads). */
export interface ImageLoadPartition {
  id: number;
  kind: string;
  label: string | null;
}

/** Full result of `image_load`: the (stable) image id plus its detected
 *  partitions. Same command as `imageLoad`, typed for callers that need the
 *  partition list (e.g. auto-mount). Extra `ImageInfo` fields are ignored. */
export interface ImageLoadDetail {
  id: string;
  partitions: ImageLoadPartition[];
}

export function imageLoadDetail(args: ImageLoadArgs): Promise<ImageLoadDetail> {
  return invokeCmd<ImageLoadDetail>('image_load', args);
}

/** Transitional full-shape loader for callers that own their local `ImageInfo`
 *  view model. Keeps `image_load` behind the IPC boundary without forcing all
 *  image metadata types into this RAID-focused module in one refactor. */
export function imageLoadFull<T>(args: ImageLoadArgs): Promise<T> {
  return invokeCmd<T>('image_load', args);
}

/** Event emitted by `image_export_raw` for each written chunk (and an initial
 *  0-byte tick). Filter on `task_id` to attribute it to a specific export. */
export const IMAGE_EXPORT_PROGRESS_EVENT = 'image-export-progress';

/** Payload of {@link IMAGE_EXPORT_PROGRESS_EVENT}. Mirrors the Rust
 *  `ImageExportProgress` serde struct (`commands::image`). */
export interface ImageExportProgress {
  task_id: string;
  bytes_done: number;
  bytes_total: number;
}

export interface ImageExportResult {
  bytes_written: number;
  bad_regions: Array<{ offset: number; len: number }>;
  synthetic_bytes: number;
  read_faults_detected: boolean;
}

/** Stream a loaded image — real OR an assembled (reconstructed) RAID device —
 *  to a raw `.dd`/`.img` file at `destination`, returning total bytes written.
 *  Emits throttled {@link IMAGE_EXPORT_PROGRESS_EVENT} updates and aborts
 *  (with a retained partial file) when `task_cancel({ taskId })` is called. */
export function imageExportRaw(args: {
  imageId: string;
  destination: string;
  taskId: string;
  readFaultPolicy?: 'strict' | 'zeroFillAndReport';
  /** Preserve source bytes by default; optionally project unlocked partitions as plaintext. */
  mode?: 'source' | 'unlockedPartitions';
}): Promise<ImageExportResult> {
  return invokeCmd<ImageExportResult>('image_export_raw', args);
}

export function imageBadRegions(args: { imageId: string }): Promise<Omit<ImageExportResult, 'bytes_written'>> {
  return invokeCmd<Omit<ImageExportResult, 'bytes_written'>>('image_bad_regions', args);
}
