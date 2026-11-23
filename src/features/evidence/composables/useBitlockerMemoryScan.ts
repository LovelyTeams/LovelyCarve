// 在内存镜像中查找 BitLocker VMK 候选。扫描是后台任务（任务中心可取消），这里
// 按检材与分区登记进度，关闭解锁对话框后再打开仍能看到；用候选解锁走统一的
// `encrypted_volume_unlock`。
import { computed, reactive, type ComputedRef } from 'vue';
import { tr } from '../../../composables/useLocale';
import { notifyError } from '../../../composables/useToast';
import { BITLOCKER_SCAN_PROGRESS_EVENT, bitlockerScanMemory, type BitlockerScanProgress, type BitlockerVmkScanResult } from '../../../lib/ipc/encrypted';
import { onEvent, type UnlistenFn } from '../../../lib/ipc/events';
import type { ImageId } from '../../../lib/ipc/ids';
import { taskCancel } from '../../../lib/ipc/taskRuntime';

export interface BitlockerScanJob {
  imageId: ImageId;
  partitionId: number;
  taskId: string;
  bytesDone: number;
  bytesTotal: number;
  cancelling: boolean;
}

const scans = reactive(new Map<string, BitlockerScanJob>());
const listeners = new Map<string, UnlistenFn>();
let generation = 0;

function scanKey(imageId: ImageId, partitionId: number): string {
  return `${imageId}:${partitionId}`;
}

export function useBitlockerScanJob(imageId: () => ImageId, partitionId: () => number | undefined): ComputedRef<BitlockerScanJob | undefined> {
  return computed(() => {
    const partition = partitionId();
    return partition === undefined ? undefined : scans.get(scanKey(imageId(), partition));
  });
}

/** Release UI subscriptions without cancelling a scan already started. */
export function disposeBitlockerMemoryScans(): void {
  generation++;
  for (const stop of listeners.values()) stop();
  listeners.clear();
  scans.clear();
}

/** Scan `memoryPath` for VMK candidates against a partition; `null` on error or cancellation. */
export async function startBitlockerMemoryScan(imageId: ImageId, partitionId: number, memoryPath: string): Promise<BitlockerVmkScanResult | null> {
  const key = scanKey(imageId, partitionId);
  if (scans.has(key)) {
    notifyError(tr("内存扫描失败"), tr("此分区已有扫描任务在进行中"));
    return null;
  }
  const taskId = crypto.randomUUID();
  const epoch = generation;
  scans.set(key, { imageId, partitionId, taskId, bytesDone: 0, bytesTotal: 0, cancelling: false });
  try {
    const unlisten = await onEvent<BitlockerScanProgress>(BITLOCKER_SCAN_PROGRESS_EVENT, event => {
      if (epoch !== generation || event.payload.task_id !== taskId) return;
      const job = scans.get(key);
      if (!job) return;
      job.bytesDone = event.payload.bytes_done;
      job.bytesTotal = event.payload.bytes_total;
    });
    if (epoch !== generation) { unlisten(); return null; }
    listeners.set(key, unlisten);
    return await bitlockerScanMemory({ imageId, partitionId, memoryPath, taskId });
  } catch (error) {
    if (epoch === generation) notifyError(tr("内存扫描失败"), String(error));
    return null;
  } finally {
    if (epoch === generation) {
      scans.delete(key);
      listeners.get(key)?.();
      listeners.delete(key);
    }
  }
}

export async function cancelBitlockerMemoryScan(imageId: ImageId, partitionId: number): Promise<void> {
  const job = scans.get(scanKey(imageId, partitionId));
  if (!job || job.cancelling) return;
  job.cancelling = true;
  try {
    await taskCancel({ taskId: job.taskId });
  } catch (error) {
    notifyError(tr("取消失败"), String(error));
    job.cancelling = false;
  }
}
