
import { tr } from '../../composables/useLocale';
import { notifyInfo, notifyError } from '../../composables/useToast';
import type { LoadFileOptions, OpenTabResult, WorkspaceTab } from '../workspace/useTabManager';
import type { useRaidStack } from '../../features/raid/api';
import { imageLoad } from '../../lib/ipc/image';
import type { EvidenceRef } from '../../lib/ipc/cases';
import { raidMemberTabAliases, raidTabPathKey } from '../workspace/raidTabAliases';

/** Restore real members before reconstructed arrays and retire removed evidence tabs. */
export function useCaseWorkspace(options: {
  openImagePath: (path: string, options?: LoadFileOptions) => Promise<OpenTabResult>;
  detectRaid: ReturnType<typeof useRaidStack>['detect'];
  tabs: () => WorkspaceTab[];
  closeTab: (id: string) => void;
  evidence: () => readonly EvidenceRef[];
  caseDirectory?: () => string | null;
}) {
  const { openImagePath, detectRaid, closeTab } = options;
  /** Restore sources sequentially, then open independent evidence and arrays.
   *  Failures (file moved/deleted) are skipped and surfaced together. Synthetic
   *  `assembled://` RAID evidence opens last: a fresh session has no registered
   *  synthetic images, so a raid_auto_detect run (which re-registers them under
   *  their case-recorded ids) must come after the member images have loaded. */
  async function onOpenCaseEvidence(paths: string[]) {
    const capturedDirectory = options.caseDirectory?.();
    const current = () => options.caseDirectory?.() === capturedDirectory;
    const failed: string[] = [];
    let queued = 0;
    const real = paths.filter((p) => !p.startsWith('assembled://'));
    const assembled = paths.filter((p) => p.startsWith('assembled://'));
    const unavailable = new Set<string>();
    let memberAliases: ReadonlyMap<string, string> = new Map();
    if (assembled.length > 0) {
      // Register sources before detection without creating member tabs.
      for (const path of real) {
        if (/\.(?:zip|7z|tar|tgz|gz|bak|ab|ldbk)$/i.test(path)) continue;
        if (!current()) return;
        try {
          await imageLoad({ path });
          if (!current()) return;
        } catch (error) {
          if (!current()) return;
          unavailable.add(path);
          failed.push(`${path.split(/[\\/]/).pop() || path}（${String(error)}）`);
        }
      }
      const stack = await detectRaid({ promptAssembled: false });
      if (!current()) return;
      memberAliases = raidMemberTabAliases(stack, options.evidence());
    }
    for (const path of real) {
      if (!current()) return;
      if (unavailable.has(path) || memberAliases.has(raidTabPathKey(path))) continue;
      // 案件证据都是镜像/文件夹（source_format 属 ImageFormat），统一强制走
      // image_load 分支。否则文件夹证据（如 C:\Users\…\Downloads，无磁盘镜像
      // 扩展名）会被扩展名检查拒绝——重开案件时只剩
      // 路径、丢了 source_format，无法靠扩展名识别。
      const result = await openImagePath(path, { forceImage: true, quiet: true });
      if (!current()) return;
      if (!result.ok) failed.push(`${path.split(/[\\/]/).pop() || path}（${result.error}）`);
      else if (result.queued) queued++;
    }
    if (assembled.length > 0) {
      // Source staging and detection above already registered the synthetic
      // sessions under their recorded ids. Open only these array views here.
      for (const path of assembled) {
        if (!current()) return;
        const result = await openImagePath(path, { quiet: true });
        if (!current()) return;
        if (!result.ok) failed.push(`${path.replace('assembled://', '')}（${result.error}）`);
      }
    }
    const opened = paths.length - failed.length;
    if (failed.length > 0) {
      notifyError(
        tr("{0} 个检材未能打开", { 0: failed.length }),
        tr("已打开 {0} 个；以下检材可能已被移动或删除，或 RAID 成员不全：{1}", { 0: opened, 1: failed.join('；') }),
      );
    } else if (queued > 0) {
      notifyInfo(tr("检材正在后台加载"), tr("{0} 个检材正在加载，可在标签页或任务中心查看进度。", { 0: queued }));
    } else if (opened > 0) {
      notifyInfo(tr("已加载案件检材"), tr("已加载 {0} 个检材", { 0: opened }));
    }
  }

  /** Close the tab for an evidence image the user just removed from the case. */
  function onCaseEvidenceRemoved(path: string) {
    const tab = options.tabs().find((t) => t.filePath === path);
    if (tab) void closeTab(tab.id);
  }

  return { onOpenCaseEvidence, onCaseEvidenceRemoved };
}
