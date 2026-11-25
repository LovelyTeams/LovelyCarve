import { onScopeDispose } from 'vue';
import { tr } from '../../composables/useLocale';
import { confirm } from '../../composables/useConfirm';
import { caseList, type CaseInfo, type CaseListItem } from '../../lib/ipc/cases';
import { onEvidenceLinkReceived, takeEvidenceLinks, type EvidenceLinkPlace, type PluginInstallLink } from '../../lib/ipc/evidenceLinks';
import type { FileLocationTarget } from '../../lib/models/fileLocation';
import type { ImageId } from '../../lib/ipc/ids';

/** Opens what received `lovelycarve://` links name: their case (asking before
 *  another open case closes), evidence and file, which the file list selects,
 *  previews and, for a byte range, highlights. Links that arrive while one
 *  opens wait; only the latest of them opens next. Plugin links go to Settings. */
export function useEvidenceLinks(options: {
  activeCase: () => CaseInfo | null;
  refreshActiveCase: () => Promise<void>;
  /** Open a registered case and restore its evidence; false when it did not open. */
  openCase: (entry: CaseListItem) => Promise<boolean>;
  /** Show an evidence item of the active case at its root. */
  openEvidence: (imageId: ImageId) => Promise<void>;
  openLocation: (target: FileLocationTarget) => Promise<void>;
  openPluginInstall: (link: PluginInstallLink) => void;
  onError: (title: string, message: string) => void;
}) {
  let disposed = false;
  let opening = false;
  let latest: EvidenceLinkPlace | null = null;
  let unlisten: (() => void) | null = null;

  async function receive(): Promise<void> {
    let received;
    try {
      received = await takeEvidenceLinks();
    } catch (error) {
      if (!disposed) options.onError(tr("打开链接失败"), String(error));
      return;
    }
    if (disposed) return;
    for (const item of received) {
      if (!item.link && !item.plugin_install) options.onError(tr("链接无效"), item.error ?? item.url);
    }
    const install = received.filter(item => item.plugin_install).at(-1)?.plugin_install;
    if (install) options.openPluginInstall(install);
    const next = received.filter(item => item.link).at(-1)?.link;
    if (!next) return;
    latest = next;
    if (opening) return;
    opening = true;
    try {
      while (latest && !disposed) {
        const link = latest;
        latest = null;
        try {
          await open(link);
        } catch (error) {
          if (!disposed) options.onError(tr("打开链接失败"), error instanceof Error ? error.message : String(error));
        }
      }
    } finally {
      opening = false;
    }
  }

  async function open(link: EvidenceLinkPlace): Promise<void> {
    if (!options.activeCase()) await options.refreshActiveCase();
    const current = options.activeCase();
    if (current?.id !== link.case_id) {
      const entry = (await caseList()).find(item => item.id === link.case_id);
      if (!entry) throw new Error(tr("本机案件列表中没有链接所指的案件"));
      if (entry.missing) throw new Error(tr("无法读取案件目录：{0}", { 0: entry.dir }));
      if (current && !await confirm({
        title: tr("切换到案件「{0}」？", { 0: entry.name }),
        message: tr("当前案件「{0}」将关闭，它的标签页随之关闭，检材上运行中的任务会停止。", { 0: current.name }),
        confirmText: tr("切换案件"),
      })) return;
      if (disposed || !await options.openCase(entry) || options.activeCase()?.id !== link.case_id) return;
    }
    const imageId = link.image_id;
    if (!imageId) return;
    if (!options.activeCase()?.evidence.some(item => item.image_id === imageId)) {
      throw new Error(tr("链接所指的检材不在案件中"));
    }
    if (link.node_id) {
      await options.openLocation({ imageId, nodeId: link.node_id, offset: link.offset, length: link.length });
    } else if (link.root_id && link.path) {
      await options.openLocation({ imageId, rootId: link.root_id, path: link.path });
    } else {
      await options.openEvidence(imageId);
    }
  }

  // Subscribe before taking the queue, so no link arrives in between unseen.
  void onEvidenceLinkReceived(() => void receive())
    .then(stop => {
      if (disposed) stop();
      else unlisten = stop;
    }, error => console.warn('subscribe to evidence links failed', error))
    .finally(() => {
      if (!disposed) void receive();
    });
  onScopeDispose(() => {
    disposed = true;
    unlisten?.();
    unlisten = null;
  });
}
