
import { tr } from '../../../composables/useLocale';
import { reactive, computed, type ComputedRef } from 'vue';
import { notifyError, notifySuccess } from '../../../composables/useToast';
import { bitlockerScanMemory, bitlockerUnlock, bitlockerUnlockWithVmk } from '../../../lib/ipc/bitlocker';
import { taskCancel } from '../../../lib/ipc/taskRuntime';
import { onEvent, type UnlistenFn } from '../../../lib/ipc/events';

// Module-scoped registries of in-flight BitLocker operations. Living outside
// any component lets the dump/scan survive popover close / view changes —
// the backend tokio task continues regardless; this just keeps the UI's
// observable progress alive so any component can pick it back up.

import type { BitlockerKeyCredential, BitlockerUnlockMode, BitlockerUnlockJob, BitlockerScanJob, BitlockerVmkScanResult } from '../model/bitlocker';

interface UnlockProgressEvent {
  task_id: string;
  bytes_done: number;
  bytes_total: number;
}

interface ScanProgressEvent {
  task_id: string;
  bytes_done: number;
  bytes_total: number;
}

const unlockJobs = reactive(new Map<string, BitlockerUnlockJob>());
const unlockListeners = new Map<string, UnlistenFn>();

const scanJobs = reactive(new Map<string, BitlockerScanJob>());
const scanListeners = new Map<string, UnlistenFn>();

function jobKey(imageId: string, partitionId: number): string {
  return `${imageId}:${partitionId}`;
}

