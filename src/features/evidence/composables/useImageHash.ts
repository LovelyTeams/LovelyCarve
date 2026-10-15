
import { tr } from '../../../composables/useLocale';
import { reactive, computed, type ComputedRef } from 'vue';
import { notifyError, notifySuccess } from '../../../composables/useToast';
import { imageHashCompute } from '../../../lib/ipc/image';
import { taskCancel } from '../../../lib/ipc/taskRuntime';
import { onEvent, type UnlistenFn } from '../../../lib/ipc/events';

// Module-scoped registry of in-flight source-image hash jobs. Pattern
// mirrors useBitlockerUnlocks: the backend task runs on tokio's blocking
// pool, the listener is owned here rather than in any component, so
// closing the hash panel or switching views never cancels the job.

export type HashAlgo = 'md5' | 'sha1' | 'sha256' | 'sm3';

export const ALL_HASH_ALGOS: readonly HashAlgo[] = ['md5', 'sha1', 'sha256', 'sm3'] as const;

export interface HashResult {
  md5: string | null;
  sha1: string | null;
  sha256: string | null;
  sm3: string | null;
  bad_regions: Array<{ offset: number; len: number }>;
  synthetic_bytes: number;
  read_faults_detected: boolean;
}

export interface ImageHashJob {
  imageId: string;
  taskId: string;
  algorithms: HashAlgo[];
  bytesDone: number;
  bytesTotal: number;
  cancelling: boolean;
}

interface HashProgressEvent {
  task_id: string;
  bytes_done: number;
  bytes_total: number;
}

const jobs = reactive(new Map<string, ImageHashJob>());
const results = reactive(new Map<string, HashResult>());
const listeners = new Map<string, UnlistenFn>();
let generation = 0;
export function disposeImageHashJobs() {
  generation++;
  for (const stop of listeners.values()) stop();
  listeners.clear(); jobs.clear(); results.clear();
}

function newTaskId(): string {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
    return crypto.randomUUID();
  }
  return `hash-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
}

export function useImageHashJob(imageId: () => string): ComputedRef<ImageHashJob | undefined> {
  return computed(() => jobs.get(imageId()));
}

export function useImageHashResult(imageId: () => string): ComputedRef<HashResult | undefined> {
  return computed(() => results.get(imageId()));
}

/** Reset cached results for an image — call when the image is unloaded or
 *  reloaded so a stale hash from a previous load doesn't surface. */
export function clearImageHashResult(imageId: string) {
  results.delete(imageId);
}

export async function startImageHash(
  imageId: string,
  algorithms: HashAlgo[],
): Promise<boolean> {
  if (jobs.has(imageId)) {
    notifyError(tr("哈希计算失败"), tr("此镜像已有哈希任务在进行中"));
    return false;
  }
  if (algorithms.length === 0) {
    notifyError(tr("哈希计算失败"), tr("请至少选择一种算法"));
    return false;
  }
  const taskId = newTaskId();
  const epoch = generation;
  jobs.set(imageId, {
    imageId,
    taskId,
    algorithms: [...algorithms],
    bytesDone: 0,
    bytesTotal: 0,
    cancelling: false,
  });

  try {
    const unlisten = await onEvent<HashProgressEvent>('image-hash-progress', event => {
      if (epoch !== generation || event.payload.task_id !== taskId) return;
      const job = jobs.get(imageId);
      if (!job) return;
      job.bytesDone = event.payload.bytes_done;
      job.bytesTotal = event.payload.bytes_total;
    });
    if (epoch !== generation) { unlisten(); return false; }
    listeners.set(imageId, unlisten);

    const result = await imageHashCompute<HashResult>({
      imageId,
      algorithms,
      taskId,
      readFaultPolicy: 'zeroFillAndReport',
    });
    if (epoch !== generation) return true;
    // Merge with any previous result so the user can re-run a single
    // algorithm without dropping the other three.
    const prev = results.get(imageId) ?? {
      md5: null,
      sha1: null,
      sha256: null,
      sm3: null,
      bad_regions: [],
      synthetic_bytes: 0,
      read_faults_detected: false,
    };
    results.set(imageId, {
      md5: result.md5 ?? prev.md5,
      sha1: result.sha1 ?? prev.sha1,
      sha256: result.sha256 ?? prev.sha256,
      sm3: result.sm3 ?? prev.sm3,
      bad_regions: result.bad_regions,
      synthetic_bytes: result.synthetic_bytes,
      read_faults_detected: result.read_faults_detected,
    });
    notifySuccess(tr("源盘哈希计算完成"), `${algorithms.map(a => a.toUpperCase()).join(' / ')}`);
    return true;
  } catch (err) {
    if (epoch === generation) notifyError(tr("源盘哈希计算失败"), String(err));
    return false;
  } finally {
    if (epoch === generation) {
      jobs.delete(imageId);
      const u = listeners.get(imageId);
      if (u) {
        u();
        listeners.delete(imageId);
      }
    }
  }
}

export async function cancelImageHash(imageId: string): Promise<void> {
  const job = jobs.get(imageId);
  if (!job) return;
  job.cancelling = true;
  try {
    await taskCancel({ taskId: job.taskId });
  } catch (err) {
    notifyError(tr("取消失败"), String(err));
    job.cancelling = false;
  }
}
