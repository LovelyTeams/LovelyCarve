
import { tr } from '../../../composables/useLocale';
import { ref, shallowRef, onScopeDispose, getCurrentScope } from 'vue';
import { open } from '@tauri-apps/plugin-dialog';

import { pushPersistentToast, type PersistentToastHandle } from '../../../composables/useToast';
import { bitlockerStatus } from '../../../lib/ipc/bitlocker';
import { onEvent } from '../../../lib/ipc/events';
import { imageLoadFull } from '../../../lib/ipc/image';
import type { ImageInfo } from '../../../lib/ipc/image';
import { forgetUnlockedImageInfo, latestUnlockedImageInfo } from './unlockedImageInfo';


export type OpenFileOutcome =
  /** `error` is a user-facing reason; absent when nothing was attempted
   *  (empty path). Callers decide how to surface it. */
  | { opened: false; error?: string }
  | { opened: true; mode: 'image'; nextPanel: 'files' | 'dashboard'; imageId: string };

type OpenFileOptions = {
  onBeforeLoad?: () => void;
  /** Force the disk-image load path (`image_load`) regardless of extension.
   *  Set for folder sources, which have no disk-image extension and would
   *  otherwise be rejected by extension sniffing. */
  forceImage?: boolean;
  caseDir?: string;
  /** Transient legacy QCOW credentials, forwarded only to this load call. */
  qcowPassword?: string;
  qcowKeyHex?: string;
};

/** Payload of the backend `image-load-progress` event (commands::image). */
type ImageLoadProgressEvent = {
  path: string;
  stage: string;
  current: number;
  total: number;
};

type LoadProgressEvent = {
  stage: string;
  current: number;
  total: number;
  entries: number;
};

export const DISK_IMAGE_EXTENSIONS = [
  'dd',
  'hc',
  'tc',
  'luks',
  'dsk',
  'img',
  'raw',
  '001',
  'bin',
  'ubi',
  'iso',
  'e01',
  'ewf',
  'ad1',
  'vhd',
  'vhdx',
  'vmdk',
  'qcow',
  'qcow2',
  'hdd',
  'hds',
  'vdi',
  'pmfx',
  'ldbk',
  'ab',
  'bak',
  'tar',
  'tgz',
  'zip',
  '7z',
];

const PICKER_IMAGE_EXTENSIONS = [...DISK_IMAGE_EXTENSIONS, 'gz', 'xml'];

export function isDiskImageExtension(extension: string | undefined | null): boolean {
  return DISK_IMAGE_EXTENSIONS.includes((extension || '').toLowerCase());
}

function isDiskImagePath(fileName: string): boolean {
  const lower = fileName.toLowerCase();
  const extension = lower.split('.').pop();
  return lower.endsWith('.tar.gz') || lower.split(/[\\/]/).pop() === 'diskdescriptor.xml' || isDiskImageExtension(extension);
}

// Image ids we've already raised the locked-BitLocker alert for. Module-scoped
// so re-activating a tab (which restores a snapshot, not re-runs `loadFile`) or
// any re-render can't re-pop. Keyed on the stable backend image id.
const alertedBitlockerImageIds = new Set<string>();

// 「前往解锁」跳转回调：把用户带到镜像信息面板（BitLocker 解锁控件所在）。由
// AppShell 注入——组合式函数不持有面板状态，沿用 useRaidStack 的
// registerRaidImageOpener 解耦套路。
let bitlockerUnlockNavigator: (() => void) | null = null;

/** Register the handler that navigates to the 镜像信息 panel where
 *  BitLocker unlock lives. Called once by AppShell. */
export function registerBitlockerUnlockNavigator(navigate: () => void) {
  bitlockerUnlockNavigator = navigate;
  return () => { if (bitlockerUnlockNavigator === navigate) bitlockerUnlockNavigator = null; };
}

// The open 「检测到 BitLocker」 reminder of each image, keyed like the claim set.
const bitlockerReminders = new Map<string, PersistentToastHandle>();

/** The user has started unlocking this image's BitLocker volume: the reminder
 *  has done its job, and left open it would sit over the unlock popover. */
export function dismissBitlockerReminder(imageId: string) {
  bitlockerReminders.get(imageId)?.dismiss();
  bitlockerReminders.delete(imageId);
}

/** After a disk image loads, if it carries one or more *locked* BitLocker
 *  partitions, raise a persistent bottom-right toast (with a 前往解锁 CTA).
 *  Fire-and-forget — never blocks the load on the per-partition status probes.
 *  Fires at most once per image id. `kind === 'bitlocker'` alone only says a
 *  BitLocker volume exists; we probe `bitlockerStatus` to avoid a false alert
 *  on a volume already unlocked earlier this session. */
