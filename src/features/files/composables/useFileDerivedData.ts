
import { tr } from '../../../composables/useLocale';
import { onScopeDispose, reactive } from 'vue';
import { fsDigestLoadNodes, fsDigestPersist, fsFtypeLoadNodes, fsFtypePersist, type FsTypeIdent, type StoredDigest } from '../../../lib/ipc/fs/derived';
import { notifyError } from '../../../composables/useToast';
import type { DiskFsNode as FsNode } from '../model/model';
import { ALL_HASH_ALGOS, chunkNodeIds, fileNodeIdsForDerivedHydration } from '../model/derivedData';

const pendingWrites = new Set<Promise<void>>();
function trackWrite(write: Promise<unknown>, title: string) {
  const pending = write.then(() => {}, error => { notifyError(title, String(error)); });
  pendingWrites.add(pending);
  void pending.finally(() => pendingWrites.delete(pending));
}
export async function flushFileDerivedWrites(): Promise<void> {
  while (pendingWrites.size) await Promise.all([...pendingWrites]);
}

/** Per-evidence derived columns; directory pages share one cache owner. */
export function useFileDerivedData(options: {
  imageId: () => string;
  queryGeneration: () => number;
  revealTypeColumn: () => void;
}) {
  const digestCache = reactive(new Map<string, StoredDigest>());
  const ftypeCache = reactive(new Map<string, FsTypeIdent>());
  const digestHydratedNodeIds = new Set<string>();
  const ftypeHydratedNodeIds = new Set<string>();
  let hashGeneration = 0;
  let hydrationGeneration = 0;
  let disposed = false;
  onScopeDispose(() => { disposed = true; hashGeneration++; });

  function clearHydration() {
    hydrationGeneration++;
    digestHydratedNodeIds.clear();
    ftypeHydratedNodeIds.clear();
  }

  function reset() {
    hashGeneration++;
    digestCache.clear();
    ftypeCache.clear();
    clearHydration();
  }

  async function hydrateDigests(nodes: FsNode[], loadGen: number) {
    const hashGen = hashGeneration;
    const hydrationGen = hydrationGeneration;
    const imageId = options.imageId();
    const nodeIds = fileNodeIdsForDerivedHydration(nodes, digestHydratedNodeIds);
    if (nodeIds.length === 0) return;
    try {
      for (const chunk of chunkNodeIds(nodeIds)) {
        const stored = await fsDigestLoadNodes({ imageId, nodeIds: chunk });
        if (disposed || hashGen !== hashGeneration || hydrationGen !== hydrationGeneration || loadGen !== options.queryGeneration()) return;
        for (const nodeId of chunk) digestHydratedNodeIds.add(nodeId);
        for (const [nodeId, digest] of Object.entries(stored)) {
          const cached = digestCache.get(nodeId);
          if (!cached) {
            digestCache.set(nodeId, digest);
            continue;
          }
          // 缓存里已有的节点按算法补缺：本会话刚算的更新鲜不覆盖，存档里
          // 多出来的算法（如上次会话算过 SHA256、本次只算了 MD5）照样回填。
          const merged: StoredDigest = { ...cached };
          let filled = false;
          for (const a of ALL_HASH_ALGOS) {
            if (!merged[a] && digest[a]) {
              merged[a] = digest[a];
              filled = true;
            }
          }
          if (filled) digestCache.set(nodeId, merged);
        }
      }
    } catch (err) {
      // 存档读不到不打断浏览——列保持空，可重新计算。
      console.warn('hydrate digests failed', err);
    }
  }

  function persistDigests(imageId: string, entries: Record<string, StoredDigest>) {
    if (Object.keys(entries).length === 0) return;
    trackWrite(fsDigestPersist({ imageId, entries }), tr("哈希持久化失败"));
  }

  async function hydrateFtypes(nodes: FsNode[], loadGen: number) {
    const hashGen = hashGeneration;
    const hydrationGen = hydrationGeneration;
    const imageId = options.imageId();
    const nodeIds = fileNodeIdsForDerivedHydration(nodes, ftypeHydratedNodeIds);
    if (nodeIds.length === 0) return;
    try {
      for (const chunk of chunkNodeIds(nodeIds)) {
        const stored = await fsFtypeLoadNodes({ imageId, nodeIds: chunk });
        if (disposed || hashGen !== hashGeneration || hydrationGen !== hydrationGeneration || loadGen !== options.queryGeneration()) return;
        for (const nodeId of chunk) ftypeHydratedNodeIds.add(nodeId);
        let any = false;
        for (const [nodeId, ident] of Object.entries(stored)) {
          if (!ftypeCache.has(nodeId)) {
            ftypeCache.set(nodeId, ident);
            any = true;
          }
        }
        if (any) options.revealTypeColumn();
      }
    } catch (err) {
      // 存档读不到不打断浏览——列保持空，可重新识别。
      console.warn('hydrate ftypes failed', err);
    }
  }

  function persistFtypes(imageId: string, entries: Record<string, FsTypeIdent>) {
    if (Object.keys(entries).length === 0) return;
    trackWrite(fsFtypePersist({ imageId, entries }), tr("类型识别持久化失败"));
  }

  return { digestCache, ftypeCache, hydrateDigests, hydrateFtypes, persistDigests, persistFtypes, clearHydration, reset, generation: () => hashGeneration };
}
