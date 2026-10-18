
import { tr } from '../../../composables/useLocale';
// 「自动挂载案件全部分区」编排。
//
// 打开案件时（且挂载方式为「文件夹」、开关开启），依次把案件内每块证据磁盘
// 载入后端镜像池，并把它们的全部可挂载分区只读挂到默认挂载根目录下
// （`<根目录>/<镜像 id>/<分区名>`，复用 fs_create_mount_dir 的建夹逻辑）。一次性
// 把整个案件的分区都挂上，省去逐块磁盘、逐分区点「挂载」。
//
// 仅「文件夹」模式：驱动器盘符 H–Y 只有十几个，多块磁盘的案件会很快耗尽。
// BitLocker 等需密钥解锁的分区跳过（自动挂载拿不到密钥）。已挂载的分区按
// (镜像 id, 分区节点) 去重跳过，故重复打开同一案件不会重复挂或建出 `分区0 (2)`。
import { ref } from 'vue';

import type { CaseInfo } from '../../../lib/ipc/cases';
import { fsCreateMountDir, fsMountReadonly, fsMounts } from '../../../lib/ipc/mount';
import { imageLoadDetail } from '../../../lib/ipc/image';
import { useUiPreferences } from '../../../composables/useUiPreferences';
import { notifyError, notifyInfo, notifySuccess } from '../../../composables/useToast';

// 可自动挂载的分区类型：自带文件系统、无需解锁。与 DiskImageInfo 的
// MOUNTABLE_KINDS 保持一致，但去掉 bitlocker（需解锁，自动流程跳过）。
const AUTO_MOUNTABLE_KINDS = new Set([
  'ext4', 'btrfs', 'fat32', 'fat12', 'fat16', 'exfat', 'cdfs', 'squashfs',
  'apfscontainer', 'hfsplus', 'hfsx', 'lvm', 'ntfs', 'refs',
  'ufs', 'xfs', 'zfs',
]);

const autoMounting = ref(false);

export function useAutoMount() {
  const { mountTarget, mountFolderRoot, autoMountCaseEvidence } = useUiPreferences();

  // 打开案件后调用：满足条件就把案件内全部证据的可挂载分区挂上。设计为
  // 即发即忘（不阻塞案件打开），自带起止提示。并发再入用 autoMounting 挡掉。
  async function maybeAutoMountCase(caseInfo: CaseInfo | null): Promise<void> {
    if (!autoMountCaseEvidence.value) return; // 开关关闭
    if (mountTarget.value !== 'folder') return; // 仅文件夹模式
    if (autoMounting.value) return; // 上一轮还没跑完
    if (!caseInfo || caseInfo.evidence.length === 0) return;
    const root = mountFolderRoot.value.trim() || caseInfo.dir?.trim() || '';
    if (!root) return; // 无根目录无法自动建夹

    autoMounting.value = true;
    let mounted = 0;
    let failed = 0;
    let skipped = 0;
    try {
      // 已挂载的 (镜像 id, 分区节点) 集合：跳过它们，避免重复挂载 / 建重名夹。
      let alreadyMounted = new Set<string>();
      try {
        const views = await fsMounts();
        alreadyMounted = new Set(views.map(v => `${v.image_id}::${v.root_node_id}`));
      } catch {
        // 拿不到现有挂载列表不致命——大不了对已挂的再试一次（会失败计入 failed）。
      }

      notifyInfo(tr("自动挂载案件分区"), tr("正在加载 {0} 个检材并挂载其分区…", { 0: caseInfo.evidence.length }));

      for (const ev of caseInfo.evidence) {
        let detail;
        try {
          detail = await imageLoadDetail({ path: ev.path });
        } catch {
          failed += 1; // 整块磁盘载入失败（文件缺失 / 格式不支持等）
          continue;
        }
        for (const p of detail.partitions) {
          if (!AUTO_MOUNTABLE_KINDS.has(p.kind)) continue;
          const nodeId = `part:${p.id}`;
          if (alreadyMounted.has(`${detail.id}::${nodeId}`)) {
            skipped += 1;
            continue;
          }
          try {
            const name = p.label ?? tr("分区 {0}", { 0: p.id });
            const dir = await fsCreateMountDir({ root, imageId: detail.id, name });
            await fsMountReadonly({ imageId: detail.id, nodeId, mountPoint: dir });
            mounted += 1;
          } catch {
            failed += 1;
          }
        }
      }
    } finally {
      autoMounting.value = false;
    }

    if (mounted === 0 && failed === 0) return; // 没什么可挂的，安静收场
    const parts = [tr("已挂载 {0} 个分区", { 0: mounted })];
    if (skipped > 0) parts.push(tr("跳过 {0} 个已挂载", { 0: skipped }));
    if (failed > 0) parts.push(tr("失败 {0}", { 0: failed }));
    const detail = parts.join(' · ');
    if (failed === 0) notifySuccess(tr("自动挂载完成"), detail);
    else notifyError(tr("自动挂载部分失败"), detail);
  }

  return { maybeAutoMountCase, autoMounting };
}
