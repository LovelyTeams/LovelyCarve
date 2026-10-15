
import { tr } from '../../../composables/useLocale';
import { computed, onMounted, onScopeDispose, ref, watch } from 'vue';
import { open } from '@tauri-apps/plugin-dialog';
import { carveRecover, type CarveRecoveryResult, type CarveScanSnapshot } from '../../../lib/ipc/carve';
import { onTaskProgress, taskCancel, type BackendTaskProgressEvent } from '../../../lib/ipc/taskRuntime';
import { notifyError, notifyInfo, notifySuccess } from '../../../composables/useToast';
export function useCarveRecovery(options: { imageId: () => string; scan: () => CarveScanSnapshot | null; checkedIds: () => ReadonlySet<string> }) {
  const recovering = ref(false); const recoveryTaskId = ref<string | null>(null);
  const recoveryProgress = ref<BackendTaskProgressEvent | null>(null); const recoveryResult = ref<CarveRecoveryResult | null>(null);
  let disposed = false; let contextGeneration = 0; let unlisten: (() => void) | null = null;
  const recoveryPercent = computed(() => {
    const progress = recoveryProgress.value;
    if (!progress?.total) return 0;
    return Math.min(100, (progress.done / progress.total) * 100);
  });
  function makeTaskId(): string {
    return typeof crypto.randomUUID === 'function'
      ? crypto.randomUUID()
      : `carve-recovery-${Date.now()}-${Math.random().toString(16).slice(2)}`;
  }

  async function chooseRecoveryDestination(artifactIds: string[] | null) {
    const scan = options.scan();
    if (!scan || disposed || recovering.value) return;
    const scanId = scan.scan_id;
    const generation = contextGeneration;
    recovering.value = true;
    try {
      const picked = await open({ directory: true, multiple: false, title: tr("选择雕刻文件恢复目录") });
      if (!picked || Array.isArray(picked) || disposed || generation !== contextGeneration) return;
      const taskId = makeTaskId();
      recoveryTaskId.value = taskId;
      recoveryProgress.value = null;
      recoveryResult.value = null;
      const result = await carveRecover({
        scanId,
        options: {
          artifact_ids: artifactIds,
          destination: picked,
          task_id: taskId,
        },
      });
      if (disposed || generation !== contextGeneration) return;
      recoveryResult.value = result;
      if (result.failed > 0 || result.report_error) {
        notifyInfo(tr("雕刻恢复完成但有警告"), tr("成功 {0} 个，失败 {1} 个", { 0: result.recovered, 1: result.failed }));
      } else {
        notifySuccess(tr("雕刻恢复完成"), tr("已恢复 {0} 个文件并生成 DFXML 报告", { 0: result.recovered }));
      }
    } catch (error) {
      if (!disposed) notifyError(tr("雕刻恢复失败"), String(error));
    } finally {
      if (!disposed && generation === contextGeneration) {
        recovering.value = false; recoveryTaskId.value = null; recoveryProgress.value = null;
      }
    }
  }

  function recoverSelected() {
    void chooseRecoveryDestination([...options.checkedIds()]);
  }

  function recoverAll() {
    void chooseRecoveryDestination(null);
  }

  function recoverOne(id: string) {
    void chooseRecoveryDestination([id]);
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
      .catch(error => { if (!disposed) notifyError(tr("订阅雕刻恢复进度失败"), String(error)); });
  });
  onScopeDispose(() => { disposed = true; contextGeneration++; unlisten?.(); unlisten = null; });
  return { recovering, recoveryProgress, recoveryResult, recoveryPercent, recoverSelected, recoverAll, recoverOne, cancelRecovery };
}
