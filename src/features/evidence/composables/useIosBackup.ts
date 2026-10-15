
import { tr } from '../../../composables/useLocale';
import { onBeforeUnmount, ref, watch } from 'vue';
import type { ImageInfo } from '../../../lib/ipc/image';
import { imageUnload } from '../../../lib/ipc/image';
import { iosBackupAudit, iosBackupUnlock, type IosBackupFinding } from '../../../lib/ipc/iosBackup';
import { taskCancel } from '../../../lib/ipc/taskRuntime';
import { notifyError } from '../../../composables/useToast';

export function useIosBackup(info: () => ImageInfo, openImage: (path: string) => void) {
  const password = ref('');
  const error = ref('');
  const busy = ref<'unlock' | 'audit' | null>(null);
  const cancelling = ref(false);
  const findings = ref<IosBackupFinding[] | null>(null);
  let generation = 0;
  let taskId: string | undefined;

  watch(() => info().id, () => {
    generation += 1;
    password.value = '';
    error.value = '';
    findings.value = null;
    busy.value = null;
    cancelling.value = false;
    taskId = undefined;
  });

  async function run(operation: 'unlock' | 'audit') {
    if (busy.value) return;
    const captured = generation;
    const imageId = info().id;
    const currentTaskId = crypto.randomUUID();
    taskId = currentTaskId;
    busy.value = operation;
    error.value = '';
    cancelling.value = false;
    const request = { imageId, password: password.value, taskId: currentTaskId };
    password.value = '';
    try {
      if (operation === 'unlock') {
        const image = await iosBackupUnlock(request);
        if (captured === generation && !cancelling.value) openImage(image.path);
        else await imageUnload({ imageId: image.id });
      } else {
        const result = await iosBackupAudit({ imageId, taskId: currentTaskId });
        if (captured === generation && !cancelling.value) findings.value = result;
      }
    } catch (cause) {
      if (captured === generation) error.value = String(cause);
      else notifyError(operation === 'unlock' ? tr("iOS 备份解锁失败") : tr("iOS 备份检查失败"), String(cause));
    } finally {
      request.password = '';
      if (captured === generation) { busy.value = null; taskId = undefined; }
    }
  }

  async function cancel() {
    if (!taskId) return;
    const captured = generation;
    cancelling.value = true;
    try { await taskCancel({ taskId }); }
    catch (cause) {
      if (captured === generation) { error.value = String(cause); cancelling.value = false; }
    }
  }

  onBeforeUnmount(() => { generation += 1; password.value = ''; });
  return { password, error, busy, cancelling, findings, run, cancel };
}
