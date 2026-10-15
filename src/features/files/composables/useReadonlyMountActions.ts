
import { tr } from '../../../composables/useLocale';
import { computed, ref, shallowRef, watch, onScopeDispose } from 'vue';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { fsCreateMountDir, fsMountReadonly, fsMounts, fsSuggestMountPoint, fsUnmountReadonly, type DokanMountView } from '../../../lib/ipc/mount';
import { notifyError, pushPersistentToast } from '../../../composables/useToast';
import { isHostDockerMountRoot, type DiskFsNode as FsNode } from '../model/model';

/** Readonly mount actions for synthetic container roots; query state is independent. */
export function useReadonlyMountActions(options: {
  imageId: () => string;
  node: () => FsNode | null;
  target: () => 'drive' | 'folder';
  folderRoot: () => string;
  caseDirectory: () => string | null;
  closeMenu: () => void;
}) {
  const readonlyMounts = shallowRef<DokanMountView[]>([]);
  const mountBusy = ref(false);
  let generation = 0;
  let queryGeneration = 0;
  let disposed = false;
  watch([options.imageId, options.caseDirectory], () => {
    generation++;
    queryGeneration++;
    readonlyMounts.value = [];
    mountBusy.value = false;
  }, { flush: 'sync' });
  onScopeDispose(() => { disposed = true; generation++; queryGeneration++; });

  function mountViewForNode(nodeId: string): DokanMountView | null {
    return readonlyMounts.value.find(view => view.image_id === options.imageId() && view.root_node_id === nodeId) ?? null;
  }

  const rowCanMount = computed(() => isHostDockerMountRoot(options.node()));
  const rowMountView = computed(() => { const node = options.node(); return node ? mountViewForNode(node.id) : null; });
  const rowMountLabel = computed(() => {
    const view = rowMountView.value;
    if (view) {
      const trimmed = view.mount_point.replace(/[\\/]+$/, '');
      const leaf = trimmed.split(/[\\/]/).filter(Boolean).pop() || trimmed;
      return tr("卸载 {0}", { 0: leaf });
    }
    return options.target() === 'folder' ? tr("挂载到只读文件夹") : tr("挂载为只读盘符");
  });

  async function refreshReadonlyMounts() {
    const request = ++queryGeneration;
    try {
      const views = await fsMounts();
      if (!disposed && request === queryGeneration) readonlyMounts.value = views;
    } catch {
      if (!disposed && request === queryGeneration) readonlyMounts.value = [];
    }
  }

  async function chooseRowMountPoint(title: string, imageId: string, target: 'drive' | 'folder', root: string): Promise<string | null> {
    if (target === 'drive') return fsSuggestMountPoint();
    if (root) {
      return fsCreateMountDir({ root, imageId, name: title });
    }
    const picked = await openDialog({
      directory: true,
      multiple: false,
      title: tr("选择挂载目录（须为空且位于 NTFS 卷）"),
    });
    return typeof picked === 'string' ? picked : null;
  }

  async function onToggleRowMount() {
    const node = options.node();
    if (!isHostDockerMountRoot(node) || mountBusy.value) return;
    const epoch = generation;
    const imageId = options.imageId();
    const target = options.target();
    const root = options.folderRoot().trim() || options.caseDirectory()?.trim() || '';
    const current = () => !disposed && epoch === generation;
    options.closeMenu();
    mountBusy.value = true;
    try {
      await refreshReadonlyMounts();
      if (!current()) return;
      const existing = readonlyMounts.value.find(view => view.image_id === imageId && view.root_node_id === node.id);
      if (existing) {
        const toast = pushPersistentToast({ kind: 'info', title: tr("正在卸载…"), detail: existing.mount_point });
        try {
          await fsUnmountReadonly({ mountId: existing.id });
          toast.update({ kind: 'success', title: tr("卸载成功"), detail: tr("{0} 已释放", { 0: existing.mount_point }) });
          toast.dismiss(3200);
        } catch (error) {
          toast.update({ kind: 'error', title: tr("卸载失败"), detail: String(error) });
          toast.dismiss(6000);
        }
        if (current()) await refreshReadonlyMounts();
        return;
      }

      const mountPoint = await chooseRowMountPoint(node.name, imageId, target, root);
      if (!mountPoint || !current()) return;
      const toast = pushPersistentToast({
        kind: 'info',
        title: tr("正在挂载 Docker 文件系统…"),
        detail: `${node.name} → ${mountPoint}`,
      });
      try {
        const view = await fsMountReadonly({ imageId, nodeId: node.id, mountPoint });
        toast.update({ kind: 'success', title: tr("挂载成功"), detail: `${node.name} → ${view.mount_point}` });
        toast.dismiss(4200);
      } catch (error) {
        toast.update({ kind: 'error', title: tr("挂载失败"), detail: String(error) });
        toast.dismiss(6000);
      }
      if (current()) await refreshReadonlyMounts();
    } catch (error) {
      if (current()) notifyError(tr("挂载失败"), String(error));
    } finally {
      if (current()) mountBusy.value = false;
    }
  }
  return { mountBusy, rowCanMount, rowMountView, rowMountLabel, refreshReadonlyMounts, onToggleRowMount };
}
