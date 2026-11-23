// BitLocker、LUKS、VeraCrypt、FileVault 的解锁都走 `encrypted_volume_unlock`：
// 立即提交、后台运行，任务中心按 taskId 列出并可取消；结果更新检材信息或打开
// 新的明文会话。登记表按检材与区域去重，分区行据此显示“解锁中”。
import { reactive } from 'vue';
import { tr } from '../../../composables/useLocale';
import { notifyError, notifyInfo, notifySuccess } from '../../../composables/useToast';
import { encryptedVolumeUnlock, type EncryptedUnlockOptions, type EncryptionScheme, type UnlockCredential } from '../../../lib/ipc/encrypted';
import { imageUnload, type ImageInfo } from '../../../lib/ipc/image';
import { IpcCommandError } from '../../../lib/ipc/client';
import type { ImageId } from '../../../lib/ipc/ids';
import { evidenceContext } from './evidenceContext';
import { clearUnlockedImageInfo, rememberUnlockedImageInfo } from './unlockedImageInfo';

export interface EncryptedUnlockJob {
  imageId: ImageId;
  /** Undefined for the whole image or container. */
  partitionId?: number;
  scheme: EncryptionScheme;
  taskId: string;
}

// The main window owns completion and source metadata, independently of the dialog.
const jobs = reactive(new Map<string, EncryptedUnlockJob>());
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

/** The unlock running for a partition of an image, if any. Reads the reactive
 *  registry, so templates and computeds track it. */
export function encryptedUnlockJobFor(imageId: ImageId, partitionId: number): EncryptedUnlockJob | undefined {
  for (const job of jobs.values()) {
    if (job.imageId === imageId && job.partitionId === partitionId) return job;
  }
  return undefined;
}

/** Drop the secret text the caller still holds; the request keeps its own copy. */
function forgetCredential(credential: UnlockCredential) {
  switch (credential.kind) {
    case 'password': credential.password = ''; break;
    case 'recoveryPassword': credential.recoveryPassword = ''; break;
    case 'masterKey': credential.keyHex = ''; credential.tweakHex = undefined; break;
    case 'vmk': credential.keyHex = ''; break;
    case 'startupKey':
    case 'clearKey': break;
  }
}

/** Submit immediately; the task center lists the unlock under `taskId`, and
 *  metadata updates outlive the page. */
export function startEncryptedVolumeUnlock(info: ImageInfo, options: EncryptedUnlockOptions): boolean {
  const key = `${info.id}:${options.partitionId ?? 'whole'}${options.target === 'newImage' ? `:lv:${options.logicalVolumeIndex ?? 0}` : ''}`;
  if (jobs.has(key)) {
    forgetCredential(options.credential);
    notifyError(tr("解锁加密卷失败"), tr("此加密区域已在解锁"));
    return false;
  }
  const taskId = crypto.randomUUID();
  const epoch = generation;
  const context = evidenceContext();
  const updater = updateImage;
  const opener = openDerivedImage;
  const current = () => epoch === generation && context === evidenceContext();
  jobs.set(key, { imageId: info.id, ...(options.partitionId === undefined ? {} : { partitionId: options.partitionId }), scheme: options.scheme, taskId });
  void (async () => {
    try {
      options.target ??= 'sourcePartition';
      // IPC may defer transport: the request carries its own copy of the
      // credential, so clearing the local one cannot change its payload.
      const request = encryptedVolumeUnlock({ imageId: info.id, options: { ...options, credential: { ...options.credential } }, taskId });
      forgetCredential(options.credential);
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
      notifySuccess(tr("加密卷解锁成功"), result.recovery_password
        ? tr("恢复密码：{0}", { 0: result.recovery_password })
        : result.cipher);
      // Paused system encryption: the unencrypted rest reads as stored.
      if (result.veracrypt && !result.veracrypt.encryption_complete) notifyInfo(tr("加密未完成"), result.cipher);
    } catch (error) {
      if (current() && !(error instanceof IpcCommandError && error.code === 'cancelled')) {
        notifyError(tr("解锁加密卷失败"), String(error));
      } else if (!current()) {
        console.warn('旧会话加密卷解锁已结束', error);
      }
    } finally {
      forgetCredential(options.credential);
      if (jobs.get(key)?.taskId === taskId) jobs.delete(key);
    }
  })();
  notifyInfo(tr("已开始后台解锁"));
  return true;
}
