// Streams a loaded image — in practice an assembled (reconstructed) RAID
// device — to a raw `.dd`/`.img` file on disk, with a live progress toast and
// a cancel button. Pattern mirrors `useImageHash` (a per-image long task driven
// by an `image-*-progress` event + `task_cancel`) crossed with the
// persistent-toast export UX in `DiskFileList.runExportDir`.
//
// Job state is module-scoped (keyed by image id) so it survives KeepAlive
// panel remounts and a second export of the same image is refused while one is
// in flight — the backend `image_export_raw` is the single writer.
import { reactive } from 'vue';
import { save } from '@tauri-apps/plugin-dialog';
import { bitlockerExportDecryptedImage } from '../../../lib/ipc/bitlocker';
import { imageExportRaw, IMAGE_EXPORT_PROGRESS_EVENT, type ImageExportResult, type ImageExportProgress } from '../../../lib/ipc/image';
import { taskCancel } from '../../../lib/ipc/taskRuntime';
import { onEvent, type UnlistenFn } from '../../../lib/ipc/events';
import { tr } from '../../../composables/useLocale';
import { notifyError, pushPersistentToast } from '../../../composables/useToast';
import { formatBytes } from '../../../utils/format';
import { evidenceContext } from './evidenceContext';

export interface ImageExportJob {
  imageId: string;
  taskId: string;
  destination: string;
  bytesDone: number;
  bytesTotal: number;
  startedAtMs: number;
  lastProgressAtMs: number;
  lastBytesDone: number;
  bytesPerSecond: number;
  cancelling: boolean;
}

const jobs = reactive(new Map<string, ImageExportJob>());
const choices = new Map<string, symbol>();
const listeners = new Map<string, UnlistenFn>();
let generation = 0;
export function disposeImageExportJobs() {
  generation++;
  for (const stop of listeners.values()) stop();
  listeners.clear(); choices.clear(); jobs.clear();
}

type Exporter = (args: {
  imageId: string;
  destination: string;
  taskId: string;
}) => Promise<number | ImageExportResult>;

