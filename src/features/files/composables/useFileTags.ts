
import { FILE_TAGS, type FileTag } from '../model/tags';
// 文件标签（书签）：检查员把可疑/重点文件沉淀下来供筛选与报告引用。
//
// 模块级单例状态，双层持久化：
//   1. localStorage（键 imageId → nodeId → 标签）——无案件也能用，本机即时生效；
//   2. 案件 case.sqlite3.file_tags（普通打标用 fs_tags_apply_changes 增量
//      upsert/delete；fs_tags_save 仅保留给全量兼容写入，旧 tags/*.json 仍可
//      兼容读取）——标签随案件目录走，换机器/清浏览器存储后重开案件仍在。
//      无案件时后端返回 false，静默退回仅 localStorage。
// imageId 在案件工作流里跨会话稳定（commands/image.rs 按证据路径复用 id），
// 所以重开案件后标签仍能对上；无案件裸开镜像时 id 是新 uuid，标签只活在本机。
import { reactive } from 'vue';

import { fsTagsApplyChanges, fsTagsLoad, fsTagsLoadNodes, fsTagsSave } from '../../../lib/ipc/fs/tags';
import type { ImageId, NodeId } from '../../../lib/ipc/ids';

const STORE_KEY = 'lc.fileTags';

type TagStore = Record<string, Record<string, FileTag>>;

function loadStore(): TagStore {
  try {
    const raw = localStorage.getItem(STORE_KEY);
    if (raw) return JSON.parse(raw) as TagStore;
  } catch {
    // 损坏的存档按空库处理；下一次打标会重建。
  }
  return {};
}

const store = reactive<TagStore>(loadStore());
let saveTimer: number | null = null;
let hydrationGeneration = 0;
const pendingWrites = new Set<Promise<void>>();
function trackWrite(write: Promise<unknown>) {
  const pending = write.then(() => {}, error => console.warn('persist file tags failed', error));
  pendingWrites.add(pending);
  void pending.finally(() => pendingWrites.delete(pending));
}
type PendingTagChanges = {
  upserts: Record<string, FileTag>;
  deletes: Set<string>;
};

/** 等待增量同步到案件目录的标签变更（同一防抖窗口内可能改了多个镜像）。 */
const pendingCaseChanges = new Map<string, PendingTagChanges>();
/** 本会话已从案件目录水合过的镜像，避免每次切镜像都重拉。案件切换时清空。 */
const hydratedImages = new Set<string>();
/** 本会话已按节点集合水合过的标签，避免目录来回切换时重复读。 */
const hydratedNodeIdsByImage = new Map<string, Set<string>>();

function isFileTag(value: string): value is FileTag {
  return (FILE_TAGS as readonly string[]).includes(value);
}

function pendingFor(imageId: ImageId): PendingTagChanges {
  let pending = pendingCaseChanges.get(imageId);
  if (!pending) {
    pending = { upserts: {}, deletes: new Set() };
    pendingCaseChanges.set(imageId, pending);
  }
  return pending;
}

function queueCaseTagChange(imageId: ImageId, nodeIds: NodeId[], tag: FileTag | null) {
  if (nodeIds.length === 0) return;
  const pending = pendingFor(imageId);
  for (const nodeId of nodeIds) {
    if (tag) {
      pending.upserts[nodeId] = tag;
      pending.deletes.delete(nodeId);
    } else {
      delete pending.upserts[nodeId];
      pending.deletes.add(nodeId);
    }
  }
}

function queueCaseTagUpserts(imageId: ImageId, entries: Record<string, FileTag>) {
  const pairs = Object.entries(entries);
  if (pairs.length === 0) return;
  const pending = pendingFor(imageId);
  for (const [nodeId, tag] of pairs) {
    pending.upserts[nodeId] = tag;
    pending.deletes.delete(nodeId);
  }
}

/** 把某镜像的完整标签表推到案件目录。无案件时后端 no-op（返回 false）；
 *  真实写盘失败只告警——localStorage 副本还在，重开本机不丢。 */
function pushFullToCase(imageId: ImageId) {
  const entries: Record<string, string> = { ...(store[imageId] ?? {}) };
  trackWrite(fsTagsSave({ imageId, entries }));
}

function pushChangesToCase(imageId: ImageId, changes: PendingTagChanges) {
  const upserts: Record<string, string> = { ...changes.upserts };
  const deleteNodeIds = Array.from(changes.deletes);
  if (Object.keys(upserts).length === 0 && deleteNodeIds.length === 0) return;
  trackWrite(fsTagsApplyChanges({ imageId, upserts, deleteNodeIds }));
}

function drainWrites() {
  if (saveTimer !== null) window.clearTimeout(saveTimer);
  saveTimer = null;
  try { localStorage.setItem(STORE_KEY, JSON.stringify(store)); } catch { /* Memory remains usable if storage is full. */ }
  const pending = Array.from(pendingCaseChanges.entries());
  pendingCaseChanges.clear();
  for (const [imageId, changes] of pending) pushChangesToCase(imageId, changes);
}

function persist() {
  if (saveTimer !== null) window.clearTimeout(saveTimer);
  saveTimer = window.setTimeout(drainWrites, 300);
}

