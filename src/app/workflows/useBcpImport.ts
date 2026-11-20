import { onScopeDispose, watch } from 'vue';
import { notifyError } from '../../composables/useToast';
import { tr } from '../../composables/useLocale';
import { importBcpAnalysis } from '../../lib/ipc/analysisResults';
import { IpcCommandError } from '../../lib/ipc/client';
import { fsSourceLocate } from '../../lib/ipc/fs/query';
import type { ImageId } from '../../lib/ipc/ids';
import { listLoadedImages } from '../../lib/ipc/image';
import { raidTabPathKey } from '../workspace/raidTabAliases';

/** Imports from an already-open evidence tab and publishes to the shared analysis tree. */
export function useBcpImport(options: {
  caseId: () => string | undefined;
  onPublished: (imageId: ImageId, runId: string) => void;
}) {
  let caseRevision = 0;
  watch(options.caseId, () => { caseRevision++; }, { flush: 'sync' });
  onScopeDispose(() => { caseRevision++; });
  const importing = new Set<string>();

  return async function importBcp(path: string, mode: 'explicit' | 'detect-zip'): Promise<void> {
    if (!options.caseId()) return;
    const revision = caseRevision;
    try {
      const loaded = (await listLoadedImages()).find(image => raidTabPathKey(image.path) === raidTabPathKey(path));
      if (caseRevision !== revision) return;
      if (!loaded) throw new Error(tr("加载未完成"));

      if (mode === 'detect-zip') {
        try {
          const index = await fsSourceLocate({ imageId: loaded.id, rootId: 'root', path: 'GAB_ZIP_INDEX.xml' });
          if (!index.file_id || caseRevision !== revision) return;
        } catch (error) {
          if (error instanceof IpcCommandError && error.code === 'notFound') return;
          throw error;
        }
      }

      const key = `${revision}:${loaded.id}`;
      if (importing.has(key)) return;
      importing.add(key);
      try {
        const result = await importBcpAnalysis(loaded.id);
        if (caseRevision === revision) options.onPublished(loaded.id, result.runId);
      } finally {
        importing.delete(key);
      }
    } catch (error) {
      if (caseRevision === revision && !(error instanceof IpcCommandError && error.code === 'cancelled')) {
        notifyError(tr("导入 BCP 结果失败"), String(error));
      }
    }
  };
}