function newTaskId(): string {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
    return crypto.randomUUID();
  }
  return `export-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
}

/** Percent (0..100) for the toast progress bar; 0 until a total is known. */
function percent(job: ImageExportJob): number {
  return job.bytesTotal > 0 ? (job.bytesDone / job.bytesTotal) * 100 : 0;
}

function exportSpeedText(job: ImageExportJob, fallbackDone = job.bytesDone): string {
  const elapsedSeconds = Math.max((Date.now() - job.startedAtMs) / 1000, 0.001);
  const averageBytesPerSecond = fallbackDone / elapsedSeconds;
  const bytesPerSecond = job.bytesPerSecond > 0 ? job.bytesPerSecond : averageBytesPerSecond;
  return `${formatBytes(bytesPerSecond, 1)}/s`;
}

/** Prompt for a destination, then stream `imageId` to it as RAW. Resolves
 *  `true` on a completed write, `false` on cancel-dialog / refusal / failure.
 *  Progress + cancel live in a persistent toast owned here. */
async function startImageExport(opts: {
  imageId: string;
  defaultFileName: string;
  title?: string;
  runningTitle?: string;
  doneTitle?: string;
  duplicateMessage?: string;
  exporter?: Exporter;
  mode?: 'source' | 'unlockedPartitions';
}): Promise<boolean> {
  if (jobs.has(opts.imageId) || choices.has(opts.imageId)) {
    notifyError(tr("导出失败"), opts.duplicateMessage ?? tr("此镜像已有导出任务在进行中"));
    return false;
  }

  const epoch = generation;
  const context = evidenceContext();
  const choice = Symbol(opts.imageId);
  choices.set(opts.imageId, choice);
  let destination: string | null;
  try {
    destination = await save({
      title: opts.title ?? tr("导出重组镜像为 RAW"), defaultPath: opts.defaultFileName,
      filters: [{ name: tr("RAW 磁盘镜像"), extensions: ['dd', 'img', 'raw'] }],
    });
  } finally {
    if (choices.get(opts.imageId) === choice) choices.delete(opts.imageId);
  }
  if (!destination || epoch !== generation || context !== evidenceContext()) return false;

  const startedAtMs = Date.now();
  // Share the same reactive job between progress handlers and registry identity checks.
  const job = reactive<ImageExportJob>({
    imageId: opts.imageId,
    taskId: newTaskId(),
    destination,
    bytesDone: 0,
    bytesTotal: 0,
    startedAtMs,
    lastProgressAtMs: startedAtMs,
    lastBytesDone: 0,
    bytesPerSecond: 0,
    cancelling: false,
  });
  jobs.set(opts.imageId, job);

  const toast = pushPersistentToast({
    kind: 'info',
    title: opts.runningTitle ?? tr("导出重组镜像中…"),
    detail: tr("{0} · 准备中", { 0: destination }),
    progress: 0,
    action: { get label() { return tr("取消"); }, onClick: () => void cancelImageExport(opts.imageId) },
  });

  // Wire the progress listener BEFORE invoking so the backend's initial
  // 0-byte tick isn't missed.
  let unlisten: UnlistenFn | null = null;
  try {
    unlisten = await onEvent<ImageExportProgress>(IMAGE_EXPORT_PROGRESS_EVENT, event => {
      if (epoch !== generation || event.payload.task_id !== job.taskId) return;
      const now = Date.now();
      const deltaBytes = event.payload.bytes_done - job.lastBytesDone;
      const deltaSeconds = (now - job.lastProgressAtMs) / 1000;
      if (deltaBytes > 0 && deltaSeconds > 0) {
        const instantBytesPerSecond = deltaBytes / deltaSeconds;
        job.bytesPerSecond =
          job.bytesPerSecond > 0
            ? job.bytesPerSecond * 0.65 + instantBytesPerSecond * 0.35
            : instantBytesPerSecond;
      }
      job.bytesDone = event.payload.bytes_done;
      job.bytesTotal = event.payload.bytes_total;
      job.lastProgressAtMs = now;
      job.lastBytesDone = job.bytesDone;
      const tail = job.cancelling ? tr(" · 取消中…") : '';
      toast.update({
        detail: `${destination} · ${formatBytes(job.bytesDone)} / ${formatBytes(job.bytesTotal)} · ${exportSpeedText(job)}${tail}`,
        progress: percent(job),
      });
    });
    if (epoch === generation) listeners.set(job.taskId, unlisten);
    else { unlisten(); unlisten = null; }
  } catch (err) {
    // Losing live progress shouldn't abort the export itself.
    notifyError(tr("订阅导出进度失败"), String(err));
  }

  if (epoch !== generation || context !== evidenceContext()) {
    unlisten?.(); listeners.delete(job.taskId);
    if (jobs.get(opts.imageId) === job) jobs.delete(opts.imageId);
    toast.dismiss(); return false;
  }
  try {
    // The task center lists the export under `job.taskId`; cancelling there
    // ends this call like the toast's cancel button.
    const exporter = opts.exporter ?? imageExportRaw;
    const outcome = await exporter({
      imageId: opts.imageId,
      destination,
      taskId: job.taskId,
      ...(opts.mode ? { mode: opts.mode } : {}),
    });
    if (epoch !== generation) return true;
    const written = typeof outcome === 'number' ? outcome : outcome.bytes_written;
    const faultDetail = typeof outcome !== 'number' && outcome.read_faults_detected
      ? tr(" · 警告：含 {0} 补零字节（{1} 个坏区）", { 0: formatBytes(outcome.synthetic_bytes), 1: outcome.bad_regions.length })
      : '';
    toast.update({
      kind: faultDetail ? 'info' : 'success',
      title: faultDetail ? tr("导出完成，但包含补零字节") : (opts.doneTitle ?? tr("重组镜像导出完成")),
      detail: tr("{0} · {1} · 平均 {2}{3}", { 0: destination, 1: formatBytes(written), 2: exportSpeedText(job, written), 3: faultDetail }),
      action: undefined,
      progress: 100,
    });
    toast.dismiss(3600);
    return true;
  } catch (err) {
    // The backend returns Err("export cancelled") with a retained partial
    // file when cancelled; treat that as an info tail, not an error.
    if (epoch !== generation) return false;
    const cancelled = job.cancelling || /cancel/i.test(String(err));
    toast.update({
      kind: cancelled ? 'info' : 'error',
      title: cancelled ? tr("导出已取消") : tr("导出失败"),
      detail: cancelled ? tr("{0} · 已取消（保留部分文件）", { 0: destination }) : String(err),
      action: undefined,
      progress: undefined,
    });
    toast.dismiss(4200);
    return false;
  } finally {
    if (jobs.get(opts.imageId) === job) jobs.delete(opts.imageId);
    if (listeners.delete(job.taskId)) unlisten?.();
  }
}

async function startBitlockerDecryptedImageExport(opts: {
  imageId: string;
  partitionId: number;
  defaultFileName: string;
}): Promise<boolean> {
  return startImageExport({
    imageId: opts.imageId,
    defaultFileName: opts.defaultFileName,
    title: tr("导出 BitLocker 解密后完整镜像"),
    runningTitle: tr("导出解密后完整镜像中…"),
    doneTitle: tr("解密后完整镜像导出完成"),
    exporter: args =>
      bitlockerExportDecryptedImage({
        ...args,
        partitionId: opts.partitionId,
      }),
  });
}

/** Request cancellation of an in-flight export. Idempotent. */
async function cancelImageExport(imageId: string): Promise<void> {
  const job = jobs.get(imageId);
  if (job) await cancelExportJob(job);
}

async function cancelExportJob(job: ImageExportJob): Promise<void> {
  if (job.cancelling) return;
  job.cancelling = true;
  try {
    await taskCancel({ taskId: job.taskId });
  } catch (err) {
    notifyError(tr("取消失败"), String(err));
    job.cancelling = false;
  }
}

export function useImageExport() {
  return {
    startImageExport,
    startBitlockerDecryptedImageExport,
    cancelImageExport,
    /** In-flight job for an image, or undefined. Reads the module-scoped
     *  reactive map, so calling it in a template / computed is tracked. */
    jobFor: (imageId: string | null | undefined): ImageExportJob | undefined =>
      imageId ? jobs.get(imageId) : undefined,
  };
}
