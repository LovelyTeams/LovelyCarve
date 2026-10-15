
import { tr } from '../../../composables/useLocale';
import { computed, onActivated, onDeactivated, onMounted, onScopeDispose, ref, watch } from 'vue';
import { open } from '@tauri-apps/plugin-dialog';
import { bitlockerStatus } from '../../../lib/ipc/bitlocker';
import { fsAvailableMountPoints, fsCreateMountDir, fsMountReadonly, fsMounts, fsShareMount, fsUnmountReadonly, fsUnshareMount, type DokanMountView } from '../../../lib/ipc/mount';
import type { ImageInfo, DiskPartition } from '../../../lib/ipc/image';
import type { MountTarget } from '../../../composables/useUiPreferences';
import { notifySuccess, notifyError } from '../../../composables/useToast';
import { partitionTitle } from '../model/evidenceOverview';

interface EvidenceMountOptions {
  info: () => ImageInfo | null;
  mountTarget: () => MountTarget;
  mountFolderRoot: () => string;
  caseDirectory: () => string | null | undefined;
  onUnlocked: () => void;
}

/** Owns overview mount UI; native mounts outlive this view and remain backend-owned. */
export function useEvidenceMounts(options: EvidenceMountOptions) {
  let generation = 0;
  let disposed = false;
  function current(captured: number) { return !disposed && captured === generation; }
  // ── Dokan read-only mount ─────────────────────────────────────────────────
  // Mirror the supported partition set in fs_tree.rs::partition_node; only
  // these surface a `part:{id}` directory node that dokan_mount can project.
  const MOUNTABLE_KINDS = new Set([
    'ext4', 'btrfs', 'fat32', 'fat12', 'fat16', 'exfat', 'cdfs', 'squashfs',
    'apfscontainer', 'hfsplus', 'hfsx', 'lvm', 'ntfs', 'refs',
    'ufs', 'xfs', 'zfs', 'bitlocker',
  ]);

  const mounts = ref<DokanMountView[]>([]);
  const busyPartitions = ref<Set<number>>(new Set());
  const busyShares = ref<Set<string>>(new Set());
  const pickerOpenFor = ref<number | null>(null);
  const availableDrives = ref<string[]>([]);
  const pickerLoading = ref(false);

  // Synthetic-root evidence (docker / tar / zip / 7z / AD1) has no partition table —
  // `partitions` is empty and the file tree descends from a single merged root.
  // Surface a mount affordance for that root using its node id
  // ('docker:' / 'tar:' / 'zip:' / 'sevenz:' / 'ad1:'), reusing the per-partition mount machinery via
  // a sentinel busy/picker key. (Folder evidence is already an on-disk directory,
  // so mounting it adds nothing and is intentionally excluded.)
  const SYNTHETIC_MOUNT_KEY = -1;
  const syntheticRoot = computed<{ id: string; label: string } | null>(() => {
    switch (options.info()?.source_format) {
      case 'tar':
        return { id: 'tar:', get label() { return tr("归档根"); } };
      case 'zip':
        return { id: 'zip:', get label() { return tr("归档根"); } };
      case 'sevenz':
        return { id: 'sevenz:', get label() { return tr("归档根"); } };
      case 'ad1':
        return { id: 'ad1:', get label() { return tr("逻辑镜像根"); } };
      case 'docker':
        return { id: 'docker:', get label() { return tr("容器根"); } };
      default:
        return null;
    }
  });

  function isMountable(p: DiskPartition): boolean {
    return MOUNTABLE_KINDS.has(p.kind);
  }

  // BitLocker partitions only surface a usable filesystem after unlock — show
  // the mount button once the unlock control reports success. For every other
  // mountable kind the button is always available.
  const bitlockerUnlocked = ref<Set<number>>(new Set());

  function canShowMount(p: DiskPartition): boolean {
    if (p.kind !== 'bitlocker') return true;
    return bitlockerUnlocked.value.has(p.id);
  }

  async function refreshBitlockerUnlockedSet() {
    const info = options.info();
    const captured = generation;
    if (!info) {
      bitlockerUnlocked.value = new Set();
      return;
    }
    const next = new Set<number>();
    const probes = info.partitions
      .filter(p => p.kind === 'bitlocker')
      .map(async p => {
        try {
          const status = await bitlockerStatus({
            imageId: info.id,
            partitionId: p.id,
          });
          if (!status.locked) next.add(p.id);
        } catch {
          // Non-fatal — locked state defaults to "still locked"
        }
      });
    await Promise.all(probes);
    if (current(captured)) bitlockerUnlocked.value = next;
  }

  function onBitlockerUnlocked(partitionId: number) {
    const next = new Set(bitlockerUnlocked.value);
    next.add(partitionId);
    bitlockerUnlocked.value = next;
    // 解锁后的卷才能读到 VSS 区，重新列一次卷影副本。
    options.onUnlocked();
  }

  function mountViewFor(nodeId: string): DokanMountView | null {
    const imageId = options.info()?.id;
    return mounts.value.find(m => m.image_id === imageId && m.root_node_id === nodeId) || null;
  }

  function mountFor(p: DiskPartition): DokanMountView | null {
    return mountViewFor(`part:${p.id}`);
  }

  async function refreshMounts() {
    const captured = generation;
    try {
      const result = await fsMounts();
      if (current(captured)) mounts.value = result;
    } catch {
      // Non-Windows or transient; quietly leave the list empty.
      if (current(captured)) mounts.value = [];
    }
  }

  function setBusy(id: number, busy: boolean) {
    const next = new Set(busyPartitions.value);
    if (busy) next.add(id);
    else next.delete(id);
    busyPartitions.value = next;
  }

  function setShareBusy(id: string, busy: boolean) {
    const next = new Set(busyShares.value);
    if (busy) next.add(id);
    else next.delete(id);
    busyShares.value = next;
  }

  function shareNameFor(title: string, view: DokanMountView): string {
    const cleaned = title
      .replace(/["/\\[\]:|<>+=;,?*\u0000-\u001f]/g, '_')
      .replace(/\s+/g, '_')
      .replace(/_+/g, '_')
      .replace(/^_+|_+$/g, '')
      .slice(0, 48);
    const prefix = cleaned || 'mount';
    return `LC_${prefix}_${view.id.slice(0, 8)}`;
  }

  async function toggleShare(view: DokanMountView, title: string) {
    const captured = generation;
    if (busyShares.value.has(view.id)) return;
    setShareBusy(view.id, true);
    try {
      if (view.share) {
        await fsUnshareMount({ mountId: view.id });
        notifySuccess(tr("共享已关闭"), view.share.unc_path);
      } else {
        const share = await fsShareMount({
          mountId: view.id,
          shareName: shareNameFor(title, view),
        });
        try {
          await navigator.clipboard.writeText(share.unc_path);
          notifySuccess(tr("共享成功"), tr("{0} 已复制", { 0: share.unc_path }));
        } catch {
          notifySuccess(tr("共享成功"), share.unc_path);
        }
      }
      if (current(captured)) await refreshMounts();
    } catch (e) {
      notifyError(view.share ? tr("取消共享失败") : tr("共享失败"), String(e));
    } finally {
      if (current(captured)) setShareBusy(view.id, false);
    }
  }

  // Mount toggle keyed by node id + a numeric busy/picker key, so both
  // per-partition (`part:N`, key = partition id) and synthetic-root (`tar:` /
  // `docker:`, key = SYNTHETIC_MOUNT_KEY) mounts share one code path.
  async function beginMount(nodeId: string, key: number, title: string) {
    const info = options.info();
    if (!info || disposed) return;
    const context = { imageId: info.id, generation };
    if (busyPartitions.value.has(key)) return;
    const existing = mountViewFor(nodeId);
    if (existing) {
      setBusy(key, true);
      try {
        await fsUnmountReadonly({ mountId: existing.id });
        notifySuccess(tr("卸载成功"), tr("{0} 已释放", { 0: existing.mount_point }));
        if (current(context.generation)) await refreshMounts();
      } catch (e) {
        notifyError(tr("卸载失败"), String(e));
      } finally {
        if (current(context.generation)) setBusy(key, false);
      }
      return;
    }
    // Mount path branches on the chosen target (设置 → 界面 → 分区挂载方式).
    if (options.mountTarget() === 'folder') {
      // 解析「默认挂载根目录」：优先设置里配置的，其次当前案件目录。有根目录就
      // 直接在其下新建（或复用同名空）文件夹并挂载，不再弹目录选择框；都没有
      // 配置时退回手动选择目录。
      const root = options.mountFolderRoot().trim() || options.caseDirectory()?.trim() || '';
      if (root) {
        setBusy(key, true);
        let dir: string;
        try {
          dir = await fsCreateMountDir({ root, imageId: context.imageId, name: title });
        } catch (e) {
          notifyError(tr("挂载失败"), tr("无法在默认挂载根目录下新建文件夹：{0}", { 0: String(e) }));
          if (current(context.generation)) setBusy(key, false);
          return;
        }
        if (!current(context.generation)) return;
        setBusy(key, false);
        await performMount(context, nodeId, key, title, dir);
        return;
      }
      let folder: string | null = null;
      try {
        const picked = await open({
          directory: true,
          multiple: false,
          title: tr("选择挂载目录（须为空且位于 NTFS 卷）"),
        });
        folder = typeof picked === 'string' ? picked : null;
      } catch (e) {
        notifyError(tr("挂载失败"), String(e));
        return;
      }
      if (!folder || !current(context.generation)) return; // 用户取消或已切换检材
      await performMount(context, nodeId, key, title, folder);
      return;
    }
    // 盘符模式：刷新空闲盘符，在按钮下方展开选择器。
    pickerLoading.value = true;
    pickerOpenFor.value = key;
    try {
      const drives = await fsAvailableMountPoints();
      if (!current(context.generation)) return;
      availableDrives.value = drives;
      if (availableDrives.value.length === 0) {
        notifyError(tr("挂载失败"), tr("没有可用盘符（H–Y 全部占用）"));
        pickerOpenFor.value = null;
      }
    } catch (e) {
      if (!current(context.generation)) return;
      notifyError(tr("挂载失败"), String(e));
      pickerOpenFor.value = null;
    } finally {
      if (current(context.generation)) pickerLoading.value = false;
    }
  }

  async function finishMountAt(nodeId: string, key: number, title: string, mountPoint: string) {
    const imageId = options.info()?.id;
    if (!imageId || disposed) return;
    return performMount({ imageId, generation }, nodeId, key, title, mountPoint);
  }

  async function performMount(context: { imageId: string; generation: number }, nodeId: string, key: number, title: string, mountPoint: string) {
    if (!current(context.generation) || busyPartitions.value.has(key)) return;
    pickerOpenFor.value = null;
    setBusy(key, true);
    try {
      const view = await fsMountReadonly({
        imageId: context.imageId,
        nodeId,
        mountPoint,
      });
      notifySuccess(tr("挂载成功"), `${title} → ${view.mount_point}`);
      if (current(context.generation)) await refreshMounts();
    } catch (e) {
      notifyError(tr("挂载失败"), String(e));
    } finally {
      if (current(context.generation)) setBusy(key, false);
    }
  }

  function toggleMount(p: DiskPartition) {
    return beginMount(`part:${p.id}`, p.id, partitionTitle(p, tr));
  }

  function mountAt(p: DiskPartition, mountPoint: string) {
    return finishMountAt(`part:${p.id}`, p.id, partitionTitle(p, tr), mountPoint);
  }

  watch([() => options.info()?.id, options.caseDirectory], () => {
    generation += 1;
    mounts.value = [];
    busyPartitions.value = new Set();
    busyShares.value = new Set();
    bitlockerUnlocked.value = new Set();
    pickerLoading.value = false;
    availableDrives.value = [];
    pickerOpenFor.value = null;
    void refreshMounts();
    void refreshBitlockerUnlockedSet();
  }, { immediate: true, flush: 'sync' });

  function onGlobalClick() { pickerOpenFor.value = null; }
  function onEsc(event: KeyboardEvent) {
    if (event.key === 'Escape' && !event.isComposing) pickerOpenFor.value = null;
  }
  let listening = false;
  function attach() {
    if (listening) return;
    listening = true;
    document.addEventListener('click', onGlobalClick);
    document.addEventListener('keydown', onEsc);
  }
  function detach() {
    listening = false;
    pickerOpenFor.value = null;
    document.removeEventListener('click', onGlobalClick);
    document.removeEventListener('keydown', onEsc);
  }
  onMounted(attach);
  onActivated(attach);
  onDeactivated(detach);
  onScopeDispose(() => { disposed = true; generation += 1; detach(); });

  return { SYNTHETIC_MOUNT_KEY, syntheticRoot, mounts, busyPartitions, busyShares, pickerOpenFor, availableDrives, pickerLoading, isMountable, canShowMount, onBitlockerUnlocked, mountViewFor, mountFor, toggleShare, beginMount, finishMountAt, toggleMount, mountAt };
}
