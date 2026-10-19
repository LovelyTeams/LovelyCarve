
import { tr } from '../../../composables/useLocale';
import { notifyError, notifyInfo, notifySuccess } from '../../../composables/useToast';
import { encryptedVolumeUnlock, type EncryptedUnlockOptions } from '../../../lib/ipc/encrypted';
import { imageUnload, type ImageInfo } from '../../../lib/ipc/image';
import { IpcCommandError } from '../../../lib/ipc/client';
import { evidenceContext } from './evidenceContext';
import { clearUnlockedImageInfo, rememberUnlockedImageInfo } from './unlockedImageInfo';

// The main window owns completion and source metadata, independently of the dialog.
const jobs = new Map<string, string>();
let generation = 0;
let updateImage: ((info: ImageInfo) => void) | null = null;
let openDerivedImage: ((path: string) => Promise<boolean>) | null = null;

export function registerEncryptedVolumeImageUpdater(updater: (info: ImageInfo) => void) {
  updateImage = updater;
  return () => { if (updateImage === updater) updateImage = null; };
}

export function registerEncryptedVolumeImageOpener(opener: (path: string) => Promise<boolean>) {
  openDerivedImage = opener;
  return () => { if (openDerivedImage === opener) openDerivedImage = null; };
}

export function disposeEncryptedVolumeUnlockJobs() {
  generation++;
  jobs.clear();
  clearUnlockedImageInfo();
}

/** Submit immediately; the task center lists the unlock under `taskId`, and
 *  metadata updates outlive the page. */
export function startEncryptedVolumeUnlock(info: ImageInfo, options: EncryptedUnlockOptions): boolean {
  const key = `${info.id}:${options.partitionId ?? 'whole'}${options.target === 'newImage' ? `:lv:${options.logicalVolumeIndex ?? 0}` : ''}`;
  if (jobs.has(key)) {
    options.password = '';
    options.masterKeyHex = undefined;
    notifyError(tr("解锁加密容器失败"), tr("此加密区域已有解锁任务，可在任务中心查看或取消"));
    return false;
  }
  const taskId = crypto.randomUUID();
  const epoch = generation;
  const context = evidenceContext();
  const updater = updateImage;
  const opener = openDerivedImage;
  const current = () => epoch === generation && context === evidenceContext();
  jobs.set(key, taskId);
  void (async () => {
    try {
      options.target ??= 'sourcePartition';
      // IPC may defer transport: the request carries its own copy of the
      // credentials, so clearing the local ones cannot change its payload.
      const request = encryptedVolumeUnlock({ imageId: info.id, options: { ...options }, taskId });
      options.password = '';
      const result = await request;
      if (!current()) {
        if (result.image.id !== info.id) await imageUnload({ imageId: result.image.id });
        return;
      }
      if (result.image.id === info.id) {
        rememberUnlockedImageInfo(result.image);
        updater?.(result.image);
      } else {
        try {
          if (!opener) throw new Error(tr("明文会话导航服务不可用"));
          if (!await opener(result.image.path)) {
            await imageUnload({ imageId: result.image.id });
            return;
          }
        } catch (error) {
          await imageUnload({ imageId: result.image.id });
          throw error;
        }
      }
      notifySuccess(tr("加密卷解锁成功"), result.cipher);
    } catch (error) {
      if (current() && !(error instanceof IpcCommandError && error.code === 'cancelled')) {
        notifyError(tr("解锁加密容器失败"), String(error));
      } else if (!current()) {
        console.warn('旧会话加密卷解锁已结束', error);
      }
    } finally {
      options.password = '';
      options.masterKeyHex = undefined;
      if (jobs.get(key) === taskId) jobs.delete(key);
    }
  })();
  notifyInfo(tr("已开始后台解锁"), tr("可在任务中心查看状态或取消"));
  return true;
}
