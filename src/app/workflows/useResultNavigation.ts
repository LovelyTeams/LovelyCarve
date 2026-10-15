
import { tr } from '../../composables/useLocale';

import { type FileListNavigator } from '../../features/files/model/navigation';

import { type FileLocationTarget } from '../../lib/models/fileLocation';
import { nextTick, ref, watch } from 'vue';

/** Activate the source evidence before using its asynchronously mounted list. */
export function useResultNavigation(options: {
  caseKey: () => string | null;
  imageId: () => string | null;
  openImage: (imageId: string) => Promise<boolean>;
  showFiles: () => void;
  isFilesActive: () => boolean;
  panel: () => FileListNavigator | null;
  onError: (message: string) => void;
}) {
  const locating = ref(false);
  let caseVersion = 0;
  watch(options.caseKey, () => { caseVersion++; }, { flush: 'sync' });
  async function openLocation(target: FileLocationTarget) {
    if (locating.value) return;
    const caseKey = options.caseKey();
    const version = caseVersion;
    locating.value = true;
    try {
      if (!target.imageId || !(target.nodeId || (target.rootId && target.path))) throw new Error(tr("该记录缺少文件定位信息"));
      if (target.offset !== undefined && (!Number.isSafeInteger(target.offset) || target.offset < 0)) throw new Error(tr("命中偏移无效，无法准确定位"));
      if (options.imageId() !== target.imageId && !await options.openImage(target.imageId)) {
        throw new Error(tr("无法打开来源检材，请检查检材文件是否可用"));
      }
      if (caseVersion !== version || options.caseKey() !== caseKey) return;
      if (options.imageId() !== target.imageId) throw new Error(tr("来源检材未能激活，请重试"));
      options.showFiles();
      await nextTick();
      for (let attempt = 0; attempt < 100; attempt++) {
        if (caseVersion !== version || options.caseKey() !== caseKey || options.imageId() !== target.imageId || !options.isFilesActive()) return;
        const panel = options.panel();
        if (panel?.imageId === target.imageId) {
          if (target.nodeId) await panel.openFileByNodeId(target.nodeId, target.offset, target.length);
          else if (target.rootId && target.path) await panel.openFileBySource(target.rootId, target.path);
          return;
        }
        await new Promise(resolve => setTimeout(resolve, 50));
      }
      throw new Error(tr("文件列表未能及时加载，请重试"));
    } catch (error) {
      if (caseVersion === version && options.caseKey() === caseKey) options.onError(String(error));
    } finally { locating.value = false; }
  }
  return { locating, openLocation };
}
