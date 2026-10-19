import { computed, readonly, ref } from 'vue';
import { releaseUpdateCheck, releaseUpdateInstall } from '../../../lib/ipc/system';
import { onEvent, type UnlistenFn } from '../../../lib/ipc/events';

export type UpdateStatus = 'idle' | 'checking' | 'up-to-date' | 'available' | 'unsupported' | 'downloading' | 'installing' | 'error';

const updateVersion = ref('');
const status = ref<UpdateStatus>('idle');
const error = ref('');
const downloadProgress = ref<number | null>(null);
let checkPromise: Promise<void> | null = null;

interface ReleaseProgress {
  version: string;
  downloaded: number;
  total: number;
  installing: boolean;
}

function isBusy(): boolean {
  return ['checking', 'downloading', 'installing'].includes(status.value);
}

async function checkForUpdates(): Promise<void> {
  if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return;
  if (checkPromise) return checkPromise;
  if (isBusy()) return;

  const operation = (async () => {
    status.value = 'checking';
    error.value = '';
    downloadProgress.value = null;
    updateVersion.value = '';
    try {
      const release = await releaseUpdateCheck();
      updateVersion.value = release.version;
      if (!release.available) {
        status.value = 'up-to-date';
        return;
      }
      if (!release.canInstall) {
        status.value = 'unsupported';
        return;
      }
      status.value = 'available';
    } catch (cause) {
      status.value = 'error';
      error.value = String(cause);
    }
  })();
  checkPromise = operation;
  try {
    await operation;
  } finally {
    checkPromise = null;
  }
}

async function installUpdate(): Promise<void> {
  if (status.value !== 'available') return;
  status.value = 'downloading';
  error.value = '';
  downloadProgress.value = null;
  let unlisten: UnlistenFn | undefined;
  try {
    unlisten = await onEvent<ReleaseProgress>('app-update-progress', ({ payload }) => {
      if (payload.version !== updateVersion.value) return;
      downloadProgress.value = payload.total > 0 ? Math.min(100, Math.round(payload.downloaded * 100 / payload.total)) : null;
      if (payload.installing) status.value = 'installing';
    });
    await releaseUpdateInstall(updateVersion.value);
    status.value = 'installing';
  } catch (cause) {
    status.value = 'error';
    error.value = String(cause);
  } finally {
    unlisten?.();
  }
}

export function useAppUpdater() {
  return {
    status: readonly(status),
    error: readonly(error),
    updateVersion: readonly(updateVersion),
    downloadProgress: readonly(downloadProgress),
    isBusy: computed(isBusy),
    hasUpdate: computed(() => status.value === 'available'),
    checkForUpdates,
    installUpdate,
  };
}