async function maybeAlertLockedBitlocker(info: ImageInfo): Promise<void> {
  if (!info.id || alertedBitlockerImageIds.has(info.id)) return;
  const bdeParts = info.partitions.filter(p => p.kind === 'bitlocker');
  if (bdeParts.length === 0) return;
  // Claim the id up-front so two near-simultaneous loads of the same image
  // can't both fire while the async probes are in flight.
  alertedBitlockerImageIds.add(info.id);
  try {
    const statuses = await Promise.all(
      bdeParts.map(p =>
        bitlockerStatus({ imageId: info.id, partitionId: p.id })
          .then(s => s.locked)
          .catch(() => true), // probe failed → assume locked (a fresh image usually is)
      ),
    );
    const lockedCount = statuses.filter(Boolean).length;
    if (lockedCount === 0) {
      // Already unlocked this session — drop the claim so a later re-lock /
      // re-load can still alert.
      alertedBitlockerImageIds.delete(info.id);
      return;
    }
    const handle = pushPersistentToast({
      kind: 'info',
      title: tr("检测到 BitLocker 加密分区"),
      detail:
        lockedCount > 1
          ? tr("镜像包含 {0} 个已锁定的 BitLocker 分区，需解锁后才能浏览其文件系统。", { 0: lockedCount })
          : tr("镜像包含一个已锁定的 BitLocker 分区，需解锁后才能浏览其文件系统。"),
      action: {
        get label() { return tr("前往解锁"); },
        onClick: () => {
          bitlockerUnlockNavigator?.();
          dismissBitlockerReminder(info.id);
        },
      },
    });
    bitlockerReminders.set(info.id, handle);
  } catch {
    // Any unexpected failure: release the claim so detection can retry later.
    alertedBitlockerImageIds.delete(info.id);
  }
}

/** Frozen view of a file-session's reactive state, suitable for stashing
 *  in a Map and restoring later (e.g. multi-tab swap). The shape mirrors
 *  the refs returned by `useFileSession()` 1-to-1 — capture is just a
 *  read of `.value`, restore is just an assignment back. */
export interface FileSessionSnapshot {
  isEvidenceMode: boolean;
  loading: boolean;
  currentFileName: string;
  parserName: string;
  loadProgress: LoadProgressEvent | null;
  imageId: string;
  imageInfo: ImageInfo | null;
}