function newTaskId(prefix: string): string {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
    return crypto.randomUUID();
  }
  return `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
}

export function useBitlockerJob(
  imageId: () => string,
  partitionId: () => number,
): ComputedRef<BitlockerUnlockJob | undefined> {
  return computed(() => unlockJobs.get(jobKey(imageId(), partitionId())));
}

export function useBitlockerScanJob(
  imageId: () => string,
  partitionId: () => number,
): ComputedRef<BitlockerScanJob | undefined> {
  return computed(() => scanJobs.get(jobKey(imageId(), partitionId())));
}

export function bitlockerJobSnapshot(imageId: string, partitionId: number): BitlockerUnlockJob | undefined {
  return unlockJobs.get(jobKey(imageId, partitionId));
}

interface BitlockerSnapshotWorkflow {
  context: () => unknown;
  unlocked: (imageId: string, partitionId: number) => void;
}
let snapshotWorkflow: BitlockerSnapshotWorkflow | null = null;
let generation = 0;
export function registerBitlockerSnapshotWorkflow(value: BitlockerSnapshotWorkflow): () => void {
  snapshotWorkflow = value;
  return () => { if (snapshotWorkflow === value) snapshotWorkflow = null; };
}
/** Release UI subscriptions without cancelling an already-started backend operation. */
export function disposeBitlockerJobs(): void {
  generation++;
  for (const stop of unlockListeners.values()) stop();
  for (const stop of scanListeners.values()) stop();
  unlockListeners.clear(); scanListeners.clear(); unlockJobs.clear(); scanJobs.clear();
  snapshotWorkflow = null;
}

async function runUnlock(
  imageId: string,
  partitionId: number,
  mode: BitlockerUnlockMode,
  payload: Record<string, unknown>,
  command: 'bitlocker_unlock' | 'bitlocker_unlock_with_vmk',
  successLabel: string,
): Promise<boolean> {
  const key = jobKey(imageId, partitionId);
  if (unlockJobs.has(key)) {
    notifyError(tr("解锁失败"), tr("此分区已有解锁任务在进行中"));
    return false;
  }
  const taskId = newTaskId('bde');
  const epoch = generation;
  const workflow = snapshotWorkflow;
  const context = workflow?.context();
  unlockJobs.set(key, {
    imageId,
    partitionId,
    taskId,
    mode,
    bytesDone: 0,
    bytesTotal: 0,
    cancelling: false,
  });

  try {
    const unlisten = await onEvent<UnlockProgressEvent>('bitlocker-unlock-progress', event => {
      if (epoch !== generation || event.payload.task_id !== taskId) return;
      const job = unlockJobs.get(key);
      if (!job) return;
      job.bytesDone = event.payload.bytes_done;
      job.bytesTotal = event.payload.bytes_total;
    });
    if (epoch !== generation) { unlisten(); return false; }
    unlockListeners.set(key, unlisten);

    if (command === 'bitlocker_unlock') {
      await bitlockerUnlock({ imageId, partitionId, taskId, ...payload });
    } else {
      await bitlockerUnlockWithVmk({
        imageId,
        partitionId,
        taskId,
        vmkHex: String(payload.vmkHex ?? ''),
      });
    }
    if (epoch !== generation) return true;
    notifySuccess(tr("BitLocker 解锁成功"), successLabel);
    // 解锁后自动为该 NTFS 卷构建快照（即发即忘，不阻塞解锁返回）。
    if (workflow && workflow === snapshotWorkflow && context === workflow.context()) workflow.unlocked(imageId, partitionId);
    return true;
  } catch (err) {
    if (epoch === generation) notifyError(tr("BitLocker 解锁失败"), String(err));
    return false;
  } finally {
    if (epoch === generation) {
      unlockJobs.delete(key);
      const u = unlockListeners.get(key);
      if (u) {
        u();
        unlockListeners.delete(key);
      }
    }
  }
}

/** Recovery-key unlock. Sends `recoveryPassword` and leaves `password` null. */
export function startBitlockerRecoveryUnlock(
  imageId: string,
  partitionId: number,
  recoveryPassword: string,
): Promise<boolean> {
  return runUnlock(
    imageId,
    partitionId,
    'recovery',
    { recoveryPassword, password: null },
    'bitlocker_unlock',
    tr("Partition {0} 已通过恢复密钥解锁", { 0: partitionId }),
  );
}

/** User-password unlock. Sends `password` and leaves `recoveryPassword` null. */
export function startBitlockerPasswordUnlock(
  imageId: string,
  partitionId: number,
  password: string,
): Promise<boolean> {
  return runUnlock(
    imageId,
    partitionId,
    'password',
    { recoveryPassword: null, password },
    'bitlocker_unlock',
    tr("Partition {0} 已通过用户密码解锁", { 0: partitionId }),
  );
}

/** VMK unlock — used after a memory-image scan surfaces a candidate. */
export function startBitlockerVmkUnlock(
  imageId: string,
  partitionId: number,
  vmkHex: string,
): Promise<boolean> {
  return runUnlock(
    imageId,
    partitionId,
    'vmk',
    { vmkHex },
    'bitlocker_unlock_with_vmk',
    tr("Partition {0} 已通过 VMK 解锁", { 0: partitionId }),
  );
}

export function startBitlockerKeyUnlock(imageId: string, partitionId: number, credential: BitlockerKeyCredential): Promise<boolean> {
  if (credential.kind === 'vmk') return startBitlockerVmkUnlock(imageId, partitionId, credential.vmkHex);
  const payload = credential.kind === 'startup' ? { startupKeyPath: credential.startupKeyPath }
    : credential.kind === 'fvek' ? { fvekHex: credential.fvekHex, tweakHex: credential.tweakHex || null } : {};
  return runUnlock(imageId, partitionId, 'key', payload, 'bitlocker_unlock', tr("分区 {0} 已解锁", { 0: partitionId }));
}

export async function cancelBitlockerUnlock(imageId: string, partitionId: number): Promise<void> {
  const key = jobKey(imageId, partitionId);
  const job = unlockJobs.get(key);
  if (!job) return;
  job.cancelling = true;
  try {
    await taskCancel({ taskId: job.taskId });
  } catch (err) {
    notifyError(tr("取消失败"), String(err));
    job.cancelling = false;
  }
}

/** Scan a memory dump for BitLocker VMK candidates against a partition. The
 *  scan job is registered in `scanJobs` for the duration of the call so the
 *  popover can render a progress bar. Returns the parsed candidate list (or
 *  `null` on error / cancellation). */
export async function startBitlockerMemoryScan(
  imageId: string,
  partitionId: number,
  memoryPath: string,
): Promise<BitlockerVmkScanResult | null> {
  const key = jobKey(imageId, partitionId);
  if (scanJobs.has(key)) {
    notifyError(tr("内存扫描失败"), tr("此分区已有扫描任务在进行中"));
    return null;
  }
  const taskId = newTaskId('bde-scan');
  const epoch = generation;
  scanJobs.set(key, {
    imageId,
    partitionId,
    taskId,
    bytesDone: 0,
    bytesTotal: 0,
    cancelling: false,
  });

  try {
    const unlisten = await onEvent<ScanProgressEvent>('bitlocker-scan-progress', event => {
      if (epoch !== generation || event.payload.task_id !== taskId) return;
      const job = scanJobs.get(key);
      if (!job) return;
      job.bytesDone = event.payload.bytes_done;
      job.bytesTotal = event.payload.bytes_total;
    });
    if (epoch !== generation) { unlisten(); return null; }
    scanListeners.set(key, unlisten);

    const result = await bitlockerScanMemory<BitlockerVmkScanResult>({
      imageId,
      partitionId,
      memoryPath,
      taskId,
    });
    return result;
  } catch (err) {
    if (epoch === generation) notifyError(tr("内存扫描失败"), String(err));
    return null;
  } finally {
    if (epoch === generation) {
      scanJobs.delete(key);
      const u = scanListeners.get(key);
      if (u) {
        u();
        scanListeners.delete(key);
      }
    }
  }
}

export async function cancelBitlockerMemoryScan(imageId: string, partitionId: number): Promise<void> {
  const key = jobKey(imageId, partitionId);
  const job = scanJobs.get(key);
  if (!job) return;
  job.cancelling = true;
  try {
    await taskCancel({ taskId: job.taskId });
  } catch (err) {
    notifyError(tr("取消失败"), String(err));
    job.cancelling = false;
  }
}
