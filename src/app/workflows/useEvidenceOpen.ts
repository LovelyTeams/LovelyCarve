
import { tr } from '../../composables/useLocale';
import { ref } from 'vue';
import type { AddImageKind } from '../../features/evidence/model/types';
import type { LoadFileOptions, OpenTabResult } from '../workspace/useTabManager';
import { notifyError } from '../../composables/useToast';

/** Source selection and native pickers share the workspace's existing tab load path. */
export function useEvidenceOpen(options: {
  pickFiles: (title: string) => Promise<string[]>;
  pickFolder: () => Promise<string | null>;
  pickBcpFiles: () => Promise<string[]>;
  openImagePath: (path: string, options?: LoadFileOptions) => Promise<OpenTabResult>;
  openRaidMemberDialog: () => Promise<void>;
  preferredBcpFiles: Map<string, string>;
  showPanel: (panel: string) => void;
}) {
  const { pickFiles, pickFolder, pickBcpFiles, openImagePath, openRaidMemberDialog, preferredBcpFiles } = options;
  const showAddImageDialog = ref(false);
  const showPhysicalDiskPicker = ref(false);
  /** Resolver for the in-flight `pickPhysicalDisk()` promise — settled by the
   *  picker's `select` (device path) / `close` (null) callbacks. */
  let physicalDiskResolve: ((path: string | null) => void) | null = null;

  /** Open the physical-disk picker and resolve with the chosen device path
   *  (or null on cancel). Mirrors `useFileSession`'s `pickFile` contract so the
   *  `onAddImageSelect` branch reads the same as the 镜像文件 / 文件夹 branches. */
  function pickPhysicalDisk(): Promise<string | null> {
    showPhysicalDiskPicker.value = true;
    return new Promise(resolve => {
      physicalDiskResolve = resolve;
    });
  }

  function onPhysicalDiskSelect(path: string) {
    showPhysicalDiskPicker.value = false;
    physicalDiskResolve?.(path);
    physicalDiskResolve = null;
  }

  function onPhysicalDiskPickerClose() {
    showPhysicalDiskPicker.value = false;
    physicalDiskResolve?.(null);
    physicalDiskResolve = null;
  }

  /** Route the "添加证据" chooser pick to the matching load flow. Closing first
   *  keeps the dialog from sitting over the native file picker. The case gate
   *  already ran in `handleOpenFile` before the dialog opened. */
  async function onAddImageSelect(kind: AddImageKind) {
    showAddImageDialog.value = false;
    if (kind === 'image') {
      // Open one or more disk images, each as its own tab (last one active).
      const paths = await pickFiles(tr("选择磁盘镜像（可多选）"));
      for (const path of paths) {
        if (/\.bcp$/i.test(path)) await openBcpFilePaths([path]);
        else await openImagePath(path);
      }
    } else if (kind === 'folder') {
      const path = await pickFolder();
      if (path) await openImagePath(path, { forceImage: true });
    } else if (kind === 'bcp') {
      await openBcpFilePaths(await pickBcpFiles());
    } else if (kind === 'physical_disk') {
      // 物理磁盘路径（\\.\PhysicalDriveN）无可识别扩展名，必须 forceImage 走
      // image_load 分支，否则会被 loadFile 的扩展名嗅探误判。打开失败多为未提权，
      // 静默加载后在后端原因后补上提权提示。
      const path = await pickPhysicalDisk();
      if (path) {
        const result = await openImagePath(path, { forceImage: true, quiet: true });
        if (!result.ok) {
          notifyError(tr("打开物理磁盘失败"), tr("{0}：{1}（读取物理磁盘需要以管理员身份运行本程序）", { 0: path, 1: result.error }));
        }
      }
    } else {
      await openRaidMemberDialog();
    }
  }

  async function openBcpFilePaths(files: string[]) {
    const folders = [...new Set(files.map(file => file.replace(/[\\/][^\\/]+$/, '')))];
    for (const folder of folders) {
      const lastFile = files.filter(file => file.replace(/[\\/][^\\/]+$/, '') === folder).at(-1);
      if (lastFile) preferredBcpFiles.set(folder, lastFile.split(/[\\/]/).at(-1) ?? '');
      const result = await openImagePath(folder, { forceImage: true });
      if (result.ok) options.showPanel('bcp');
    }
  }

  /** Open an image path requested from the image-info page (the RAID panel's
   *  打开重组镜像, or an unlocked volume's decrypted image). Synthetic images only
   *  live for this session, so pair the backend reason with the recovery step. */
  async function onOpenImageFromInfo(path: string) {
    const result = await openImagePath(path, { quiet: true });
    if (result.ok) return true;
    const hint = path.startsWith('decrypted://')
      ? tr("解密会话已关闭，请重新解锁源镜像")
      : path.startsWith('vss://') ? tr("卷影副本会话已关闭，请在源镜像的「卷影副本」中重新打开")
      : path.startsWith('assembled://') ? tr("请重新运行 RAID 检测后重试") : '';
    notifyError(tr("打开镜像失败"), hint ? `${result.error}；${hint}` : result.error);
    return false;
  }

  return {
    showAddImageDialog, showPhysicalDiskPicker, onPhysicalDiskSelect,
    onPhysicalDiskPickerClose, onAddImageSelect, onOpenImageFromInfo,
  };
}
