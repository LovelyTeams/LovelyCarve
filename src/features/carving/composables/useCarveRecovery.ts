
import { tr } from '../../../composables/useLocale';
import { computed, onMounted, onScopeDispose, ref, watch } from 'vue';
import { open } from '@tauri-apps/plugin-dialog';
import { IpcCommandError } from '../../../lib/ipc/client';
import { carveRecover, type CarveRecoveryResult, type CarveScanSnapshot } from '../../../lib/ipc/carve';
import { onTaskProgress, taskCancel, type BackendTaskProgressEvent } from '../../../lib/ipc/taskRuntime';
import { notifyError, notifyInfo, notifySuccess } from '../../../composables/useToast';

/** Carved artifacts and filesystem records to export; `null` artifacts means every carved result. */
export interface RecoverySelection { artifactIds: string[] | null; fileIds: string[] }

export function useCarveRecovery(options: { imageId: () => string; scan: () => CarveScanSnapshot | null }) {
  const recovering = ref(false); const recoveryTaskId = ref<string | null>(null);
  const recoveryProgress = ref<BackendTaskProgressEvent | null>(null); const recoveryResult = ref<CarveRecoveryResult | null>(null);
  let disposed = false; let contextGeneration = 0; let unlisten: (() => void) | null = null;
  const recoveryPercent = computed(() => {
    const progress = recoveryProgress.value;
    if (!progress?.total) return 0;
    return Math.min(100, (progress.done / progress.total) * 100);
  });
  /** Files recovered and failed across carved artifacts and filesystem records. */
  const recoveryTotals = computed(() => {
    const result = recoveryResult.value;
    if (!result) return null;
    return { recovered: result.recovered + (result.files?.recovered ?? 0), failed: result.failed + (result.files?.failed ?? 0) };
  });
  function makeTaskId(): string {
    return typeof crypto.randomUUID === 'function'
      ? crypto.randomUUID()
      : `carve-recovery-${Date.now()}-${Math.random().toString(16).slice(2)}`;
  }

  async function recover(selection: RecoverySelection) {
    const scan = options.scan();
    if (!scan || disposed || recovering.value) return;
    if (selection.artifactIds?.length === 0 && selection.fileIds.length === 0) return;
    const scanId = scan.scan_id;
    const generation = contextGeneration;
    recovering.value = true;
    try {
      const picked = await open({ directory: true, multiple: false, title: tr("选择文件恢复目录") });
      if (!picked || Array.isArray(picked) || disposed || generation !== contextGeneration) return;
      const taskId = makeTaskId();
      recoveryTaskId.value = taskId;
      recoveryProgress.value = null;
      recoveryResult.value = null;
      const result = await carveRecover({
        scanId,
        options: {
          artifact_ids: selection.artifactIds,
          file_ids: selection.fileIds,
          destination: picked,
          task_id: taskId,
        },
      });
      if (disposed || generation !== contextGeneration) return;
      recoveryResult.value = result;
      const totals = recoveryTotals.value!;
      if (totals.failed > 0 || result.report_error || result.files?.read_faults.read_faults_detected) {
        notifyInfo(tr("文件恢复完成但有警告"), tr("成功 {0} 个，失败 {1} 个", { 0: totals.recovered, 1: totals.failed }));
      } else {
        notifySuccess(tr("文件恢复完成"), tr("已恢复 {0} 个文件并生成恢复报告", { 0: totals.recovered }));
      }
    } catch (error) {
      if (disposed) return;
      if (error instanceof IpcCommandError && error.code === 'cancelled') notifyInfo(tr("恢复已取消"), tr("已完成的文件保留在恢复目录中。"));
      else notifyError(tr("文件恢复失败"), String(error));
    } finally {
      if (!disposed && generation === contextGeneration) {
        recovering.value = false; recoveryTaskId.value = null; recoveryProgress.value = null;
      }
    }
  }

  async function cancelRecovery() {
    if (!recoveryTaskId.value) return;
    try {
      await taskCancel({ taskId: recoveryTaskId.value });
    } catch (error) {
      notifyError(tr("取消恢复失败"), String(error));
    }
  }

  watch([options.imageId, () => options.scan()?.scan_id], () => {
    contextGeneration++; recovering.value = false; recoveryTaskId.value = null; recoveryProgress.value = null; recoveryResult.value = null;
  }, { flush: 'sync' });
  onMounted(() => {
    void onTaskProgress(event => { if (!disposed && event.payload.task.id === recoveryTaskId.value) recoveryProgress.value = event.payload; })
      .then(cleanup => { if (disposed) cleanup(); else unlisten = cleanup; })
      .catch(error => { if (!disposed) notifyError(tr("订阅恢复进度失败"), String(error)); });
  });
  onScopeDispose(() => { disposed = true; contextGeneration++; unlisten?.(); unlisten = null; });
  return { recovering, recoveryProgress, recoveryResult, recoveryTotals, recoveryPercent, recover, cancelRecovery };
}