export function useFileSession() {
  const isEvidenceMode = ref(false);
  const loading = ref(false);
  const currentFileName = ref('');
  const parserName = ref('');
  const loadProgress = ref<LoadProgressEvent | null>(null);
  const imageId = ref('');
  const imageInfo = shallowRef<ImageInfo | null>(null);

  let loadGeneration = 0;
  if (getCurrentScope()) onScopeDispose(resetSession);
  let unlistenImageLoadProgress: (() => void) | null = null;

  function cleanupLoadProgressListener() {
    if (unlistenImageLoadProgress) {
      unlistenImageLoadProgress();
      unlistenImageLoadProgress = null;
    }
  }

  function resetSession() {
    loadGeneration++;
    cleanupLoadProgressListener();
    isEvidenceMode.value = false;
    loading.value = false;
    currentFileName.value = '';
    parserName.value = '';
    loadProgress.value = null;
    imageId.value = '';
    imageInfo.value = null;
  }

  async function loadFile(filePath: string, options: OpenFileOptions = {}): Promise<OpenFileOutcome> {
    options.onBeforeLoad?.();
    const generation = ++loadGeneration;
    try {
      loading.value = true;
      loadProgress.value = null;
      imageId.value = '';
      imageInfo.value = null;
      cleanupLoadProgressListener();

      const parts = filePath.replace(/\\/g, '/').split('/');
      currentFileName.value = parts[parts.length - 1];

      // `assembled://` paths name synthetic RAID images already registered in
      // the backend by raid_auto_detect — image_load resolves them by path
      // reuse. Checked before any extension dispatch: the label embeds the
      // evidence-controlled mdadm set_name, which may end in `.pcap` etc.
      const isAssembled = filePath.startsWith('assembled://')
        || filePath.startsWith('decrypted://')
        || filePath.startsWith('vss://');

      isEvidenceMode.value = false;

      if (isAssembled || options.forceImage || isDiskImagePath(currentFileName.value)) {
        // Surface backend open phases (decompress / 检测分区 / 检测文件系统) in
        // the loading overlay instead of a featureless spinner. Filter by path
        // so a concurrent load in another tab can't bleed in.
        loadProgress.value = null;
        const stop = await onEvent<ImageLoadProgressEvent>(
          'image-load-progress',
          event => {
            if (generation !== loadGeneration || event.payload.path !== filePath) return;
            loadProgress.value = {
              stage: event.payload.stage,
              current: event.payload.current,
              total: event.payload.total,
              entries: 0,
            };
          },
        );
        if (generation !== loadGeneration) { stop(); return { opened: false }; }
        unlistenImageLoadProgress = stop;
        const info = await imageLoadFull<ImageInfo>({
          path: filePath,
          qcowPassword: options.qcowPassword,
          qcowKeyHex: options.qcowKeyHex,
          ...(options.caseDir ? { caseDir: options.caseDir } : {}),
        });
        if (generation !== loadGeneration) return { opened: false };
        imageId.value = info.id;
        forgetUnlockedImageInfo(info.id);
        imageInfo.value = info;
        isEvidenceMode.value = true;
        // Fire-and-forget: alert (bottom-right) if the image has a locked
        // BitLocker partition. Must not block the load / panel switch on the
        // per-partition status probes.
        if (!info.index_only) void maybeAlertLockedBitlocker(info);
        parserName.value = info.index_only ? tr("离线索引（{0}）", { 0: info.source_format }) : `Disk Image (${info.source_format})`;
        return { opened: true, mode: 'image', nextPanel: info.index_only ? 'files' : 'dashboard', imageId: info.id };
      }

      return {
        opened: false,
        error: tr("暂不支持该文件类型。当前版本支持磁盘镜像、逻辑镜像与归档文件，以及文件夹；请选择镜像文件，或通过「添加检材」选择文件夹或物理磁盘。"),
      };
    } finally {
      if (generation === loadGeneration) { cleanupLoadProgressListener(); loading.value = false; }
    }
  }

  /** Open the platform folder picker for a directory-as-evidence source.
   *  Returns null on cancel. The chosen path loads through `image_load`,
   *  which treats a directory as a logical-evidence `FolderImage`. */
  async function pickFolder(): Promise<string | null> {
    const selected = await open({
      directory: true,
      multiple: false,
      title: tr("选择文件夹作为逻辑检材"),
    });
    if (!selected || typeof selected !== 'string') return null;
    return selected;
  }

  async function pickBcpFiles(): Promise<string[]> {
    const selected = await open({
      multiple: true,
      title: tr("选择 BCP 文件（可多选）"),
      filters: [{ name: tr("BCP 数据文件"), extensions: ['bcp'] }],
    });
    if (!selected) return [];
    return Array.isArray(selected) ? selected as string[] : [selected as string];
  }

  /** Open the platform picker in multi-select mode. Returns the selected
   *  paths, or an empty array on cancel. Used for opening several disk images
   *  at once and for picking RAID member disks (caller sets the title). */
  async function pickFiles(title = tr("选择磁盘镜像（可多选）")): Promise<string[]> {
    const selected = await open({
      multiple: true,
      title,
      filters: [
        {
          name: 'Disk Images',
          extensions: PICKER_IMAGE_EXTENSIONS,
        },
        {
          name: 'All Files',
          extensions: ['*'],
        },
      ],
    });
    if (!selected) return [];
    return Array.isArray(selected) ? (selected as string[]) : [selected as string];
  }

  async function openByPath(filePath: string, options: OpenFileOptions = {}): Promise<OpenFileOutcome> {
    if (!filePath) return { opened: false };
    return loadFile(filePath, options);
  }

  /** Snapshot every reactive ref into a plain object the multi-tab
   *  manager can store for inactive tabs. We tear down the streaming
   *  progress listeners as part of the snapshot — leaving them alive
   *  for an inactive tab would let backend events from another tab's load
   *  bleed into this tab's progress state. */
  function captureSnapshot(): FileSessionSnapshot {
    loadGeneration++;
    cleanupLoadProgressListener();
    return {
      isEvidenceMode: isEvidenceMode.value,
      loading: loading.value,
      currentFileName: currentFileName.value,
      parserName: parserName.value,
      loadProgress: loadProgress.value,
      imageId: imageId.value,
      imageInfo: imageInfo.value,
    };
  }

  /** Restore a previously captured session into the live refs.
   *  Listeners are NOT re-subscribed — restored tabs are post-load
   *  state, no events will be emitted unless the user kicks off a
   *  fresh analysis (which sets up its own listeners on demand). */
  function restoreSnapshot(snap: FileSessionSnapshot): void {
    loadGeneration++;
    cleanupLoadProgressListener();
    isEvidenceMode.value = snap.isEvidenceMode;
    loading.value = snap.loading;
    currentFileName.value = snap.currentFileName;
    parserName.value = snap.parserName;
    loadProgress.value = snap.loadProgress;
    imageId.value = snap.imageId;
    imageInfo.value = latestUnlockedImageInfo(snap.imageInfo);
  }

  return {
    updateImageInfo: (info: ImageInfo) => { if (imageId.value === info.id) imageInfo.value = info; },
    isEvidenceMode,
    loading,
    currentFileName,
    parserName,
    loadProgress,
    imageId,
    imageInfo,
    pickFolder,
    pickBcpFiles,
    pickFiles,
    openByPath,
    resetSession,
    captureSnapshot,
    restoreSnapshot,
  };
}
