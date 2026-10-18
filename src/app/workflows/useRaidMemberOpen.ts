
import { tr } from '../../composables/useLocale';
import { onScopeDispose, reactive, ref, watch, type Ref } from 'vue';
import type { CaseInfo } from '../../lib/ipc/cases';
import { caseEvidenceList } from '../../lib/ipc/cases';
import { imageInfoById, imageLoad } from '../../lib/ipc/image';
import type { RaidArray, RaidStack } from '../../lib/ipc/raid';
import type { RaidMemberQueueItem } from '../../features/raid/model/types';
import { useRaidStack } from '../../features/raid/api';
import { notifyInfo, notifyError } from '../../composables/useToast';
import type { LoadFileOptions, OpenTabResult } from '../workspace/useTabManager';

/** Stage members without activating tabs, detect only their arrays, then open assembled evidence. */
export function useRaidMemberOpen(options: {
  activeCase: Ref<CaseInfo | null>;
  removeEvidence: (imageId: string) => Promise<void>;
  pickFiles: (title: string) => Promise<string[]>;
  openImagePath: (path: string, options?: LoadFileOptions) => Promise<OpenTabResult>;
}) {
  const { activeCase, removeEvidence, pickFiles, openImagePath } = options;
  const { detect: detectRaid, error: raidStackError } = useRaidStack();
  const showRaidMemberDialog = ref(false);
  const raidMemberQueue = ref<RaidMemberQueueItem[]>([]);
  const raidMemberStack = ref<RaidStack | null>(null);
  const raidMemberDetecting = ref(false);
  const raidMemberOpening = ref(false);
  const raidMemberError = ref<string | null>(null);
  const raidMemberPreExistingIds = ref<Set<string>>(new Set());
  let memberLoadQueue = Promise.resolve();
  let generation = 0;
  onScopeDispose(() => { generation += 1; });
  /** Image ids that participate in a detected RAID array — whole-disk members,
   *  plus any image hosting a PV of a volume group whose LV backs a member. A
   *  disk loaded by the RAID flow but NOT in this set is not a RAID disk. */
  function raidMemberImageIds(stack: RaidStack): Set<string> {
    const ids = new Set<string>();
    const vgsBackingArray = new Set<string>();
    for (const arr of stack.arrays) {
      for (const m of arr.members) {
        if (m.origin.kind === 'whole') ids.add(m.origin.image_id);
        else vgsBackingArray.add(m.origin.vg_uuid);
      }
    }
    for (const vg of stack.volume_groups) {
      if (!vgsBackingArray.has(vg.vg_uuid)) continue;
      for (const pv of vg.members) ids.add(pv.image_id);
    }
    return ids;
  }

  function resetRaidMemberDialog() {
    generation += 1;
    raidMemberQueue.value = [];
    raidMemberStack.value = null;
    raidMemberError.value = null;
    raidMemberDetecting.value = false;
    raidMemberOpening.value = false;
    raidMemberPreExistingIds.value = new Set();
  }

  async function openRaidMemberDialog() {
    resetRaidMemberDialog();
    const captured = generation;
    if (activeCase.value) {
      try {
        const evidence = await caseEvidenceList();
        if (captured !== generation) return;
        raidMemberPreExistingIds.value = new Set(evidence.map(item => item.image_id));
      } catch (e) {
        console.error('case evidence list failed:', e);
      }
    }
    if (captured === generation) showRaidMemberDialog.value = true;
  }

  async function addRaidMemberFiles() {
    const captured = generation;
    const paths = await pickFiles(tr("添加 RAID 成员盘（可重复选择）"));
    if (captured === generation) await addRaidMemberPaths(paths);
  }

  /** Native drops and the picker reserve queue entries before any loading,
   *  then run batches sequentially so the final detection includes every drop. */
  function addRaidMemberPaths(paths: string[]): Promise<void> {
    if (raidMemberOpening.value) {
      notifyInfo(tr("正在打开重组镜像"), tr("打开完成后再添加成员盘。"));
      return Promise.resolve();
    }
    const freshPaths = [...new Set(paths)].filter(path => !raidMemberQueue.value.some(member => member.path === path));
    if (paths.length > 0 && freshPaths.length === 0) {
      notifyInfo(tr("成员盘已在队列中"), tr("所选磁盘已经放入 RAID 检测队列。"));
      return Promise.resolve();
    }
    if (freshPaths.length === 0) return Promise.resolve();
    const captured = generation;
    const freshMembers = freshPaths.map(path => reactive<RaidMemberQueueItem>({
      path,
      id: null,
      sizeBytes: null,
      status: 'loading',
      message: null,
      preExisting: false,
      isMember: false,
    }));
    raidMemberQueue.value.push(...freshMembers);
    memberLoadQueue = memberLoadQueue.then(async () => {
      for (const member of freshMembers) {
        if (captured !== generation) return;
        try {
          const summary = await imageLoad({ path: member.path });
          if (captured !== generation) return;
          member.id = summary.id;
          member.sizeBytes = summary.size_bytes;
          member.status = 'loaded';
          member.preExisting = raidMemberPreExistingIds.value.has(summary.id);
        } catch (e) {
          if (captured !== generation) return;
          member.status = 'failed';
          member.message = e instanceof Error ? e.message : String(e);
          console.error('RAID member load failed:', member.path, e);
        }
      }
      if (raidMemberQueue.value.some(member => member.status === 'loaded')) await detectRaidMemberQueue();
    }).catch(error => {
      if (captured !== generation) return;
      notifyError(tr("添加成员盘失败"), String(error));
    });
    return memberLoadQueue;
  }

  async function detectRaidMemberQueue() {
    const captured = generation;
    if (!raidMemberQueue.value.some(member => member.status === 'loaded')) {
      raidMemberStack.value = null;
      raidMemberError.value = null;
      return;
    }

    raidMemberDetecting.value = true;
    raidMemberError.value = null;
    try {
      const stack = await detectRaid({ promptAssembled: false });
      if (captured !== generation) return;
      if (!stack) {
        raidMemberError.value = raidStackError.value ?? tr("自动检测未返回结果，请稍后重试。");
        return;
      }
      raidMemberStack.value = stack;
      const memberIds = raidMemberImageIds(stack);
      for (const member of raidMemberQueue.value) {
        member.isMember = Boolean(member.id && memberIds.has(member.id));
      }
    } finally {
      if (captured === generation) raidMemberDetecting.value = false;
    }
  }

  function raidArrayImageIds(stack: RaidStack, array: RaidArray): Set<string> {
    const ids = new Set<string>();
    for (const member of array.members) {
      const origin = member.origin;
      if (origin.kind === 'whole') {
        ids.add(origin.image_id);
      } else {
        const vg = stack.volume_groups.find(item => item.vg_uuid === origin.vg_uuid);
        for (const pv of vg?.members ?? []) ids.add(pv.image_id);
      }
    }
    return ids;
  }

  function stagedRaidMemberIds(): Set<string> {
    return new Set(raidMemberQueue.value.map(member => member.id).filter((id): id is string => Boolean(id)));
  }

  function relevantRaidArrays(stack: RaidStack): RaidArray[] {
    const stagedIds = stagedRaidMemberIds();
    return stack.arrays.filter(array => {
      for (const imageId of raidArrayImageIds(stack, array)) {
        if (stagedIds.has(imageId)) return true;
      }
      return false;
    });
  }

  async function removeRaidMember(member: RaidMemberQueueItem) {
    if (member.status === 'loading') return;
    raidMemberQueue.value = raidMemberQueue.value.filter(item => item.path !== member.path);
    if (member.id && !member.preExisting && activeCase.value) {
      try {
        await removeEvidence(member.id);
      } catch (e) {
        console.error('remove RAID member evidence failed:', member.path, e);
        notifyError(tr("移出成员盘失败"), String(e));
      }
    }
    if (raidMemberQueue.value.some(item => item.status === 'loaded')) {
      await detectRaidMemberQueue();
    } else {
      raidMemberStack.value = null;
      raidMemberError.value = null;
    }
  }

  async function confirmRaidMemberOpen() {
    let stack = raidMemberStack.value;
    if (!stack) {
      await detectRaidMemberQueue();
      stack = raidMemberStack.value;
    }
    if (!stack) return;

    const assembled = relevantRaidArrays(stack).filter(array => array.assembled_image_id);
    if (assembled.length === 0) {
      notifyError(tr("尚无可打开的 RAID 阵列"), tr("请继续添加成员盘，或在镜像信息页使用手动重组。"));
      return;
    }

    raidMemberOpening.value = true;
    try {
      const memberIds = raidMemberImageIds(stack);
      const strays = raidMemberQueue.value.filter(
        member => member.id && !memberIds.has(member.id) && !member.preExisting,
      );
      for (const stray of strays) {
        try {
          await removeEvidence(stray.id!);
        } catch (e) {
          console.error('prune non-member RAID queue item failed:', stray.path, e);
        }
      }

      // Logical volumes spanning PVs on these arrays (NAS hybrid volumes) open
      // with them: their arrays alone only show a partial LVM volume group.
      const arrayImages = assembled.map(array => array.assembled_image_id!);
      const spanningVolumes = stack.volume_groups
        .filter(vg => vg.members.some(member => arrayImages.includes(member.image_id)))
        .flatMap(vg => vg.logical_volumes.map(lv => lv.assembled_image_id))
        .filter((id): id is string => Boolean(id));
      let opened = 0;
      for (const imageId of [...arrayImages, ...spanningVolumes]) {
        try {
          const summary = await imageInfoById({ imageId });
          const result = await openImagePath(summary.path);
          if (result.ok) opened += 1;
        } catch (e) {
          notifyError(tr("打开重组镜像失败"), String(e));
        }
      }

      if (opened > 0) {
        const notes: string[] = [];
        if (strays.length > 0) notes.push(tr("{0} 块非成员盘未加入案件", { 0: strays.length }));
        notifyInfo(tr("RAID 重组镜像已打开"), notes.join('；') || tr("已打开 {0} 个重组镜像标签页", { 0: opened }));
        showRaidMemberDialog.value = false;
      }
    } finally {
      raidMemberOpening.value = false;
    }
  }

  watch(() => activeCase.value?.dir ?? null, () => {
    resetRaidMemberDialog();
    showRaidMemberDialog.value = false;
  }, { flush: 'sync' });

  return {
    showRaidMemberDialog, raidMemberQueue, raidMemberStack, raidMemberDetecting,
    raidMemberOpening, raidMemberError, openRaidMemberDialog, addRaidMemberFiles, addRaidMemberPaths,
    detectRaidMemberQueue, removeRaidMember, confirmRaidMemberOpen, detectRaid,
  };
}
