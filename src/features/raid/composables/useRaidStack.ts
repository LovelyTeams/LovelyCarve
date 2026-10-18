
import { tr } from '../../../composables/useLocale';
// Session-wide RAID detection state. raid_auto_detect scans ALL loaded
// images (LVM PV joins and mdadm member sets span images), so its result is
// not per-image — every RaidStackPanel instance (one per tab) shares one
// stack, and panel remounts render the cached result without re-firing the
// IPC. Case restore (AppShell.onOpenCaseEvidence) runs the same detect
// so freshly re-registered synthetic images show up in already-open panels.
import { ref } from 'vue';
import { imageInfoById } from '../../../lib/ipc/image';
import { raidAutoDetect, type RaidArray, type RaidStack } from '../../../lib/ipc/raid';
import { formatBinaryBytes as formatBytes } from '../../../utils/format';
import { confirm } from '../../../composables/useConfirm';
import { notifyError, pushPersistentToast, type PersistentToastHandle } from '../../../composables/useToast';
import { activeMembers, levelShortLabel } from '../model/stackView';

const stack = ref<RaidStack | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);
// Stale-response guard: only the latest detect() run may publish its result
// (panels in several tabs can trigger overlapping runs).
let generation = 0;
// md sets the examiner chose to assemble without the members their metadata
// still expects. Every later run in this case keeps honouring the choice, so
// a re-detect never silently drops the degraded image again.
const degradedSets = new Set<string>();
// Per-array announcement state, keyed by mdadm set UUID. An incomplete array
// keeps a PERSISTENT toast alive (an auto-dismissing one fires while the
// image-load overlay still has the user's eyes and vanishes unseen); once
// the missing members get loaded the toast is dismissed and the open-it
// dialog takes over. `promptedAssembled` keeps the dialog to once a session.
const incompleteToasts = new Map<string, PersistentToastHandle>();
const promptedAssembled = new Set<string>();

// Tab opener injected by AppShell (the only place that owns the tab
// manager) — a composable can't emit through the component tree.
let openImage: ((path: string) => void) | null = null;

/** Register the "open an image path as a tab" handler used when the user
 *  confirms opening a freshly assembled array. */
export function registerRaidImageOpener(opener: (path: string) => void) {
  openImage = opener;
  return () => { if (openImage === opener) openImage = null; };
}

function arrayTitle(arr: RaidArray): string {
  return arr.set_name || `md/${arr.set_uuid.slice(0, 12)}`;
}

/** Sequential "open the assembled image?" dialogs, one per newly completed
 *  array (the confirm host shows one dialog at a time). Fire-and-forget from
 *  detect() — opening happens via the injected tab opener. */
async function promptAssembledArrays(result: RaidStack, arrays: RaidArray[]) {
  const captured = generation;
  for (const arr of arrays) {
    if (captured !== generation) return;
    const fs = result.filesystems.find(f => f.set_uuid === arr.set_uuid);
    const detail = [formatBytes(arr.assembled_bytes), fs?.kind].filter(Boolean).join(' · ');
    const params = { 0: levelShortLabel(arr.level), 1: arrayTitle(arr), 2: detail };
    const ok = await confirm({
      title: arr.degraded ? tr("RAID 阵列已降级重组") : tr("检测到完整 RAID 阵列"),
      message: arr.degraded
        ? tr("{0} {1} 已在缺少成员的情况下重组为虚拟镜像（{2}），缺失部分由校验或镜像数据重建。是否打开重组镜像？", params)
        : tr("{0} {1} 已重组为虚拟镜像（{2}）。是否打开重组镜像？", params),
      confirmText: tr("打开重组镜像"),
      cancelText: tr("暂不打开"),
    });
    if (captured !== generation) return;
    if (!ok || !arr.assembled_image_id) continue;
    try {
      // RaidStack carries only the synthetic image's id — resolve its
      // `assembled://<label>` path, which the tab loader opens by reuse.
      const summary = await imageInfoById({ imageId: arr.assembled_image_id });
      openImage?.(summary.path);
    } catch (e) {
      notifyError(tr("打开重组镜像失败"), String(e));
    }
  }
}

/** Retire case-specific detection and prompts when its source sessions close. */
export function disposeRaidStack() {
  generation += 1;
  stack.value = null;
  loading.value = false;
  error.value = null;
  degradedSets.clear();
  for (const toast of incompleteToasts.values()) toast.dismiss();
  incompleteToasts.clear();
  promptedAssembled.clear();
}

/** Announce what a detect() run found: an incomplete array raises (or keeps)
 *  a persistent "成员不全" toast; a completed one dismisses that toast and
 *  raises the open-it confirm dialog (unless suppressed — case restore
 *  auto-opens its assembled evidence tabs anyway). */
function announceArrays(result: RaidStack, prompt: boolean) {
  for (const arr of result.arrays) {
    if (arr.assembled_image_id) {
      incompleteToasts.get(arr.set_uuid)?.dismiss();
      incompleteToasts.delete(arr.set_uuid);
      continue;
    }
    if (incompleteToasts.has(arr.set_uuid)) continue;
    const active = activeMembers(arr);
    const status = arr.degradable
      ? tr("成员不全 {0}/{1}，加载其余成员盘后将自动重组，也可在 RAID 检测卡片中降级重组", { 0: active, 1: arr.raid_disks })
      : active < arr.raid_disks
        ? tr("成员不全 {0}/{1}，加载其余成员盘后将自动重组", { 0: active, 1: arr.raid_disks })
        : tr("未能重组，详见镜像信息页的 RAID 检测卡片");
    incompleteToasts.set(
      arr.set_uuid,
      pushPersistentToast({
        title: tr("检测到 RAID 阵列"),
        detail: `${levelShortLabel(arr.level)} ${arrayTitle(arr)} · ${status}`,
      }),
    );
  }

  const assembled = result.arrays.filter(
    arr => arr.assembled_image_id && !promptedAssembled.has(arr.set_uuid),
  );
  for (const arr of assembled) promptedAssembled.add(arr.set_uuid);
  if (prompt && assembled.length > 0) {
    void promptAssembledArrays(result, assembled);
  }
}

export function useRaidStack() {
  /** Run detection across all loaded images. Resolves with the fresh stack,
   *  or null when this run lost to a newer one or failed (failure lands in
   *  `error` for the panels to render). `promptAssembled: false` suppresses
   *  the open-it dialog for arrays this run completes (case restore). */
  async function detect(opts?: { promptAssembled?: boolean }): Promise<RaidStack | null> {
    const runGeneration = ++generation;
    loading.value = true;
    error.value = null;
    try {
      const result = await raidAutoDetect([...degradedSets]);
      if (runGeneration !== generation) return null;
      stack.value = result;
      announceArrays(result, opts?.promptAssembled !== false);
      return result;
    } catch (e) {
      if (runGeneration !== generation) return null;
      error.value = String(e);
      return null;
    } finally {
      if (runGeneration === generation) loading.value = false;
    }
  }

  /** The examiner's consent to assemble `setUuid` without the members its
   *  metadata still expects; re-runs detection with the choice applied. */
  async function assembleDegraded(setUuid: string): Promise<RaidStack | null> {
    degradedSets.add(setUuid);
    return detect();
  }

  return { stack, loading, error, detect, assembleDegraded };
}
