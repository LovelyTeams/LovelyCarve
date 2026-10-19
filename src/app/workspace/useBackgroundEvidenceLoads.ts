import { watch } from 'vue';
import { imageLoad } from '../../lib/ipc/image';
import { taskCancel } from '../../lib/ipc/taskRuntime';
import { IpcCommandError } from '../../lib/ipc/client';
import { notifyError } from '../../composables/useToast';
import { tr } from '../../composables/useLocale';
import type { LoadFileOptions, OpenTabResult, useTabManager } from './useTabManager';
import { raidTabPathKey } from './raidTabAliases';

/** Container preparation runs outside the singleton active file-session loader.
 *  The task center lists each load under the task id its tab carries. */
export function useBackgroundEvidenceLoads(tabs: ReturnType<typeof useTabManager>, caseDirectory: () => string | null) {
  const pending = new Map<string, { taskId: string; tabId: string | null; cancelling: boolean; completion?: Promise<unknown> }>();
  let revision = 0;
  watch(caseDirectory, () => { revision++; }, { flush: 'sync' });
  watch(() => tabs.tabs.value.map(tab => tab.loadTaskId), () => {
    for (const load of pending.values()) {
      if (!load.cancelling && load.tabId && !tabs.tabs.value.some(tab => tab.id === load.tabId)) {
        load.cancelling = true;
        void taskCancel({ taskId: load.taskId }).catch(error => console.warn('取消已关闭标签的加载失败', error));
      }
    }
  });

  async function open(path: string, opts?: LoadFileOptions): Promise<OpenTabResult> {
    const existing = tabs.tabs.value.find(tab => raidTabPathKey(tab.filePath) === raidTabPathKey(path));
    const key = raidTabPathKey(path);
    if (existing) {
      if (opts?.waitUntilReady && existing.loadTaskId) {
        try { await pending.get(key)?.completion; }
        catch (error) { return { ok: false, error: String(error) }; }
        if (existing.loadTaskId) return { ok: false, error: tr('加载未完成') };
      }
      return tabs.openNewTab(path, opts);
    }
    if (!/\.(?:zip|7z|tar|tgz|gz|bak|ab|ldbk)$/i.test(path)) return tabs.openNewTab(path, opts);
    const previous = pending.get(key);
    if (previous?.tabId && tabs.tabs.value.some(tab => tab.id === previous.tabId)) return { ok: true, queued: true };
    const epoch = revision;
    const directory = caseDirectory();
    const load = { taskId: crypto.randomUUID(), tabId: null as string | null, cancelling: false, completion: undefined as Promise<unknown> | undefined };
    pending.set(key, load);
    load.tabId = await tabs.beginBackgroundLoad(path, load.taskId, opts);
    const current = () => epoch === revision && directory === caseDirectory()
      && tabs.tabs.value.some(tab => tab.id === load.tabId);
    const completion = (async () => {
      if (!current()) throw new Error('image load cancelled');
      const loaded = await imageLoad({ path, taskId: load.taskId, ...(directory ? { caseDir: directory } : {}) });
      if (current() && load.tabId) await tabs.finishBackgroundLoad(load.tabId);
      return loaded;
    })();
    load.completion = completion;
    void completion.catch(error => {
      if (current() && !(error instanceof IpcCommandError && error.code === 'cancelled')) {
        notifyError(tr('打开检材失败'), `${path.split(/[\\/]/).pop()}：${error}`);
      }
    }).finally(() => { if (pending.get(key) === load) pending.delete(key); });
    if (opts?.waitUntilReady) {
      try { await completion; return current() ? { ok: true } : { ok: false, error: tr('加载未完成') }; }
      catch (error) { return { ok: false, error: String(error) }; }
    }
    return { ok: true, queued: true };
  }
  return { open };
}
