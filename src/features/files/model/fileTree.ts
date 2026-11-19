import { shouldShowDirectoryWithEmptyFilter } from './emptyDirectoryFilter';
import { isHostDockerContainerRootId } from './model';
import type { NodeId } from '../../../lib/ipc/ids';
export interface TreeStackFrame {
  id: string;
  name: string;
}

export interface TreeNodeData {
  id: string;
  name: string;
  parentId: NodeId | null;
  kind: 'dir' | 'file' | 'symlink' | 'unallocated';
  expanded: boolean;
  loaded: boolean;
  loading: boolean;
  childCount: number | null;
  /** 源 Windows 系统给该卷分配的原始盘符（如 "C:"）。仅分区根节点经
   *  `fetchDriveLetters` 异步回填；其余节点恒为 null。 */
  driveLetter: string | null;
}

/** One render row. Depth and the v-for key derive from the RENDER PATH, not
 *  the node: the store is flat by id, and the same id can legitimately hang
 *  under two parents (e.g. a subvolume root listed both nested and at the
 *  partition root by an older snapshot) — per-node depth/key would collide. */
export interface TreeRow {
  node: TreeNodeData;
  depth: number;
  key: string;
}

export function visibleFileTreeRows(nodes: ReadonlyMap<string, TreeNodeData>, childrenOf: ReadonlyMap<string, string[]>, emptySubtreeOf: ReadonlyMap<string, boolean>, roots: readonly string[], hideEmpty: boolean): TreeRow[] {
  const out: TreeRow[] = [];
  // onPath 防御渲染环：扁平存储里坏数据（自引用的 childrenOf）不该挂死 UI。
  const onPath = new Set<string>();
  const walk = (ids: string[], depth: number, prefix: string) => {
    for (const id of ids) {
      const node = nodes.get(id);
      if (!node || onPath.has(id)) continue;
      // 隐藏空目录：跳过「整棵子树无文件」的子级目录。优先用后端快照的子树判定
      // （emptySubtreeOf：含「里面全是空目录」的递归情形）；尚未判定且子项计数也
      // 未返回时先隐藏，避免重新装载时把未知目录短暂当成非空目录画出来。分区根
      // （depth 0，可能因上锁/不支持而报空）与文件叶子一律保留。
      if (
        hideEmpty
        && depth > 0
        && node.kind === 'dir'
        && !isHostDockerContainerRootId(id)
      ) {
        if (!shouldShowDirectoryWithEmptyFilter(emptySubtreeOf.get(id), node.childCount)) continue;
      }
      const key = `${prefix}/${id}`;
      out.push({ node, depth, key });
      if (node.expanded) {
        const kids = childrenOf.get(id);
        if (kids) {
          onPath.add(id);
          walk(kids, depth + 1, key);
          onPath.delete(id);
        }
      }
    }
  };
  walk([...roots], 0, '');
  return out;
}

export function fileTreePathFor(nodes: ReadonlyMap<string, TreeNodeData>, id: string): TreeStackFrame[] {
  const frames: TreeStackFrame[] = [];
  let cur: TreeNodeData | undefined = nodes.get(id);
  const seen = new Set<string>();
  while (cur && !seen.has(cur.id)) {
    seen.add(cur.id);
    frames.unshift({ id: cur.id, name: cur.name });
    if (cur.parentId == null) break;
    cur = nodes.get(cur.parentId);
  }
  return [{ id: 'root', name: '/' }, ...frames];
}