/** Finish writes while the backend still owns the original case. */
export async function flushFileTagWrites(): Promise<void> {
  hydrationGeneration++;
  hydratedImages.clear();
  hydratedNodeIdsByImage.clear();
  drainWrites();
  while (pendingWrites.size) await Promise.all([...pendingWrites]);
}

export function useFileTags() {
  function tagOf(imageId: ImageId, nodeId: NodeId): FileTag | undefined {
    return store[imageId]?.[nodeId];
  }

  /** 批量打标 / 清除（tag 为 null 时清除）。 */
  function setTag(imageId: ImageId, nodeIds: NodeId[], tag: FileTag | null) {
    let bucket = store[imageId];
    if (!bucket) {
      if (!tag) {
        queueCaseTagChange(imageId, nodeIds, null);
        persist();
        return;
      }
      bucket = {};
      store[imageId] = bucket;
    }
    for (const id of nodeIds) {
      if (tag) bucket[id] = tag;
      else delete bucket[id];
    }
    queueCaseTagChange(imageId, nodeIds, tag);
    persist();
  }

  /** 从案件目录水合本镜像的标签存档，并把会话内已打的标签补存回去。
   *  合并策略：本地缺的节点用存档补（换机器/清存储后恢复）；冲突本地优先
   *  （本地是本会话检查员刚改的）。合并后若与存档有出入则整表回写，覆盖
   *  「先打标、后开案件」的补存场景。`force` 用于案件切换后强制重拉。 */
  async function hydrateTags(imageId: ImageId, force = false) {
    if (!force && hydratedImages.has(imageId)) return;
    const generation = hydrationGeneration;
    let stored: Record<string, string>;
    try {
      stored = await fsTagsLoad({ imageId });
      if (generation !== hydrationGeneration) return;
      hydratedImages.add(imageId);
    } catch (err) {
      // 存档读不到不打断浏览；localStorage 副本照常工作。
      console.warn('hydrate file tags failed', err);
      return;
    }
    let bucket = store[imageId];
    let divergent = Object.keys(bucket ?? {}).length > 0;
    for (const [nodeId, tag] of Object.entries(stored)) {
      if (!isFileTag(tag)) continue; // 手改/未来版本的未知标签：跳过不渲染也不覆盖
      if (!bucket) {
        bucket = {};
        store[imageId] = bucket;
      }
      if (bucket[nodeId] === tag) continue;
      if (bucket[nodeId]) {
        // 本地优先：保留本会话的值，整表回写时会覆盖存档。
        continue;
      }
      bucket[nodeId] = tag;
    }
    // 本地有存档没有（或值不同）的才需要回写；纯回填则存档已是事实。
    if (divergent) {
      const local = store[imageId] ?? {};
      divergent =
        Object.keys(local).length !== Object.keys(stored).length ||
        Object.entries(local).some(([nodeId, tag]) => stored[nodeId] !== tag);
    }
    persist();
    if (divergent) pushFullToCase(imageId);
  }

  /** 只水合指定节点集合的标签。适合文件列表当前目录 / 全显批次：读侧按需，
   *  写侧若发现本地副本与案件不一致，只把这些节点的本地优先结果增量回写，
   *  不会覆盖案件里未水合的其它标签。 */
  async function hydrateTagsForNodes(imageId: ImageId, nodeIds: NodeId[], force = false) {
    const uniqueNodeIds = Array.from(new Set(nodeIds));
    if (uniqueNodeIds.length === 0) return;
    let hydrated = hydratedNodeIdsByImage.get(imageId);
    if (!hydrated) {
      hydrated = new Set();
      hydratedNodeIdsByImage.set(imageId, hydrated);
    }
    const pendingNodeIds = force ? uniqueNodeIds : uniqueNodeIds.filter(id => !hydrated.has(id));
    if (pendingNodeIds.length === 0) return;

    const generation = hydrationGeneration;
    let stored: Record<string, string>;
    try {
      stored = await fsTagsLoadNodes({ imageId, nodeIds: pendingNodeIds });
    } catch (err) {
      // 存档读不到不打断浏览；localStorage 副本照常工作。
      console.warn('hydrate file tags failed', err);
      return;
    }
    if (generation !== hydrationGeneration) return;
    for (const nodeId of pendingNodeIds) hydrated.add(nodeId);

    let bucket = store[imageId];
    const localWins: Record<string, FileTag> = {};
    for (const [nodeId, tag] of Object.entries(stored)) {
      if (!isFileTag(tag)) continue; // 手改/未来版本的未知标签：跳过不渲染也不覆盖
      if (!bucket) {
        bucket = {};
        store[imageId] = bucket;
      }
      if (bucket[nodeId] === tag) continue;
      if (bucket[nodeId]) {
        localWins[nodeId] = bucket[nodeId];
        continue;
      }
      bucket[nodeId] = tag;
    }
    for (const nodeId of pendingNodeIds) {
      const local = bucket?.[nodeId];
      if (local && stored[nodeId] !== local) localWins[nodeId] = local;
    }
    queueCaseTagUpserts(imageId, localWins);
    persist();
  }

  /** 案件切换时调用：已水合标记作废，下次进入文件列表按新案件重拉。 */
  function resetHydration() {
    hydrationGeneration++;
    hydratedImages.clear();
    hydratedNodeIdsByImage.clear();
  }

  return { tagOf, setTag, hydrateTags, hydrateTagsForNodes, resetHydration };
}
